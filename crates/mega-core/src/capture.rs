use serde::{Deserialize, Serialize};

use crate::SequenceNumber;

pub const CAPTURE_EVENT_SCHEMA_VERSION: u16 = 1;
pub const MAX_CAPTURE_EVENT_TEXT_CHARS: usize = 12_000;

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

/// The source of a metadata-only capture event. Raw pixels never cross this
/// contract; frame events carry dimensions and a content identity instead.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureEventKind {
    AccessibilitySnapshot,
    AccessibilityChange,
    ScreenFrame,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePrivacyClass {
    Public,
    Private,
    Sensitive,
    Denied,
}

/// Stable source identity attached to every event produced by a capture
/// session. `stream_generation` separates a restarted stream from late
/// callbacks belonging to an older stream.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct CaptureSourceProvenance {
    pub source: CaptureSource,
    pub stable_source_id: u64,
    pub stream_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureEventEnvelope {
    pub schema_version: u16,
    pub sequence: SequenceNumber,
    pub observed_at_millis: u64,
    pub correlation_id: crate::CorrelationId,
    pub source: CaptureSourceProvenance,
    pub kind: CaptureEventKind,
    pub privacy: CapturePrivacyClass,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccessibilityCaptureEvent {
    pub envelope: CaptureEventEnvelope,
    pub bundle_identifier: String,
    pub ax_sequence: u64,
    pub redacted_text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrameCaptureEvent {
    pub envelope: CaptureEventEnvelope,
    pub width: usize,
    pub height: usize,
    pub byte_len: usize,
    pub content_digest: [u8; 32],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CaptureEvent {
    Accessibility(AccessibilityCaptureEvent),
    Frame(FrameCaptureEvent),
}

impl CaptureEvent {
    pub fn envelope(&self) -> &CaptureEventEnvelope {
        match self {
            Self::Accessibility(event) => &event.envelope,
            Self::Frame(event) => &event.envelope,
        }
    }

    /// Validate the boundary before an event is queued or persisted.
    pub fn validate(&self) -> Result<(), CaptureContractError> {
        let envelope = self.envelope();
        if envelope.privacy == CapturePrivacyClass::Denied {
            return Err(CaptureContractError::PrivacyDenied);
        }
        if envelope.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
            return Err(CaptureContractError::UnsupportedSchemaVersion {
                observed: envelope.schema_version,
            });
        }
        if envelope.sequence == 0 || envelope.source.stream_generation == 0 {
            return Err(CaptureContractError::MissingOrderingIdentity);
        }
        if envelope.source.stable_source_id == 0 {
            return Err(CaptureContractError::MissingSourceIdentity);
        }
        match self {
            Self::Accessibility(event) => {
                if event.envelope.kind != CaptureEventKind::AccessibilitySnapshot
                    && event.envelope.kind != CaptureEventKind::AccessibilityChange
                {
                    return Err(CaptureContractError::MismatchedEventKind);
                }
                if event.bundle_identifier.trim().is_empty()
                    || event.bundle_identifier.chars().count() > 256
                    || !event.bundle_identifier.is_ascii()
                    || event.bundle_identifier.split('.').any(str::is_empty)
                {
                    return Err(CaptureContractError::InvalidBundleIdentifier);
                }
                if event
                    .redacted_text
                    .as_ref()
                    .is_some_and(|text| text.chars().count() > MAX_CAPTURE_EVENT_TEXT_CHARS)
                {
                    return Err(CaptureContractError::TextTooLong);
                }
            }
            Self::Frame(event) => {
                if event.envelope.kind != CaptureEventKind::ScreenFrame
                    || event.width == 0
                    || event.height == 0
                    || event.byte_len == 0
                {
                    return Err(CaptureContractError::InvalidFrameMetadata);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CaptureContractError {
    #[error("unsupported capture event schema version {observed}")]
    UnsupportedSchemaVersion { observed: u16 },
    #[error("capture event is missing a sequence or stream generation")]
    MissingOrderingIdentity,
    #[error("capture event is missing a stable source identity")]
    MissingSourceIdentity,
    #[error("capture event kind does not match its payload")]
    MismatchedEventKind,
    #[error("capture event was denied by privacy policy")]
    PrivacyDenied,
    #[error("Accessibility event has an invalid bundle identifier")]
    InvalidBundleIdentifier,
    #[error("capture event text exceeds the configured bound")]
    TextTooLong,
    #[error("frame event has invalid metadata")]
    InvalidFrameMetadata,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureHealth {
    pub state: CaptureState,
    pub queue_depth: u16,
    pub queue_capacity: u16,
    pub dropped_frames: u64,
    pub sampled_fps_milli: u16,
    pub accepted_events: u64,
    pub dropped_events: u64,
    pub privacy_denials: u64,
    pub last_event_sequence: Option<SequenceNumber>,
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

    pub fn record_event(&mut self, sequence: SequenceNumber) {
        self.accepted_events = self.accepted_events.saturating_add(1);
        self.last_event_sequence = Some(sequence);
    }

    pub fn record_event_drop(&mut self) {
        self.dropped_events = self.dropped_events.saturating_add(1);
    }

    pub fn record_privacy_denial(&mut self) {
        self.privacy_denials = self.privacy_denials.saturating_add(1);
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::CorrelationId;

    fn envelope(kind: CaptureEventKind) -> CaptureEventEnvelope {
        CaptureEventEnvelope {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            sequence: 1,
            observed_at_millis: 10,
            correlation_id: CorrelationId::new(2),
            source: CaptureSourceProvenance {
                source: CaptureSource::Display { id: 7 },
                stable_source_id: 7,
                stream_generation: 1,
            },
            kind,
            privacy: CapturePrivacyClass::Private,
        }
    }

    #[test]
    fn accessibility_event_rejects_unbounded_text_before_queueing() {
        let event = CaptureEvent::Accessibility(AccessibilityCaptureEvent {
            envelope: envelope(CaptureEventKind::AccessibilitySnapshot),
            bundle_identifier: "com.example.editor".to_owned(),
            ax_sequence: 3,
            redacted_text: Some("x".repeat(MAX_CAPTURE_EVENT_TEXT_CHARS + 1)),
        });

        assert_eq!(event.validate(), Err(CaptureContractError::TextTooLong));
    }

    #[test]
    fn frame_event_requires_a_stable_source_and_ordering_identity() {
        let mut event = CaptureEvent::Frame(FrameCaptureEvent {
            envelope: envelope(CaptureEventKind::ScreenFrame),
            width: 10,
            height: 10,
            byte_len: 400,
            content_digest: [0; 32],
        });
        event.envelope_mut_for_test().source.stable_source_id = 0;

        assert_eq!(
            event.validate(),
            Err(CaptureContractError::MissingSourceIdentity)
        );
    }

    trait EnvelopeMutForTest {
        fn envelope_mut_for_test(&mut self) -> &mut CaptureEventEnvelope;
    }

    impl EnvelopeMutForTest for CaptureEvent {
        fn envelope_mut_for_test(&mut self) -> &mut CaptureEventEnvelope {
            match self {
                CaptureEvent::Accessibility(event) => &mut event.envelope,
                CaptureEvent::Frame(event) => &mut event.envelope,
            }
        }
    }
}
