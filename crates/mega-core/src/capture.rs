use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    #[default]
    Paused,
    Preview,
    Context,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    Display { id: u64 },
    Window { id: u64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStopReason {
    ManualPause,
    PermissionLost,
    Sleep,
    ScreenLocked,
    FatalStreamError,
    Shutdown,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    #[default]
    Stopped,
    AwaitingPermission,
    Starting,
    Running {
        source: CaptureSource,
        mode: CaptureMode,
    },
    Paused {
        reason: CaptureStopReason,
    },
    Degraded {
        reason: String,
    },
    Stopping,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureHealth {
    pub state: CaptureState,
    pub queue_depth: u16,
    pub queue_capacity: u16,
    pub dropped_frames: u64,
    pub sampled_fps_milli: u16,
}

impl CaptureHealth {
    pub fn with_capacity(queue_capacity: u16) -> Self {
        Self {
            queue_capacity,
            ..Self::default()
        }
    }

    pub fn record_drop(&mut self) {
        self.dropped_frames = self.dropped_frames.saturating_add(1);
    }

    pub fn set_queue_depth(&mut self, queue_depth: u16) {
        self.queue_depth = queue_depth.min(self.queue_capacity);
    }
}
