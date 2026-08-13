use std::sync::Arc;

use crate::{AudioError, AudioSource, PcmBuffer};

/// Capabilities are descriptive only; a source can still fail at start time
/// because the OS permission or device state changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendCapabilities {
    pub microphone: bool,
    pub system_audio: bool,
}

impl BackendCapabilities {
    pub const fn none() -> Self {
        Self {
            microphone: false,
            system_audio: false,
        }
    }

    pub const fn supports(self, source: AudioSource) -> bool {
        match source {
            AudioSource::Microphone => self.microphone,
            AudioSource::SystemAudio => self.system_audio,
        }
    }
}

/// Result of a callback attempt. Native callbacks can use this without
/// blocking or waiting for the sink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackDisposition {
    Accepted,
    DroppedNotRunning,
    DroppedBackpressure,
    DroppedInvalid,
}

/// The backend invokes this object from its capture callback. Implementations
/// must return promptly; the service performs a non-blocking bounded enqueue.
pub trait AudioInputCallback: Send + Sync {
    fn push(&self, buffer: PcmBuffer) -> CallbackDisposition;
}

/// A backend owns the platform capture object and returns a session whose
/// consuming stop operation must release native resources before returning.
pub trait AudioBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    fn start(
        &self,
        source: AudioSource,
        generation: u64,
        callback: Arc<dyn AudioInputCallback>,
    ) -> Result<Box<dyn AudioSession>, AudioError>;
}

pub trait AudioSession: Send {
    fn stop(self: Box<Self>) -> Result<(), AudioError>;
}
