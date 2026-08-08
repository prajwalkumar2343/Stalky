//! Versioned, content-bounded contracts for the local UI event boundary.

use mega_core::{
    AudioHealth, CaptureState, CorrelationId, LifecycleState, SequenceNumber, Subsystem,
    SubsystemHealth,
};
use mega_permissions::PermissionEvent;
pub use mega_permissions::{PermissionCapability, PermissionOperation, PermissionState};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const EVENT_SCHEMA_VERSION: u16 = 1;
pub const PERMISSION_SCHEMA_VERSION: u16 = 1;
pub const PERMISSIONS_CHANGED_EVENT: &str = "permissions_changed";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "code", content = "details", rename_all = "snake_case")]
pub enum PermissionError {
    Unsupported {
        capability: PermissionCapability,
    },
    Busy {
        capability: PermissionCapability,
    },
    ProbeFailed {
        capability: PermissionCapability,
        message: String,
    },
    RequestFailed {
        capability: PermissionCapability,
        message: String,
    },
    SettingsFailed {
        capability: PermissionCapability,
        message: String,
    },
    InvalidTransition {
        capability: PermissionCapability,
        message: String,
    },
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { capability } => write!(formatter, "{capability:?} is unsupported"),
            Self::Busy { capability } => {
                write!(formatter, "{capability:?} is already being checked")
            }
            Self::ProbeFailed { message, .. }
            | Self::RequestFailed { message, .. }
            | Self::SettingsFailed { message, .. }
            | Self::InvalidTransition { message, .. } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PermissionError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    pub capability: PermissionCapability,
    /// Effective state for presentation. Requesting and Rechecking are
    /// transient and never replace `authorization`.
    pub state: PermissionState,
    /// Last trustworthy authorization observation.
    pub authorization: PermissionState,
    pub operation: PermissionOperation,
    pub last_error: Option<PermissionError>,
    pub can_request: bool,
    pub can_open_settings: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSnapshot {
    pub schema_version: u16,
    pub sequence: u64,
    pub statuses: Vec<PermissionStatus>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionClass {
    MetadataOnly,
    UserContent,
    Sensitive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InfrastructureEvent {
    LifecycleChanged {
        state: LifecycleState,
    },
    PermissionChanged(PermissionEvent),
    PermissionsChanged {
        snapshot: PermissionSnapshot,
    },
    CaptureChanged {
        state: CaptureState,
    },
    AudioHealthChanged {
        health: AudioHealth,
    },
    SubsystemHealthChanged {
        subsystem: Subsystem,
        health: SubsystemHealth,
    },
}

impl InfrastructureEvent {
    pub const fn subsystem(&self) -> Subsystem {
        match self {
            Self::LifecycleChanged { .. } => Subsystem::Runtime,
            Self::PermissionChanged(_) => Subsystem::Permissions,
            Self::PermissionsChanged { .. } => Subsystem::Permissions,
            Self::CaptureChanged { .. } => Subsystem::ScreenCapture,
            Self::AudioHealthChanged { .. } => Subsystem::Audio,
            Self::SubsystemHealthChanged { subsystem, .. } => *subsystem,
        }
    }

    pub const fn redaction(&self) -> RedactionClass {
        match self {
            Self::LifecycleChanged { .. }
            | Self::PermissionChanged(_)
            | Self::PermissionsChanged { .. }
            | Self::CaptureChanged { .. }
            | Self::AudioHealthChanged { .. }
            | Self::SubsystemHealthChanged { .. } => RedactionClass::MetadataOnly,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventEnvelope<T> {
    pub schema_version: u16,
    pub monotonic_millis: u64,
    pub wall_time_unix_millis: Option<i64>,
    pub subsystem: Subsystem,
    pub correlation_id: CorrelationId,
    pub sequence: SequenceNumber,
    pub redaction: RedactionClass,
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    pub fn new(
        monotonic_millis: u64,
        wall_time_unix_millis: Option<i64>,
        correlation_id: CorrelationId,
        sequence: SequenceNumber,
        payload: T,
    ) -> Self
    where
        T: RedactableEvent,
    {
        Self {
            schema_version: EVENT_SCHEMA_VERSION,
            monotonic_millis,
            wall_time_unix_millis,
            subsystem: payload.subsystem(),
            correlation_id,
            sequence,
            redaction: payload.redaction(),
            payload,
        }
    }

    pub fn with_schema_version(mut self, schema_version: u16) -> Self {
        self.schema_version = schema_version;
        self
    }
}

impl<T> EventEnvelope<T>
where
    T: Serialize,
{
    pub fn to_json(&self) -> Result<String, EnvelopeError> {
        serde_json::to_string(self).map_err(EnvelopeError::Serialize)
    }
}

impl<T> EventEnvelope<T>
where
    T: DeserializeOwned,
{
    pub fn from_json(json: &str) -> Result<Self, EnvelopeError> {
        let envelope: Self = serde_json::from_str(json).map_err(EnvelopeError::Deserialize)?;
        if envelope.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EnvelopeError::UnsupportedSchema {
                received: envelope.schema_version,
                supported: EVENT_SCHEMA_VERSION,
            });
        }
        Ok(envelope)
    }
}

pub trait RedactableEvent {
    fn subsystem(&self) -> Subsystem;
    fn redaction(&self) -> RedactionClass;
}

impl RedactableEvent for InfrastructureEvent {
    fn subsystem(&self) -> Subsystem {
        self.subsystem()
    }

    fn redaction(&self) -> RedactionClass {
        self.redaction()
    }
}

#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("could not serialize event envelope: {0}")]
    Serialize(serde_json::Error),
    #[error("could not deserialize event envelope: {0}")]
    Deserialize(serde_json::Error),
    #[error("unsupported event schema {received}; supported schema is {supported}")]
    UnsupportedSchema { received: u16, supported: u16 },
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_SCHEMA_VERSION, EnvelopeError, EventEnvelope, InfrastructureEvent,
        PERMISSION_SCHEMA_VERSION, PermissionCapability, PermissionError, PermissionOperation,
        PermissionSnapshot, PermissionStatus, RedactionClass,
    };
    use mega_core::{CorrelationId, LifecycleState, Subsystem};

    #[test]
    fn envelope_round_trips_with_explicit_metadata() {
        let event = InfrastructureEvent::LifecycleChanged {
            state: LifecycleState::Running,
        };
        let envelope =
            EventEnvelope::new(42, Some(1_735_000_000_000), CorrelationId::new(7), 9, event);

        let json = envelope.to_json().unwrap();
        let decoded = EventEnvelope::<InfrastructureEvent>::from_json(&json).unwrap();

        assert_eq!(decoded, envelope);
        assert_eq!(decoded.schema_version, EVENT_SCHEMA_VERSION);
        assert_eq!(decoded.subsystem, Subsystem::Runtime);
        assert_eq!(decoded.redaction, RedactionClass::MetadataOnly);
    }

    #[test]
    fn unknown_schema_is_rejected_before_consumption() {
        let event = InfrastructureEvent::LifecycleChanged {
            state: LifecycleState::Stopped,
        };
        let envelope = EventEnvelope::new(0, None, CorrelationId::default(), 0, event)
            .with_schema_version(EVENT_SCHEMA_VERSION + 1);

        let error = EventEnvelope::<InfrastructureEvent>::from_json(&envelope.to_json().unwrap())
            .unwrap_err();
        assert!(matches!(error, EnvelopeError::UnsupportedSchema { .. }));
    }

    #[test]
    fn permission_snapshot_round_trips_typed_state_and_error() {
        let snapshot = PermissionSnapshot {
            schema_version: PERMISSION_SCHEMA_VERSION,
            sequence: 4,
            statuses: vec![PermissionStatus {
                capability: PermissionCapability::Microphone,
                state: super::PermissionState::Denied,
                authorization: super::PermissionState::Denied,
                operation: PermissionOperation::Idle,
                last_error: Some(PermissionError::RequestFailed {
                    capability: PermissionCapability::Microphone,
                    message: "user declined".to_owned(),
                }),
                can_request: true,
                can_open_settings: true,
            }],
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: PermissionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn permission_event_is_scoped_to_permissions() {
        let event = InfrastructureEvent::PermissionsChanged {
            snapshot: PermissionSnapshot {
                schema_version: PERMISSION_SCHEMA_VERSION,
                sequence: 0,
                statuses: Vec::new(),
            },
        };
        assert_eq!(event.subsystem(), Subsystem::Permissions);
        assert_eq!(event.redaction(), RedactionClass::MetadataOnly);
    }
}
