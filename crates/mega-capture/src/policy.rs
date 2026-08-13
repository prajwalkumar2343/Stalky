use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const DEFAULT_QUEUE_DEPTH: u32 = 3;
pub const DEFAULT_SAMPLE_INTERVAL_MILLIS: u64 = 1_000;
pub const MAX_BUNDLE_IDENTIFIER_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    PrimaryDisplay,
    Display { id: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameCandidate {
    pub source: CaptureSource,
    pub timestamp_millis: u64,
    pub content_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameObservation {
    pub source: CaptureSource,
    pub timestamp_millis: u64,
    pub content_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturePolicy {
    pub enabled: bool,
    pub minimum_interval_millis: u64,
    pub deduplicate: bool,
    pub privacy: PrivacyPolicy,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_interval_millis: DEFAULT_SAMPLE_INTERVAL_MILLIS,
            deduplicate: true,
            privacy: PrivacyPolicy::default(),
        }
    }
}

impl CapturePolicy {
    pub fn decide_frame(
        &self,
        candidate: FrameCandidate,
        previous: Option<FrameObservation>,
    ) -> CaptureDecision {
        if !self.enabled {
            return CaptureDecision::Skip(CaptureSkipReason::Paused);
        }
        if let PrivacyDecision::Deny { reason } = self.privacy.frame_decision(candidate.source) {
            return CaptureDecision::Deny { reason };
        }
        if let Some(previous) = previous {
            if self.deduplicate
                && previous.source == candidate.source
                && previous.content_digest == candidate.content_digest
            {
                return CaptureDecision::Skip(CaptureSkipReason::Duplicate);
            }
            let elapsed = candidate
                .timestamp_millis
                .saturating_sub(previous.timestamp_millis);
            if elapsed < self.minimum_interval_millis {
                return CaptureDecision::Skip(CaptureSkipReason::TooSoon {
                    elapsed_millis: elapsed,
                });
            }
        }
        CaptureDecision::Admit
    }

    pub fn decide_accessibility(
        &self,
        bundle_identifier: Option<&str>,
        secure_value: bool,
    ) -> PrivacyDecision {
        if !self.enabled {
            return PrivacyDecision::Deny {
                reason: PrivacyDenyReason::Paused,
            };
        }
        self.privacy
            .accessibility_decision(bundle_identifier, secure_value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureDecision {
    Admit,
    Skip(CaptureSkipReason),
    Deny { reason: PrivacyDenyReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureSkipReason {
    Paused,
    Duplicate,
    TooSoon { elapsed_millis: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivacyDecision {
    Allow,
    Redact { window_title: bool, value: bool },
    Deny { reason: PrivacyDenyReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivacyDenyReason {
    Paused,
    SourceExcluded,
    ApplicationExcluded,
    UnknownApplication,
    SecureValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivacyPolicy {
    pub excluded_sources: BTreeSet<CaptureSource>,
    pub excluded_bundle_identifiers: BTreeSet<String>,
    pub redact_window_titles: bool,
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            excluded_sources: BTreeSet::new(),
            excluded_bundle_identifiers: BTreeSet::new(),
            redact_window_titles: true,
        }
    }
}

impl PrivacyPolicy {
    pub fn frame_decision(&self, source: CaptureSource) -> PrivacyDecision {
        if self.excluded_sources.contains(&source) {
            PrivacyDecision::Deny {
                reason: PrivacyDenyReason::SourceExcluded,
            }
        } else {
            PrivacyDecision::Allow
        }
    }

    pub fn accessibility_decision(
        &self,
        bundle_identifier: Option<&str>,
        secure_value: bool,
    ) -> PrivacyDecision {
        let Some(bundle_identifier) = bundle_identifier else {
            return PrivacyDecision::Deny {
                reason: PrivacyDenyReason::UnknownApplication,
            };
        };
        if !is_valid_bundle_identifier(bundle_identifier) {
            return PrivacyDecision::Deny {
                reason: PrivacyDenyReason::UnknownApplication,
            };
        }
        if self.excluded_bundle_identifiers.contains(bundle_identifier) {
            return PrivacyDecision::Deny {
                reason: PrivacyDenyReason::ApplicationExcluded,
            };
        }
        if secure_value {
            return PrivacyDecision::Deny {
                reason: PrivacyDenyReason::SecureValue,
            };
        }
        PrivacyDecision::Redact {
            window_title: self.redact_window_titles,
            value: false,
        }
    }
}

fn is_valid_bundle_identifier(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.chars().count() <= MAX_BUNDLE_IDENTIFIER_CHARS
        && value.is_ascii()
        && value.split('.').all(|part| !part.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(timestamp_millis: u64) -> FrameCandidate {
        FrameCandidate {
            source: CaptureSource::Display { id: 4 },
            timestamp_millis,
            content_digest: [9; 32],
        }
    }

    #[test]
    fn paused_policy_does_not_admit_frames() {
        let policy = CapturePolicy {
            enabled: false,
            ..CapturePolicy::default()
        };

        assert_eq!(
            policy.decide_frame(candidate(1), None),
            CaptureDecision::Skip(CaptureSkipReason::Paused)
        );
    }

    #[test]
    fn duplicate_and_rate_limit_decisions_are_deterministic() {
        let policy = CapturePolicy::default();
        let previous = FrameObservation {
            source: CaptureSource::Display { id: 4 },
            timestamp_millis: 1_000,
            content_digest: [9; 32],
        };

        assert_eq!(
            policy.decide_frame(candidate(2_000), Some(previous)),
            CaptureDecision::Skip(CaptureSkipReason::Duplicate)
        );
        assert_eq!(
            policy.decide_frame(
                candidate(1_500),
                Some(FrameObservation {
                    content_digest: [8; 32],
                    ..previous
                })
            ),
            CaptureDecision::Skip(CaptureSkipReason::TooSoon {
                elapsed_millis: 500
            })
        );
    }

    #[test]
    fn accessibility_privacy_defaults_to_deny_for_unknown_or_secure_sources() {
        let policy = CapturePolicy::default();

        assert_eq!(
            policy.decide_accessibility(None, false),
            PrivacyDecision::Deny {
                reason: PrivacyDenyReason::UnknownApplication
            }
        );
        assert_eq!(
            policy.decide_accessibility(Some("com.example.editor"), true),
            PrivacyDecision::Deny {
                reason: PrivacyDenyReason::SecureValue
            }
        );
    }
}
