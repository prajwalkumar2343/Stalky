//! Bounded, opt-in audio ingestion for Stalky's encrypted history pipeline.
//!
//! This crate never opens a file, serializes plaintext audio, or owns durable
//! audio storage. It validates and segments bounded PCM buffers, then hands
//! each segment to an [`AudioSink`] supplied by the desktop/vault layer. Sink
//! implementations are responsible for encrypting before persistence.

mod backend;
mod error;
mod service;
mod types;

#[cfg(target_os = "macos")]
mod native;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub use backend::{
    AudioBackend, AudioInputCallback, AudioSession, BackendCapabilities, CallbackDisposition,
};
pub use error::AudioError;
pub use service::{
    AudioMetrics, AudioService, AudioServiceConfig, AudioSink, AudioState, AudioStatus, SinkError,
};
pub use types::{
    AudioBackendKind, AudioProvenance, AudioSegment, AudioSegmentMetadata, AudioSource,
    AudioTimestamp, MAX_DEVICE_LABEL_CHARS, MAX_INPUT_FRAMES, MAX_SEGMENT_DURATION_MILLIS,
    MIN_SEGMENT_DURATION_MILLIS, PcmBuffer, PcmBufferSpec, PcmFormat, PcmSampleFormat,
};

#[cfg(target_os = "macos")]
pub use native::MacOsAudioBackend;
#[cfg(not(target_os = "macos"))]
pub use unsupported::UnsupportedAudioBackend;

/// Returns the native backend for this target, or a truthful unsupported
/// backend on targets where no capture implementation is compiled.
pub fn default_backend() -> std::sync::Arc<dyn AudioBackend> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(MacOsAudioBackend::new())
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::sync::Arc::new(UnsupportedAudioBackend::new())
    }
}
