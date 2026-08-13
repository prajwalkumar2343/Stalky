use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssertionMode, ExtractionRunId, Memory, MemoryId, MemoryStatus, PrivacyRejection, Sensitivity,
    SourceEventId, ValidatedMemoryCandidate, inspect_private_content, normalize_content,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRelationship {
    ExactDuplicate,
    Updates,
    Extends,
    Contradicts,
    Related,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReconciliationMatch {
    pub memory: Memory,
    pub relationship: CandidateRelationship,
    pub confidence: f32,
}

#[derive(Clone, Debug)]
pub struct ReconciliationInput {
    pub extraction_run_id: ExtractionRunId,
    pub candidate_index: u32,
    pub candidate: ValidatedMemoryCandidate,
    /// Existing matches are already restricted to compatible scope/subject.
    pub existing_matches: Vec<ReconciliationMatch>,
    pub supporting_observation_count: u32,
    pub supporting_activity_segment_count: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MemoryMutationPlan {
    Create {
        extraction_run_id: ExtractionRunId,
        candidate_index: u32,
        candidate: crate::MemoryCandidate,
        status: MemoryStatus,
    },
    Duplicate {
        extraction_run_id: ExtractionRunId,
        candidate_index: u32,
        existing_memory_id: MemoryId,
        source_event_ids: Vec<SourceEventId>,
        confidence: f32,
    },
    Update {
        extraction_run_id: ExtractionRunId,
        candidate_index: u32,
        existing_memory_id: MemoryId,
        candidate: crate::MemoryCandidate,
        status: MemoryStatus,
    },
    Extend {
        extraction_run_id: ExtractionRunId,
        candidate_index: u32,
        existing_memory_id: MemoryId,
        candidate: crate::MemoryCandidate,
        status: MemoryStatus,
    },
    Ignore {
        extraction_run_id: ExtractionRunId,
        candidate_index: u32,
        reason: String,
    },
    RequestReview {
        extraction_run_id: ExtractionRunId,
        candidate_index: u32,
        candidate: crate::MemoryCandidate,
        reason: String,
    },
}

impl MemoryMutationPlan {
    pub fn idempotency_key(&self) -> (&ExtractionRunId, u32) {
        match self {
            Self::Create {
                extraction_run_id,
                candidate_index,
                ..
            }
            | Self::Duplicate {
                extraction_run_id,
                candidate_index,
                ..
            }
            | Self::Update {
                extraction_run_id,
                candidate_index,
                ..
            }
            | Self::Extend {
                extraction_run_id,
                candidate_index,
                ..
            }
            | Self::Ignore {
                extraction_run_id,
                candidate_index,
                ..
            }
            | Self::RequestReview {
                extraction_run_id,
                candidate_index,
                ..
            } => (extraction_run_id, *candidate_index),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryMutationResult {
    pub memory_id: Option<MemoryId>,
    pub status: MemoryStatus,
    pub was_already_applied: bool,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ReconciliationError {
    #[error("match confidence must be finite and in [0, 1]")]
    InvalidMatchConfidence,
    #[error("duplicate target is not active")]
    DuplicateTargetNotActive,
    #[error("invalid mutation plan: {0}")]
    InvalidPlan(&'static str),
    #[error(transparent)]
    Privacy(#[from] PrivacyRejection),
}

pub fn reconcile_candidate(
    input: ReconciliationInput,
) -> Result<MemoryMutationPlan, ReconciliationError> {
    if input.existing_matches.iter().any(|matched| {
        !matched.confidence.is_finite() || !(0.0..=1.0).contains(&matched.confidence)
    }) {
        return Err(ReconciliationError::InvalidMatchConfidence);
    }

    let candidate = input.candidate.into_candidate();
    let status = promotion_status(
        candidate.memory_type,
        candidate.assertion_mode,
        candidate.sensitivity,
        input.supporting_observation_count,
        input.supporting_activity_segment_count,
    );
    let fields = PlanFields {
        extraction_run_id: input.extraction_run_id,
        candidate_index: input.candidate_index,
    };

    if let Some(matched) = best_match(
        &input.existing_matches,
        CandidateRelationship::ExactDuplicate,
    ) {
        if matched.memory.status != MemoryStatus::Active {
            return Err(ReconciliationError::DuplicateTargetNotActive);
        }
        return checked(MemoryMutationPlan::Duplicate {
            extraction_run_id: fields.extraction_run_id,
            candidate_index: fields.candidate_index,
            existing_memory_id: matched.memory.id.clone(),
            source_event_ids: candidate.supporting_source_event_ids,
            confidence: matched
                .memory
                .confidence
                .max(candidate.confidence)
                .min(candidate.assertion_mode.confidence_ceiling()),
        });
    }

    let update = best_active_match(&input.existing_matches, CandidateRelationship::Updates)
        .or_else(|| best_active_match(&input.existing_matches, CandidateRelationship::Contradicts));
    if let Some(matched) = update {
        if candidate.assertion_mode.trust_rank() < matched.memory.assertion_mode.trust_rank() {
            return checked(MemoryMutationPlan::RequestReview {
                extraction_run_id: fields.extraction_run_id,
                candidate_index: fields.candidate_index,
                candidate,
                reason: "lower-trust assertion cannot supersede higher-trust memory".into(),
            });
        }
        if status != MemoryStatus::Active {
            return checked(MemoryMutationPlan::RequestReview {
                extraction_run_id: fields.extraction_run_id,
                candidate_index: fields.candidate_index,
                candidate,
                reason: "candidate is not eligible to supersede an active memory".into(),
            });
        }
        return checked(MemoryMutationPlan::Update {
            extraction_run_id: fields.extraction_run_id,
            candidate_index: fields.candidate_index,
            existing_memory_id: matched.memory.id.clone(),
            candidate,
            status,
        });
    }

    if let Some(matched) =
        best_active_match(&input.existing_matches, CandidateRelationship::Extends)
    {
        if status != MemoryStatus::Active {
            return checked(MemoryMutationPlan::RequestReview {
                extraction_run_id: fields.extraction_run_id,
                candidate_index: fields.candidate_index,
                candidate,
                reason: "candidate is not eligible to extend an active memory".into(),
            });
        }
        return checked(MemoryMutationPlan::Extend {
            extraction_run_id: fields.extraction_run_id,
            candidate_index: fields.candidate_index,
            existing_memory_id: matched.memory.id.clone(),
            candidate,
            status,
        });
    }

    if status == MemoryStatus::PendingReview {
        return checked(MemoryMutationPlan::RequestReview {
            extraction_run_id: fields.extraction_run_id,
            candidate_index: fields.candidate_index,
            candidate,
            reason: review_reason(
                input.supporting_observation_count,
                input.supporting_activity_segment_count,
            ),
        });
    }

    checked(MemoryMutationPlan::Create {
        extraction_run_id: fields.extraction_run_id,
        candidate_index: fields.candidate_index,
        candidate,
        status,
    })
}

struct PlanFields {
    extraction_run_id: ExtractionRunId,
    candidate_index: u32,
}

fn best_match(
    matches: &[ReconciliationMatch],
    relationship: CandidateRelationship,
) -> Option<&ReconciliationMatch> {
    matches
        .iter()
        .filter(|matched| matched.relationship == relationship)
        .max_by(|left, right| left.confidence.total_cmp(&right.confidence))
}

fn best_active_match(
    matches: &[ReconciliationMatch],
    relationship: CandidateRelationship,
) -> Option<&ReconciliationMatch> {
    matches
        .iter()
        .filter(|matched| matched.memory.status == MemoryStatus::Active)
        .filter(|matched| matched.relationship == relationship)
        .max_by(|left, right| left.confidence.total_cmp(&right.confidence))
}

fn checked(plan: MemoryMutationPlan) -> Result<MemoryMutationPlan, ReconciliationError> {
    validate_mutation_plan(&plan)?;
    Ok(plan)
}

/// Validates a plan at the domain boundary before a repository applies it.
///
/// The repository still validates foreign-key references and transaction
/// preconditions. This function only checks the plan's self-contained shape,
/// scalar bounds, promotion status, and privacy policy.
pub fn validate_mutation_plan(plan: &MemoryMutationPlan) -> Result<(), ReconciliationError> {
    let (extraction_run_id, candidate_index) = plan.idempotency_key();
    if extraction_run_id.as_str().trim().is_empty() {
        return Err(ReconciliationError::InvalidPlan(
            "extraction run id is empty",
        ));
    }
    let _ = candidate_index;

    match plan {
        MemoryMutationPlan::Create {
            candidate, status, ..
        }
        | MemoryMutationPlan::Update {
            candidate, status, ..
        }
        | MemoryMutationPlan::Extend {
            candidate, status, ..
        } => {
            if !matches!(status, MemoryStatus::Active | MemoryStatus::PendingReview) {
                return Err(ReconciliationError::InvalidPlan(
                    "create, update, and extend status must be active or pending_review",
                ));
            }
            validate_plan_candidate(candidate)?;
        }
        MemoryMutationPlan::RequestReview {
            candidate, reason, ..
        } => {
            validate_plan_candidate(candidate)?;
            validate_reason(reason)?;
        }
        MemoryMutationPlan::Duplicate {
            existing_memory_id,
            source_event_ids,
            confidence,
            ..
        } => {
            if existing_memory_id.as_str().trim().is_empty() {
                return Err(ReconciliationError::InvalidPlan(
                    "duplicate target id is empty",
                ));
            }
            if source_event_ids.len() > 20 {
                return Err(ReconciliationError::InvalidPlan(
                    "duplicate provenance is unbounded",
                ));
            }
            validate_unit("duplicate confidence", *confidence)?;
        }
        MemoryMutationPlan::Ignore { reason, .. } => validate_reason(reason)?,
    }
    Ok(())
}

fn validate_plan_candidate(candidate: &crate::MemoryCandidate) -> Result<(), ReconciliationError> {
    let normalized = normalize_content(&candidate.content);
    if !(8..=500).contains(&normalized.chars().count()) {
        return Err(ReconciliationError::InvalidPlan(
            "candidate content length is outside 8..=500 characters",
        ));
    }
    validate_unit("importance", candidate.importance)?;
    validate_unit("confidence", candidate.confidence)?;
    if candidate.confidence > candidate.assertion_mode.confidence_ceiling() {
        return Err(ReconciliationError::InvalidPlan(
            "candidate confidence exceeds its assertion-mode ceiling",
        ));
    }
    if candidate.category_slugs.len() > 5 {
        return Err(ReconciliationError::InvalidPlan(
            "candidate has more than five categories",
        ));
    }
    if candidate.assertion_mode == AssertionMode::Manual {
        if candidate.supporting_source_event_ids.len() > 20 {
            return Err(ReconciliationError::InvalidPlan(
                "manual candidate provenance is unbounded",
            ));
        }
    } else if !(1..=20).contains(&candidate.supporting_source_event_ids.len()) {
        return Err(ReconciliationError::InvalidPlan(
            "candidate provenance must contain 1..=20 source events",
        ));
    }
    if candidate.scope.scope_type != crate::ScopeType::Global
        && candidate.scope.scope_key.trim().is_empty()
    {
        return Err(ReconciliationError::InvalidPlan(
            "non-global candidate scope has no key",
        ));
    }
    if let (Some(from), Some(until)) = (candidate.valid_from_ms, candidate.valid_until_ms)
        && until < from
    {
        return Err(ReconciliationError::InvalidPlan(
            "candidate validity interval is reversed",
        ));
    }
    inspect_private_content(
        &normalized,
        candidate.from_password_field,
        candidate.assertion_mode == AssertionMode::Inferred,
    )?;
    Ok(())
}

fn validate_unit(field: &'static str, value: f32) -> Result<(), ReconciliationError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ReconciliationError::InvalidPlan(field))
    }
}

fn validate_reason(reason: &str) -> Result<(), ReconciliationError> {
    if reason.trim().is_empty() || reason.chars().count() > 256 {
        return Err(ReconciliationError::InvalidPlan(
            "audit reason must contain 1..=256 characters",
        ));
    }
    Ok(())
}

fn promotion_status(
    memory_type: crate::MemoryType,
    mode: AssertionMode,
    sensitivity: Sensitivity,
    observations: u32,
    segments: u32,
) -> MemoryStatus {
    match mode {
        AssertionMode::Observed
            if memory_type == crate::MemoryType::Preference
                && (observations < 3 || segments < 2) =>
        {
            MemoryStatus::PendingReview
        }
        AssertionMode::Inferred if sensitivity == Sensitivity::Sensitive => {
            MemoryStatus::PendingReview
        }
        _ => MemoryStatus::Active,
    }
}

fn review_reason(observations: u32, segments: u32) -> String {
    if observations < 3 || segments < 2 {
        "observed preference requires three observations across two activity segments".into()
    } else {
        "sensitive inferred memory requires review".into()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{
        AppId, CandidateScope, CandidateValidationContext, MemoryCandidate, MemoryScope,
        MemoryType, ScopeId, ScopeType,
    };

    use super::*;

    fn validated(mode: AssertionMode) -> ValidatedMemoryCandidate {
        let candidate = MemoryCandidate {
            content: "User prefers Svelte for new projects.".into(),
            memory_type: MemoryType::Preference,
            assertion_mode: mode,
            category_slugs: vec![],
            scope: CandidateScope {
                scope_type: ScopeType::Global,
                scope_key: "global".into(),
                display_name: "Global".into(),
            },
            source_app_ids: vec![AppId::from("slack")],
            applicable_app_ids: vec![],
            entity_mentions: vec![],
            importance: 0.8,
            confidence: mode.confidence_ceiling(),
            valid_from_ms: None,
            valid_until_ms: None,
            supporting_source_event_ids: vec![SourceEventId::from("s1")],
            sensitivity: Sensitivity::Private,
            from_password_field: false,
        };
        let sources = HashSet::from([SourceEventId::from("s1")]);
        let categories = HashSet::new();
        let apps = HashSet::from([AppId::from("slack")]);
        candidate
            .validate(&CandidateValidationContext {
                extraction_batch_source_ids: &sources,
                category_slugs: &categories,
                app_catalog: &apps,
            })
            .unwrap()
    }

    fn memory(mode: AssertionMode) -> Memory {
        Memory {
            id: MemoryId::from("m1"),
            normalized_content: "user prefers react".into(),
            display_content: "User prefers React.".into(),
            memory_type: MemoryType::Preference,
            assertion_mode: mode,
            status: MemoryStatus::Active,
            scope: MemoryScope {
                id: ScopeId::from("global"),
                scope_type: ScopeType::Global,
                scope_key: "global".into(),
                display_name: "Global".into(),
            },
            source_app_ids: vec![],
            applicable_app_ids: vec![],
            category_slugs: vec![],
            entities: vec![],
            source_event_ids: vec![],
            importance: 0.7,
            confidence: 1.0,
            sensitivity: Sensitivity::Private,
            valid_from_ms: None,
            valid_until_ms: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            revision: 1,
        }
    }

    fn input(
        candidate: ValidatedMemoryCandidate,
        matches: Vec<ReconciliationMatch>,
    ) -> ReconciliationInput {
        ReconciliationInput {
            extraction_run_id: ExtractionRunId::from("run"),
            candidate_index: 0,
            candidate,
            existing_matches: matches,
            supporting_observation_count: 1,
            supporting_activity_segment_count: 1,
        }
    }

    #[test]
    fn lower_trust_cannot_update_explicit_memory() {
        let matched = ReconciliationMatch {
            memory: memory(AssertionMode::Explicit),
            relationship: CandidateRelationship::Updates,
            confidence: 0.9,
        };
        assert!(matches!(
            reconcile_candidate(input(validated(AssertionMode::Inferred), vec![matched])).unwrap(),
            MemoryMutationPlan::RequestReview { .. }
        ));
    }

    #[test]
    fn observed_memory_requires_repeated_cross_segment_support() {
        assert!(matches!(
            reconcile_candidate(input(validated(AssertionMode::Observed), vec![])).unwrap(),
            MemoryMutationPlan::RequestReview { .. }
        ));
        let mut promoted = input(validated(AssertionMode::Observed), vec![]);
        promoted.supporting_observation_count = 3;
        promoted.supporting_activity_segment_count = 2;
        assert!(matches!(
            reconcile_candidate(promoted).unwrap(),
            MemoryMutationPlan::Create {
                status: MemoryStatus::Active,
                ..
            }
        ));
    }

    #[test]
    fn exact_match_attaches_evidence_without_new_assertion() {
        let matched = ReconciliationMatch {
            memory: memory(AssertionMode::Explicit),
            relationship: CandidateRelationship::ExactDuplicate,
            confidence: 1.0,
        };
        assert!(
            matches!(reconcile_candidate(input(validated(AssertionMode::Explicit), vec![matched])).unwrap(), MemoryMutationPlan::Duplicate { existing_memory_id, .. } if existing_memory_id == MemoryId::from("m1"))
        );
    }

    #[test]
    fn mutation_plan_validation_rejects_unbounded_or_terminal_states() {
        assert!(matches!(
            validate_mutation_plan(&MemoryMutationPlan::Ignore {
                extraction_run_id: ExtractionRunId::from("run"),
                candidate_index: 0,
                reason: " ".into(),
            }),
            Err(ReconciliationError::InvalidPlan(_))
        ));

        let plan = MemoryMutationPlan::Create {
            extraction_run_id: ExtractionRunId::from("run"),
            candidate_index: 0,
            candidate: validated(AssertionMode::Explicit).into_candidate(),
            status: MemoryStatus::Superseded,
        };
        assert!(matches!(
            validate_mutation_plan(&plan),
            Err(ReconciliationError::InvalidPlan(_))
        ));
    }
}
