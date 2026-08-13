use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use mega_audio_capture::{
    AudioBackend, AudioBackendKind, AudioError, AudioInputCallback, AudioProvenance, AudioService,
    AudioServiceConfig, AudioSession, AudioSink, AudioSource, AudioTimestamp, BackendCapabilities,
    CallbackDisposition, PcmBuffer, PcmBufferSpec, PcmFormat, SinkError,
};

#[derive(Default)]
struct FakeBackend {
    callback: Mutex<Option<Arc<dyn AudioInputCallback>>>,
    stop_count: Arc<AtomicUsize>,
}

impl FakeBackend {
    fn callback(&self) -> Arc<dyn AudioInputCallback> {
        self.callback
            .lock()
            .expect("callback lock")
            .as_ref()
            .expect("backend started")
            .clone()
    }

    fn emit(&self, buffer: PcmBuffer) -> CallbackDisposition {
        self.callback().push(buffer)
    }
}

impl AudioBackend for FakeBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            microphone: true,
            system_audio: false,
        }
    }

    fn start(
        &self,
        _source: AudioSource,
        _generation: u64,
        callback: Arc<dyn AudioInputCallback>,
    ) -> Result<Box<dyn AudioSession>, AudioError> {
        *self.callback.lock().expect("callback lock") = Some(callback);
        Ok(Box::new(FakeSession {
            stop_count: Arc::clone(&self.stop_count),
        }))
    }
}

struct FakeSession {
    stop_count: Arc<AtomicUsize>,
}

impl AudioSession for FakeSession {
    fn stop(self: Box<Self>) -> Result<(), AudioError> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct CollectSink {
    segments: Mutex<Vec<mega_audio_capture::AudioSegmentMetadata>>,
}

impl AudioSink for CollectSink {
    fn store(&self, segment: mega_audio_capture::AudioSegment) -> Result<(), SinkError> {
        self.segments
            .lock()
            .expect("sink lock")
            .push(segment.metadata().clone());
        drop(segment);
        Ok(())
    }
}

struct BlockingState {
    entered: bool,
    release: bool,
}

struct BlockingSink {
    state: Mutex<BlockingState>,
    wake: Condvar,
    segments: Mutex<Vec<mega_audio_capture::AudioSegmentMetadata>>,
}

impl BlockingSink {
    fn new() -> Self {
        Self {
            state: Mutex::new(BlockingState {
                entered: false,
                release: false,
            }),
            wake: Condvar::new(),
            segments: Mutex::new(Vec::new()),
        }
    }

    fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("blocking sink lock");
        while !state.entered {
            state = self.wake.wait(state).expect("blocking sink wait");
        }
    }

    fn release(&self) {
        self.state.lock().expect("blocking sink lock").release = true;
        self.wake.notify_all();
    }
}

impl AudioSink for BlockingSink {
    fn store(&self, segment: mega_audio_capture::AudioSegment) -> Result<(), SinkError> {
        let mut state = self.state.lock().expect("blocking sink lock");
        if !state.entered {
            state.entered = true;
            self.wake.notify_all();
            while !state.release {
                state = self.wake.wait(state).expect("blocking sink wait");
            }
        }
        drop(state);
        self.segments
            .lock()
            .expect("sink lock")
            .push(segment.metadata().clone());
        drop(segment);
        Ok(())
    }
}

fn service(
    backend: Arc<FakeBackend>,
    sink: Arc<dyn AudioSink>,
    config: AudioServiceConfig,
) -> AudioService {
    AudioService::new(backend, sink, config).expect("valid service config")
}

fn pcm(
    source: AudioSource,
    generation: u64,
    sample_rate_hz: u32,
    frames: usize,
    timestamp_nanos: u64,
) -> PcmBuffer {
    let format = PcmFormat::new(sample_rate_hz, 1).expect("valid format");
    PcmBuffer::new(PcmBufferSpec {
        format,
        timestamp: AudioTimestamp::new(timestamp_nanos, Some(1_000)),
        provenance: AudioProvenance::new(source, AudioBackendKind::Test, generation, None)
            .expect("valid provenance"),
        samples: (0..frames).map(|sample| sample as i16).collect(),
    })
    .expect("valid PCM")
}

#[test]
fn service_segments_at_exact_frame_boundaries_and_flushes_final_chunk() {
    let backend = Arc::new(FakeBackend::default());
    let sink = Arc::new(CollectSink::default());
    let service = service(
        Arc::clone(&backend),
        Arc::clone(&sink) as Arc<dyn AudioSink>,
        AudioServiceConfig {
            queue_depth: 8,
            segment_duration_millis: 100,
        },
    );
    let started = service.start(AudioSource::Microphone).expect("start");
    for (index, frames) in [400, 400, 400, 400, 400].into_iter().enumerate() {
        assert_eq!(
            backend.emit(pcm(
                AudioSource::Microphone,
                started.generation,
                8_000,
                frames,
                10_000 + index as u64 * 50_000_000,
            )),
            CallbackDisposition::Accepted
        );
    }
    let stopped = service.stop().expect("stop");
    assert_eq!(stopped.state, mega_audio_capture::AudioState::Stopped);
    assert_eq!(stopped.metrics.emitted_segments, 3);

    let segments = sink.segments.lock().expect("sink lock");
    assert_eq!(segments.len(), 3);
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.frame_count)
            .collect::<Vec<_>>(),
        [800, 800, 400]
    );
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.sequence)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(segments[0].timestamp.monotonic_nanos, 10_000);
    assert_eq!(segments[1].timestamp.monotonic_nanos, 100_010_000);
    assert_eq!(segments[2].timestamp.monotonic_nanos, 200_010_000);
    assert!(!segments[0].final_segment);
    assert!(!segments[1].final_segment);
    assert!(segments[2].final_segment);
}

#[test]
fn service_is_explicitly_lifecycle_bound_and_tears_down_session() {
    let backend = Arc::new(FakeBackend::default());
    let sink = Arc::new(CollectSink::default());
    let service = service(Arc::clone(&backend), sink, AudioServiceConfig::default());
    assert_eq!(
        service.status().state,
        mega_audio_capture::AudioState::Stopped
    );
    let started = service.start(AudioSource::Microphone).expect("start");
    assert_eq!(
        service.start(AudioSource::Microphone),
        Err(AudioError::AlreadyActive {
            state: mega_audio_capture::AudioState::Running,
        })
    );
    assert!(service.stop().is_ok());
    assert_eq!(backend.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.stop().expect("idempotent stop").state,
        mega_audio_capture::AudioState::Stopped
    );
    assert_eq!(
        backend.emit(pcm(
            AudioSource::Microphone,
            started.generation,
            8_000,
            1,
            1
        )),
        CallbackDisposition::DroppedNotRunning
    );
    assert_eq!(service.status().metrics.dropped_not_running, 1);
}

#[test]
fn queue_is_bounded_and_accounts_for_backpressure_without_blocking_callback() {
    let backend = Arc::new(FakeBackend::default());
    let sink = Arc::new(BlockingSink::new());
    let service = service(
        Arc::clone(&backend),
        Arc::clone(&sink) as Arc<dyn AudioSink>,
        AudioServiceConfig {
            queue_depth: 1,
            segment_duration_millis: 100,
        },
    );
    let started = service.start(AudioSource::Microphone).expect("start");
    assert_eq!(
        backend.emit(pcm(
            AudioSource::Microphone,
            started.generation,
            8_000,
            800,
            0
        )),
        CallbackDisposition::Accepted
    );
    sink.wait_until_entered();
    assert_eq!(
        backend.emit(pcm(
            AudioSource::Microphone,
            started.generation,
            8_000,
            800,
            100_000_000
        )),
        CallbackDisposition::Accepted
    );
    assert_eq!(
        backend.emit(pcm(
            AudioSource::Microphone,
            started.generation,
            8_000,
            800,
            200_000_000
        )),
        CallbackDisposition::DroppedBackpressure
    );
    sink.release();
    let stopped = service.stop().expect("stop");
    assert_eq!(stopped.metrics.dropped_backpressure, 1);
    assert_eq!(stopped.metrics.accepted_buffers, 2);
}

#[test]
fn source_capability_and_pcm_bounds_fail_closed() {
    let backend = Arc::new(FakeBackend::default());
    let service = service(
        Arc::clone(&backend),
        Arc::new(CollectSink::default()),
        AudioServiceConfig::default(),
    );
    assert_eq!(
        service.start(AudioSource::SystemAudio),
        Err(AudioError::UnsupportedSource {
            audio_source: AudioSource::SystemAudio,
            detail: "the backend does not advertise this source",
        })
    );
    assert!(matches!(
        PcmFormat::new(7_999, 1),
        Err(AudioError::InvalidPcm { .. })
    ));
    assert!(matches!(
        PcmBuffer::new(PcmBufferSpec {
            format: PcmFormat::new(8_000, 2).expect("format"),
            timestamp: AudioTimestamp::new(1, None),
            provenance: AudioProvenance::new(
                AudioSource::Microphone,
                AudioBackendKind::Test,
                1,
                None
            )
            .expect("provenance"),
            samples: vec![0],
        }),
        Err(AudioError::InvalidPcm { .. })
    ));
}

#[test]
fn native_backend_is_not_faked_for_system_audio() {
    #[cfg(target_os = "macos")]
    {
        let backend = mega_audio_capture::MacOsAudioBackend::new();
        assert!(backend.capabilities().microphone);
        assert!(!backend.capabilities().system_audio);
    }
}
