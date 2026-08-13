//! Bounded, opt-in ScreenCaptureKit capture for Stalky.
//!
//! The policy, frame validation, and state machine in this crate are portable.
//! Apple framework access is confined to the macOS adapter and is never
//! constructed or started by [`CaptureService::new`].

mod error;
#[cfg_attr(
    not(any(target_os = "macos", test)),
    allow(dead_code, reason = "frame ingest is exercised by the macOS adapter")
)]
mod frame;
mod policy;
#[cfg_attr(
    not(any(target_os = "macos", test)),
    allow(dead_code, reason = "capture callbacks are used by the macOS adapter")
)]
mod service;

#[cfg(target_os = "macos")]
mod native;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub use error::CaptureError;
pub use frame::{
    BgraFrame, FrameAdmission, FrameDigest, FrameIdentity, FrameIngest, FrameInput, FrameMetadata,
    FrameMetrics, FrameProvenance, FrameRecord, FrameStatus, MAX_FRAME_BYTES, MAX_FRAME_HEIGHT,
    MAX_FRAME_WIDTH,
};
pub use policy::{
    CaptureDecision, CapturePolicy, CaptureSkipReason, CaptureSource, DEFAULT_QUEUE_DEPTH,
    DEFAULT_SAMPLE_INTERVAL_MILLIS, FrameCandidate, FrameObservation, PrivacyDecision,
    PrivacyDenyReason, PrivacyPolicy,
};
pub use service::{CaptureService, CaptureState, CaptureStatus};

#[cfg(target_os = "macos")]
pub(crate) fn platform_backend() -> native::NativeBackend {
    native::NativeBackend
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn platform_backend() -> unsupported::UnsupportedBackend {
    unsupported::UnsupportedBackend
}
