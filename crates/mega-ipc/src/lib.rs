//! Versioned, content-bounded contracts for the local UI event boundary.

use mega_core::{
    AudioHealth, CaptureState, CorrelationId, LifecycleState, SequenceNumber, Subsystem,
    SubsystemHealth,
};
use mega_permissions::PermissionEvent;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const EVENT_SCHEMA_VERSION: u16 = 1;

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
            Self::CaptureChanged { .. } => Subsystem::ScreenCapture,
            Self::AudioHealthChanged { .. } => Subsystem::Audio,
            Self::SubsystemHealthChanged { subsystem, .. } => *subsystem,
        }
    }

    pub const fn redaction(&self) -> RedactionClass {
        match self {
            Self::LifecycleChanged { .. }
            | Self::PermissionChanged(_)
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
        EVENT_SCHEMA_VERSION, EnvelopeError, EventEnvelope, InfrastructureEvent, RedactionClass,
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
}
