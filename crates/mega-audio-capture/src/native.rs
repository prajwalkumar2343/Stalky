//! macOS AVFAudio microphone adapter.
//!
//! System audio is intentionally not advertised here. ScreenCaptureKit's
//! audio-output path needs a separate entitlement/filter/lifetime contract;
//! returning an explicit unsupported capability is safer than silently
//! treating system audio as microphone input.

use std::ffi::c_float;
use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioEngine, AVAudioInputNode,
    AVAudioPCMBuffer, AVAudioTime,
};

use crate::AudioError;
use crate::backend::{AudioBackend, AudioInputCallback, AudioSession, BackendCapabilities};
use crate::types::{
    AudioBackendKind, AudioProvenance, AudioSource, AudioTimestamp, MAX_INPUT_FRAMES, PcmBuffer,
    PcmBufferSpec, PcmFormat,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct MacOsAudioBackend;

impl MacOsAudioBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl AudioBackend for MacOsAudioBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            microphone: true,
            system_audio: false,
        }
    }

    fn start(
        &self,
        source: AudioSource,
        generation: u64,
        callback: Arc<dyn AudioInputCallback>,
    ) -> Result<Box<dyn AudioSession>, AudioError> {
        if source != AudioSource::Microphone {
            return Err(AudioError::UnsupportedSource {
                audio_source: source,
                detail: "ScreenCaptureKit system-audio ingestion is not implemented",
            });
        }
        let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
        if permission != AVAudioApplicationRecordPermission::Granted {
            return Err(AudioError::PermissionRequired);
        }

        let engine = unsafe { AVAudioEngine::new() };
        let input = unsafe { engine.inputNode() };
        let output_format = unsafe { input.outputFormatForBus(0) };
        let sample_rate_hz = unsafe { output_format.sampleRate() }.round() as u32;
        let channels = u16::try_from(unsafe { output_format.channelCount() }).map_err(|_| {
            AudioError::BackendStart {
                message: "microphone reported too many channels".to_owned(),
            }
        })?;
        let format =
            PcmFormat::new(sample_rate_hz, channels).map_err(|error| AudioError::BackendStart {
                message: error.to_string(),
            })?;
        let provenance =
            AudioProvenance::new(source, AudioBackendKind::NativeMacOs, generation, None)?;
        let callback_provenance = provenance.clone();
        let callback_format = format;
        let tap: RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)> = RcBlock::new(
            move |buffer_ptr: NonNull<AVAudioPCMBuffer>, _time_ptr: NonNull<AVAudioTime>| {
                // AVAudioEngine owns the buffer until this callback returns.
                // Copy only the bounded frame window needed by the service;
                // no native pointer escapes the callback.
                let buffer = unsafe { buffer_ptr.as_ref() };
                let frame_count = unsafe { buffer.frameLength() } as usize;
                if frame_count == 0 || frame_count > MAX_INPUT_FRAMES {
                    return;
                }
                let Some(samples) = copy_pcm_samples(buffer, frame_count, callback_format) else {
                    return;
                };
                let Ok(buffer) = PcmBuffer::new(PcmBufferSpec {
                    format: callback_format,
                    timestamp: AudioTimestamp::now(),
                    provenance: callback_provenance.clone(),
                    samples,
                }) else {
                    return;
                };
                let _ = callback.push(buffer);
            },
        );

        // Request a device-rate-derived 100–200 ms tap. This avoids a fixed
        // frame count becoming an unexpectedly long callback on low-rate
        // devices, while MAX_INPUT_FRAMES still bounds the copied payload.
        let tap_buffer_frames = sample_rate_hz
            .saturating_mul(200)
            .saturating_div(1_000)
            .clamp(800, MAX_INPUT_FRAMES as u32);
        unsafe {
            input.installTapOnBus_bufferSize_format_block(
                0,
                tap_buffer_frames,
                None,
                RcBlock::as_ptr(&tap),
            );
            engine.prepare();
            if let Err(error) = engine.startAndReturnError() {
                input.removeTapOnBus(0);
                return Err(AudioError::BackendStart {
                    message: error.localizedDescription().to_string(),
                });
            }
        }
        Ok(Box::new(MacOsAudioSession { engine, input }))
    }
}

struct MacOsAudioSession {
    engine: Retained<AVAudioEngine>,
    input: Retained<AVAudioInputNode>,
}

// AVAudioEngine and its input node are created, started, and stopped by the
// service lifecycle thread. The tap only owns the callback Arc and never
// dereferences either object, so moving this session to the stop thread does
// not move native objects across concurrent access.
unsafe impl Send for MacOsAudioSession {}

impl AudioSession for MacOsAudioSession {
    fn stop(self: Box<Self>) -> Result<(), AudioError> {
        unsafe {
            self.input.removeTapOnBus(0);
            self.engine.stop();
        }
        Ok(())
    }
}

fn copy_pcm_samples(
    buffer: &AVAudioPCMBuffer,
    frame_count: usize,
    format: PcmFormat,
) -> Option<Vec<i16>> {
    let channels = usize::from(format.channels);
    let stride = unsafe { buffer.stride() };
    if stride == 0 {
        return None;
    }
    let mut samples = Vec::with_capacity(frame_count.checked_mul(channels)?);
    let float_channels = unsafe { buffer.floatChannelData() };
    if !float_channels.is_null() {
        for frame in 0..frame_count {
            for channel in 0..channels {
                // SAFETY: AVAudioPCMBuffer guarantees channel pointers and
                // frameLength samples while the callback owns the borrowed
                // buffer; bounds were checked above.
                let channel_ptr = unsafe { *float_channels.add(channel) };
                let sample = unsafe { *channel_ptr.as_ptr().add(frame * stride) };
                samples.push(float_to_i16(sample));
            }
        }
        return Some(samples);
    }

    let int16_channels = unsafe { buffer.int16ChannelData() };
    if int16_channels.is_null() {
        return None;
    }
    for frame in 0..frame_count {
        for channel in 0..channels {
            // SAFETY: Same lifetime and bounds guarantees as the float path.
            let channel_ptr = unsafe { *int16_channels.add(channel) };
            let sample = unsafe { *channel_ptr.as_ptr().add(frame * stride) };
            samples.push(sample);
        }
    }
    Some(samples)
}

fn float_to_i16(sample: c_float) -> i16 {
    if !sample.is_finite() {
        return 0;
    }
    (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16
}
