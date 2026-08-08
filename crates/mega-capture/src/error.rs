use mega_permissions::PermissionState;
use serde::Serialize;
use thiserror::Error;

use crate::{CaptureState, FrameStatus};

#[derive(Debug, Error, Serialize)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum CaptureError {
    #[error("screen capture is unsupported on this target")]
    UnsupportedTarget,
    #[error("screen recording permission preflight failed: {message}")]
    PermissionPreflight { message: String },
    #[error("screen recording permission is not granted ({observed:?})")]
    PermissionNotGranted { observed: PermissionState },
    #[error("capture is already active in state {state:?}")]
    AlreadyActive { state: CaptureState },
    #[error("capture cannot start from state {state:?}")]
    InvalidStartState { state: CaptureState },
    #[error("capture stop failed because the current state is {state:?}")]
    InvalidStopState { state: CaptureState },
    #[error("no shareable display is available")]
    NoDisplays,
    #[error("display {display_id} is not available")]
    DisplayNotFound { display_id: u32 },
    #[error("could not start capture for {capture_source}: {message}")]
    StreamStart {
        capture_source: String,
        message: String,
    },
    #[error("could not stop capture for {capture_source}: {message}")]
    StreamStop {
        capture_source: String,
        message: String,
    },
    #[error("output handler registration failed for {capture_source}: {message}")]
    OutputHandlerRegistration {
        capture_source: String,
        message: String,
    },
    #[error("frame {status:?} was rejected: {reason}")]
    InvalidFrame { status: FrameStatus, reason: String },
    #[error("stream for {capture_source} stopped with error: {message}")]
    StreamStopped {
        capture_source: String,
        message: String,
    },
}
