use std::sync::Arc;

use crate::{
    AudioBackend, AudioError, AudioInputCallback, AudioSession, AudioSource, BackendCapabilities,
};

/// Explicitly unsupported backend used on non-macOS targets. It never claims
/// to capture from a fake device and is useful for callers that want to show a
/// truthful capability state.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedAudioBackend;

impl UnsupportedAudioBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl AudioBackend for UnsupportedAudioBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::none()
    }

    fn start(
        &self,
        _source: AudioSource,
        _generation: u64,
        _callback: Arc<dyn AudioInputCallback>,
    ) -> Result<Box<dyn AudioSession>, AudioError> {
        Err(AudioError::UnsupportedTarget)
    }
}
