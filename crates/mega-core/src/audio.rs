use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioState {
    #[default]
    Stopped,
    AwaitingPermission,
    Starting,
    Ready,
    Testing,
    Degraded {
        reason: String,
    },
    Stopping,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioStatus {
    #[default]
    Unknown,
    Healthy,
    NoDevice,
    PermissionDenied,
    Overrun,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioHealth {
    pub state: AudioState,
    pub status: AudioStatus,
    pub buffer_occupancy: u32,
    pub buffer_capacity: u32,
    pub overrun_count: u64,
    pub voice_activity: bool,
}

impl AudioHealth {
    pub fn with_capacity(buffer_capacity: u32) -> Self {
        Self {
            buffer_capacity,
            ..Self::default()
        }
    }

    pub fn record_overrun(&mut self) {
        self.overrun_count = self.overrun_count.saturating_add(1);
        self.status = AudioStatus::Overrun;
    }

    pub fn set_buffer_occupancy(&mut self, buffer_occupancy: u32) {
        self.buffer_occupancy = buffer_occupancy.min(self.buffer_capacity);
    }
}
