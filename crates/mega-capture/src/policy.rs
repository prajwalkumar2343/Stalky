use serde::{Deserialize, Serialize};

pub const DEFAULT_QUEUE_DEPTH: u32 = 3;
pub const DEFAULT_SAMPLE_INTERVAL_MILLIS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    PrimaryDisplay,
    Display { id: u32 },
}

impl CaptureSource {
    pub const fn display_id(self, primary_display_id: u32) -> u32 {
        match self {
            Self::PrimaryDisplay => primary_display_id,
            Self::Display { id } => id,
        }
    }
}

impl std::fmt::Display for CaptureSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrimaryDisplay => f.write_str("primary display"),
            Self::Display { id } => write!(f, "display {id}"),
        }
    }
}
