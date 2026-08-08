use serde::{Deserialize, Serialize};

use crate::LifecycleState;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Subsystem {
    Runtime,
    ScreenCapture,
    Accessibility,
    Audio,
    Permissions,
    Ipc,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubsystemHealth {
    pub lifecycle: LifecycleState,
    pub health: HealthStatus,
    pub restart_count: u32,
    pub detail: Option<String>,
}

impl Default for SubsystemHealth {
    fn default() -> Self {
        Self {
            lifecycle: LifecycleState::Stopped,
            health: HealthStatus::Unknown,
            restart_count: 0,
            detail: None,
        }
    }
}

impl SubsystemHealth {
    pub fn stopped() -> Self {
        Self::default()
    }

    pub fn running() -> Self {
        Self {
            lifecycle: LifecycleState::Running,
            health: HealthStatus::Healthy,
            ..Self::default()
        }
    }

    pub fn degraded(detail: impl Into<String>) -> Self {
        Self {
            lifecycle: LifecycleState::Degraded {
                reason: detail.into(),
            },
            health: HealthStatus::Degraded,
            ..Self::default()
        }
    }
}
