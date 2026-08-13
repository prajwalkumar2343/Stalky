use std::fmt;
use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::AudioError;

pub const MIN_SEGMENT_DURATION_MILLIS: u32 = 100;
pub const MAX_SEGMENT_DURATION_MILLIS: u32 = 10_000;
pub const MAX_INPUT_FRAMES: usize = 16_384;
pub const MAX_DEVICE_LABEL_CHARS: usize = 128;
const MIN_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_SAMPLE_RATE_HZ: u32 = 96_000;
const MAX_CHANNELS: u16 = 2;

/// A capture source is selected explicitly for every service run. Combining
/// sources is deliberately left to a future mixer with its own provenance
/// contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSource {
    Microphone,
    SystemAudio,
}

/// Identifies how a sample entered the service. The sink can use this to
/// preserve provenance without inspecting audio content.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBackendKind {
    NativeMacOs,
    External,
    Test,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioProvenance {
    pub source: AudioSource,
    pub backend: AudioBackendKind,
    pub generation: u64,
    pub device_label: Option<String>,
}

impl AudioProvenance {
    pub fn new(
        source: AudioSource,
        backend: AudioBackendKind,
        generation: u64,
        device_label: Option<String>,
    ) -> Result<Self, AudioError> {
        if generation == 0 {
            return Err(AudioError::InvalidProvenance {
                detail: "stream generation must be non-zero",
            });
        }
        if device_label
            .as_deref()
            .is_some_and(|label| label.chars().count() > MAX_DEVICE_LABEL_CHARS)
        {
            return Err(AudioError::InvalidProvenance {
                detail: "device label exceeds the configured bound",
            });
        }
        Ok(Self {
            source,
            backend,
            generation,
            device_label,
        })
    }
}

/// Monotonic time is authoritative for ordering and duration. Wall-clock
/// time is optional because native audio callbacks cannot always provide it
/// without an additional clock read.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AudioTimestamp {
    pub monotonic_nanos: u64,
    pub unix_millis: Option<i64>,
}

impl AudioTimestamp {
    pub fn new(monotonic_nanos: u64, unix_millis: Option<i64>) -> Self {
        Self {
            monotonic_nanos,
            unix_millis,
        }
    }

    pub fn now() -> Self {
        static MONOTONIC_ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
        let monotonic_nanos = MONOTONIC_ORIGIN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        let unix_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok());
        Self::new(monotonic_nanos, unix_millis)
    }

    pub(crate) fn offset_frames(self, frames: usize, sample_rate_hz: u32) -> Self {
        let offset_nanos = (frames as u128)
            .saturating_mul(1_000_000_000)
            .checked_div(sample_rate_hz as u128)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(u64::MAX);
        let unix_offset = (frames as i128)
            .saturating_mul(1_000)
            .checked_div(sample_rate_hz as i128)
            .and_then(|value| i64::try_from(value).ok());
        Self {
            monotonic_nanos: self.monotonic_nanos.saturating_add(offset_nanos),
            unix_millis: self
                .unix_millis
                .zip(unix_offset)
                .map(|(base, offset)| base.saturating_add(offset)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PcmSampleFormat {
    I16,
}

/// The only PCM shape admitted by the portable ingestion boundary: bounded,
/// interleaved, signed 16-bit samples in native-endian Rust representation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PcmFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_format: PcmSampleFormat,
}

impl PcmFormat {
    pub fn new(sample_rate_hz: u32, channels: u16) -> Result<Self, AudioError> {
        if !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate_hz) {
            return Err(AudioError::InvalidPcm {
                detail: "sample rate must be between 8 kHz and 96 kHz",
            });
        }
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(AudioError::InvalidPcm {
                detail: "channel count must be one or two",
            });
        }
        Ok(Self {
            sample_rate_hz,
            channels,
            sample_format: PcmSampleFormat::I16,
        })
    }

    pub(crate) fn segment_frames(self, duration_millis: u32) -> usize {
        ((self.sample_rate_hz as u64)
            .saturating_mul(duration_millis as u64)
            .saturating_add(999)
            / 1_000) as usize
    }
}

/// Input to [`PcmBuffer::new`]. Keeping construction behind a validated
/// specification prevents malformed sample lengths from entering the service.
pub struct PcmBufferSpec {
    pub format: PcmFormat,
    pub timestamp: AudioTimestamp,
    pub provenance: AudioProvenance,
    pub samples: Vec<i16>,
}

/// A validated, bounded PCM callback payload. It is intentionally in-memory
/// only and zeroizes samples when dropped by the service.
pub struct PcmBuffer {
    format: PcmFormat,
    timestamp: AudioTimestamp,
    provenance: AudioProvenance,
    samples: Vec<i16>,
}

impl PcmBuffer {
    pub fn new(spec: PcmBufferSpec) -> Result<Self, AudioError> {
        let PcmBufferSpec {
            format,
            timestamp,
            provenance,
            samples,
        } = spec;
        let frame_count = samples.len() / usize::from(format.channels);
        if samples.is_empty() {
            return Err(AudioError::InvalidPcm {
                detail: "buffer must contain at least one sample frame",
            });
        }
        if samples.len() % usize::from(format.channels) != 0 {
            return Err(AudioError::InvalidPcm {
                detail: "sample count must be divisible by channel count",
            });
        }
        if frame_count > MAX_INPUT_FRAMES {
            return Err(AudioError::InvalidPcm {
                detail: "buffer exceeds the maximum callback frame count",
            });
        }
        if provenance.generation == 0 {
            return Err(AudioError::InvalidProvenance {
                detail: "stream generation must be non-zero",
            });
        }
        Ok(Self {
            format,
            timestamp,
            provenance,
            samples,
        })
    }

    pub fn format(&self) -> PcmFormat {
        self.format
    }

    pub fn timestamp(&self) -> AudioTimestamp {
        self.timestamp
    }

    pub fn provenance(&self) -> &AudioProvenance {
        &self.provenance
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels)
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    pub(crate) fn into_parts(self) -> (PcmFormat, AudioTimestamp, AudioProvenance, Vec<i16>) {
        let this = ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped after its fields are moved out, and
        // each field is read exactly once from a valid initialized value.
        unsafe {
            (
                ptr::read(&this.format),
                ptr::read(&this.timestamp),
                ptr::read(&this.provenance),
                ptr::read(&this.samples),
            )
        }
    }
}

impl fmt::Debug for PcmBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PcmBuffer")
            .field("format", &self.format)
            .field("timestamp", &self.timestamp)
            .field("provenance", &self.provenance)
            .field("frame_count", &self.frame_count())
            .finish()
    }
}

impl Drop for PcmBuffer {
    fn drop(&mut self) {
        self.samples.zeroize();
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioSegmentMetadata {
    pub sequence: u64,
    pub timestamp: AudioTimestamp,
    pub duration_nanos: u64,
    pub frame_count: usize,
    pub format: PcmFormat,
    pub provenance: AudioProvenance,
    pub final_segment: bool,
}

/// A deterministic chunk handed to the future encrypted sink. The samples
/// exist only for the duration of sink processing and are zeroized if dropped.
pub struct AudioSegment {
    metadata: AudioSegmentMetadata,
    samples: Vec<i16>,
}

impl AudioSegment {
    pub(crate) fn new(
        sequence: u64,
        timestamp: AudioTimestamp,
        format: PcmFormat,
        provenance: AudioProvenance,
        samples: Vec<i16>,
        final_segment: bool,
    ) -> Self {
        let frame_count = samples.len() / usize::from(format.channels);
        let duration_nanos = (frame_count as u128)
            .saturating_mul(1_000_000_000)
            .checked_div(format.sample_rate_hz as u128)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(u64::MAX);
        Self {
            metadata: AudioSegmentMetadata {
                sequence,
                timestamp,
                duration_nanos,
                frame_count,
                format,
                provenance,
                final_segment,
            },
            samples,
        }
    }

    pub fn metadata(&self) -> &AudioSegmentMetadata {
        &self.metadata
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    pub fn into_parts(self) -> (AudioSegmentMetadata, Vec<i16>) {
        let this = ManuallyDrop::new(self);
        // SAFETY: `this` is not dropped after the fields are moved out.
        unsafe { (ptr::read(&this.metadata), ptr::read(&this.samples)) }
    }
}

impl fmt::Debug for AudioSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AudioSegment")
            .field("metadata", &self.metadata)
            .field("sample_count", &self.samples.len())
            .finish()
    }
}

impl Drop for AudioSegment {
    fn drop(&mut self) {
        self.samples.zeroize();
    }
}
