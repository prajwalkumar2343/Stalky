use serde::{Deserialize, Serialize};

/// Stable identifier used to correlate events across local runtime boundaries.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CorrelationId(u128);

impl CorrelationId {
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u128 {
        self.0
    }
}

pub type SequenceNumber = u64;
