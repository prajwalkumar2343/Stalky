use thiserror::Error;

use crate::{AudioSource, AudioState, PcmFormat};

/// Errors at the ingestion boundary. Plaintext samples are never included in
/// an error so these values are safe to surface to diagnostics.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AudioError {
    #[error("audio capture is unsupported on this target")]
    UnsupportedTarget,
    #[error("audio source {audio_source:?} is unsupported by the selected backend: {detail}")]
    UnsupportedSource {
        audio_source: AudioSource,
        detail: &'static str,
    },
    #[error("microphone permission is required before starting audio capture")]
    PermissionRequired,
    #[error("audio capture is already active in state {state:?}")]
    AlreadyActive { state: AudioState },
    #[error("audio capture cannot start from state {state:?}")]
    InvalidStartState { state: AudioState },
    #[error("audio capture cannot stop from state {state:?}")]
    InvalidStopState { state: AudioState },
    #[error("invalid audio service configuration: {detail}")]
    InvalidConfig { detail: &'static str },
    #[error("invalid PCM buffer: {detail}")]
    InvalidPcm { detail: &'static str },
    #[error("invalid audio provenance: {detail}")]
    InvalidProvenance { detail: &'static str },
    #[error("PCM format changed during an active stream from {expected:?} to {observed:?}")]
    FormatChanged {
        expected: PcmFormat,
        observed: PcmFormat,
    },
    #[error(
        "PCM buffer provenance source {observed:?} does not match selected source {expected:?}"
    )]
    ProvenanceSourceMismatch {
        expected: AudioSource,
        observed: AudioSource,
    },
    #[error("native audio backend failed to start: {message}")]
    BackendStart { message: String },
    #[error("native audio backend failed to stop: {message}")]
    BackendStop { message: String },
    #[error("audio worker failed to shut down cleanly")]
    WorkerJoin,
    #[error("audio sink failed: {message}")]
    SinkFailed { message: String },
}
