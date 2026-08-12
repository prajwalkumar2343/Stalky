use mega_memory::{MemoryId, MemoryStatus};
use rusqlite::{OptionalExtension, Transaction};
use uuid::Uuid;

use crate::{MemoryStore, StoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteMode {
    Forget,
    Permanent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryEventType {
    SegmentClosed,
    ExtractionStarted,
    ExtractionCompleted,
    ExtractionFailed,
    CandidateRejected,
    ReconciliationApplied,
    ReconciliationFailed,
    ProfileRegenerated,
    ProfileFailed,
    ContextAssembled,
    MemoryConfirmed,
    MemoryRejected,
    MemoryEdited,
    MemoryForgotten,
    MemoryDeleted,
    EntityMerged,
}

impl MemoryEventType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SegmentClosed => "segment_closed",
            Self::ExtractionStarted => "extraction_started",
            Self::ExtractionCompleted => "extraction_completed",
            Self::ExtractionFailed => "extraction_failed",
            Self::CandidateRejected => "candidate_rejected",
            Self::ReconciliationApplied => "reconciliation_applied",
            Self::ReconciliationFailed => "reconciliation_failed",
            Self::ProfileRegenerated => "profile_regenerated",
            Self::ProfileFailed => "profile_failed",
            Self::ContextAssembled => "context_assembled",
            Self::MemoryConfirmed => "memory_confirmed",
            Self::MemoryRejected => "memory_rejected",
            Self::MemoryEdited => "memory_edited",
            Self::MemoryForgotten => "memory_forgotten",
            Self::MemoryDeleted => "memory_deleted",
            Self::EntityMerged => "entity_merged",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryEvent {
    pub id: String,
    pub event_type: MemoryEventType,
    pub correlation_id: String,
    pub memory_id: Option<MemoryId>,
    pub segment_id: Option<String>,
    pub outcome: Option<String>,
    pub occurred_at: i64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetentionReport {
    pub source_texts_expired: usize,
    pub audits_deleted: usize,
    pub events_deleted: usize,
}

impl MemoryStore {
    pub fn set_extraction_paused(&self, paused: bool, now_ms: i64) -> Result<(), StoreError> {
        self.connection().execute(
            "UPDATE memory_settings SET extraction_paused=?1, updated_at=?2 WHERE id=1",
            rusqlite::params![paused, now_ms],
        )?;
        Ok(())
    }

    pub fn extraction_paused(&self) -> Result<bool, StoreError> {
        Ok(self.connection().query_row(
            "SELECT extraction_paused FROM memory_settings WHERE id=1",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn set_extraction_policy(
        &self,
        policy_type: &str,
        policy_key: &str,
        enabled: bool,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        if !matches!(policy_type, "app" | "window" | "category") || policy_key.trim().is_empty() {
            return Err(StoreError::InvalidInput("invalid extraction policy"));
        }
        self.connection().execute(
            "INSERT INTO extraction_policies(policy_type, policy_key, enabled, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(policy_type, policy_key) DO UPDATE SET enabled=excluded.enabled, updated_at=excluded.updated_at",
            rusqlite::params![policy_type, policy_key, enabled, now_ms],
        )?;
        Ok(())
    }

    pub fn confirm_pending(
        &mut self,
        memory_id: &MemoryId,
        expected_revision: u32,
        correlation_id: &str,
        now_ms: i64,
    ) -> Result<u32, StoreError> {
        let tx = self.connection_mut().transaction()?;
        ensure_revision(&tx, memory_id, expected_revision)?;
        let changed = tx.execute(
            "UPDATE memories SET status='active', assertion_mode='manual', confidence=1.0,
             revision=revision+1, updated_at=?2 WHERE id=?1 AND status='pending_review' AND revision=?3",
            rusqlite::params![memory_id.as_str(), now_ms, expected_revision],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidInput("memory is not pending review"));
        }
        super::memory_repo::refresh_search_document(&tx, memory_id.as_str())?;
        enqueue_projection_refresh_tx(&tx, memory_id.as_str(), expected_revision + 1, now_ms)?;
        record_event_tx(
            &tx,
            MemoryEventType::MemoryConfirmed,
            correlation_id,
            Some(memory_id),
            None,
            Some("active"),
            now_ms,
        )?;
        tx.commit()?;
        Ok(expected_revision + 1)
    }

    pub fn reject_pending(
        &mut self,
        memory_id: &MemoryId,
        expected_revision: u32,
        correlation_id: &str,
        now_ms: i64,
    ) -> Result<u32, StoreError> {
        let tx = self.connection_mut().transaction()?;
        ensure_revision(&tx, memory_id, expected_revision)?;
        let changed = tx.execute(
            "UPDATE memories SET status='rejected', revision=revision+1, updated_at=?2
             WHERE id=?1 AND status='pending_review' AND revision=?3",
            rusqlite::params![memory_id.as_str(), now_ms, expected_revision],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidInput("memory is not pending review"));
        }
        tx.execute(
            "DELETE FROM memory_search_documents WHERE memory_id=?1",
            [memory_id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM memory_embeddings WHERE memory_id=?1",
            [memory_id.as_str()],
        )?;
        record_event_tx(
            &tx,
            MemoryEventType::MemoryRejected,
            correlation_id,
            Some(memory_id),
            None,
            Some("rejected"),
            now_ms,
        )?;
        tx.commit()?;
        Ok(expected_revision + 1)
    }

    pub fn apply_retention(&mut self, now_ms: i64) -> Result<RetentionReport, StoreError> {
        let tx = self.connection_mut().transaction()?;
        let (source_days, audit_days): (i64, i64) = tx.query_row(
            "SELECT source_retention_days, audit_retention_days FROM memory_settings WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let source_cutoff = now_ms.saturating_sub(source_days.saturating_mul(86_400_000));
        let audit_cutoff = now_ms.saturating_sub(audit_days.saturating_mul(86_400_000));
        let source_texts_expired = tx.execute(
            "UPDATE source_events SET evidence_text='' WHERE ended_at<?1 AND evidence_text<>''",
            [source_cutoff],
        )?;
        let audits_deleted = tx.execute(
            "DELETE FROM extraction_candidates WHERE outcome IN ('ignore', 'request_review')
             AND created_at<?1 AND memory_id IS NULL",
            [audit_cutoff],
        )?;
        let events_deleted = tx.execute(
            "DELETE FROM memory_events WHERE occurred_at<?1",
            [audit_cutoff],
        )?;
        tx.commit()?;
        Ok(RetentionReport {
            source_texts_expired,
            audits_deleted,
            events_deleted,
        })
    }

    pub fn recent_events(&self, limit: usize) -> Result<Vec<MemoryEvent>, StoreError> {
        if !(1..=500).contains(&limit) {
            return Err(StoreError::InvalidInput("event limit must be 1..=500"));
        }
        let mut stmt = self.connection().prepare(
            "SELECT id, event_type, correlation_id, memory_id, segment_id, outcome, occurred_at
             FROM memory_events ORDER BY occurred_at DESC, id DESC LIMIT ?1",
        )?;
        let events = stmt
            .query_map([limit as i64], |row| {
                let event_type: String = row.get(1)?;
                Ok((
                    row.get::<_, String>(0)?,
                    event_type,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .map(|row| {
                let (id, event_type, correlation_id, memory_id, segment_id, outcome, occurred_at) =
                    row?;
                Ok(MemoryEvent {
                    id,
                    event_type: parse_event_type(&event_type)?,
                    correlation_id,
                    memory_id: memory_id.map(MemoryId::new),
                    segment_id,
                    outcome,
                    occurred_at,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(events)
    }
}

pub(crate) fn ensure_revision(
    tx: &Transaction<'_>,
    memory_id: &MemoryId,
    expected: u32,
) -> Result<MemoryStatus, StoreError> {
    let current: Option<(u32, String)> = tx
        .query_row(
            "SELECT revision, status FROM memories WHERE id=?1",
            [memory_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((actual, status)) = current else {
        return Err(StoreError::NotFound);
    };
    if actual != expected {
        return Err(StoreError::RevisionConflict { expected, actual });
    }
    super::memory_repo::parse_status(&status)
}

pub(crate) fn enqueue_projection_refresh_tx(
    tx: &Transaction<'_>,
    memory_id: &str,
    revision: u32,
    now_ms: i64,
) -> Result<(), StoreError> {
    for projection_type in ["embedding", "profile"] {
        tx.execute(
            "INSERT INTO projection_jobs(projection_type, projection_key, source_revision, state, updated_at)
             VALUES (?1, ?2, ?3, 'pending', ?4)
             ON CONFLICT(projection_type, projection_key) DO UPDATE SET
               source_revision=MAX(source_revision, excluded.source_revision), state='pending', attempts=0, updated_at=excluded.updated_at",
            rusqlite::params![projection_type, memory_id, revision, now_ms],
        )?;
    }
    Ok(())
}

pub(crate) fn record_event_tx(
    tx: &Transaction<'_>,
    event_type: MemoryEventType,
    correlation_id: &str,
    memory_id: Option<&MemoryId>,
    segment_id: Option<&str>,
    outcome: Option<&str>,
    now_ms: i64,
) -> Result<(), StoreError> {
    if correlation_id.trim().is_empty() || correlation_id.chars().count() > 128 {
        return Err(StoreError::InvalidInput("invalid correlation ID"));
    }
    tx.execute(
        "INSERT INTO memory_events(id, event_type, correlation_id, memory_id, segment_id, outcome, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![Uuid::now_v7().to_string(), event_type.as_str(), correlation_id,
            memory_id.map(MemoryId::as_str), segment_id, outcome, now_ms],
    )?;
    Ok(())
}

fn parse_event_type(value: &str) -> Result<MemoryEventType, StoreError> {
    match value {
        "segment_closed" => Ok(MemoryEventType::SegmentClosed),
        "extraction_started" => Ok(MemoryEventType::ExtractionStarted),
        "extraction_completed" => Ok(MemoryEventType::ExtractionCompleted),
        "extraction_failed" => Ok(MemoryEventType::ExtractionFailed),
        "candidate_rejected" => Ok(MemoryEventType::CandidateRejected),
        "reconciliation_applied" => Ok(MemoryEventType::ReconciliationApplied),
        "reconciliation_failed" => Ok(MemoryEventType::ReconciliationFailed),
        "profile_regenerated" => Ok(MemoryEventType::ProfileRegenerated),
        "profile_failed" => Ok(MemoryEventType::ProfileFailed),
        "context_assembled" => Ok(MemoryEventType::ContextAssembled),
        "memory_confirmed" => Ok(MemoryEventType::MemoryConfirmed),
        "memory_rejected" => Ok(MemoryEventType::MemoryRejected),
        "memory_edited" => Ok(MemoryEventType::MemoryEdited),
        "memory_forgotten" => Ok(MemoryEventType::MemoryForgotten),
        "memory_deleted" => Ok(MemoryEventType::MemoryDeleted),
        "entity_merged" => Ok(MemoryEventType::EntityMerged),
        _ => Err(StoreError::InvalidEnum(
            "memory_event_type",
            value.to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use mega_memory::{
        AppId, AssertionMode, CandidateScope, ExtractionRunId, MemoryCandidate, MemoryMutationPlan,
        MemoryStatus, MemoryType, ScopeType, Sensitivity, SourceEventId,
    };

    use super::*;
    use crate::{SourceEventInput, SourceKind};

    fn pending_candidate() -> MemoryCandidate {
        MemoryCandidate {
            content: "User may prefer restrained interface motion.".into(),
            memory_type: MemoryType::Preference,
            assertion_mode: AssertionMode::Inferred,
            category_slugs: vec!["choices.design.interaction".into()],
            scope: CandidateScope {
                scope_type: ScopeType::Global,
                scope_key: String::new(),
                display_name: "Global".into(),
            },
            source_app_ids: Vec::<AppId>::new(),
            applicable_app_ids: vec![],
            entity_mentions: vec![],
            importance: 0.6,
            confidence: 0.7,
            valid_from_ms: None,
            valid_until_ms: None,
            supporting_source_event_ids: vec![SourceEventId::new("review-source")],
            sensitivity: Sensitivity::Private,
            from_password_field: false,
        }
    }

    #[test]
    fn review_uses_optimistic_revision_and_emits_content_free_event() {
        let mut store = MemoryStore::in_memory().unwrap();
        store
            .insert_source_event(&SourceEventInput {
                id: SourceEventId::new("review-source"),
                correlation_id: "review-evidence".into(),
                source_kind: SourceKind::AssistantConversation,
                app_id: None,
                started_at: 1,
                ended_at: 2,
                redacted_title: None,
                evidence_text: "The interface motion may be too strong.".into(),
                sensitivity: Sensitivity::Private,
                redaction_flags: vec![],
                capture_sequence: None,
                ax_sequence: None,
                created_at: 2,
            })
            .unwrap();
        let result = store
            .apply_plan(
                &MemoryMutationPlan::RequestReview {
                    extraction_run_id: ExtractionRunId::new("review-run"),
                    candidate_index: 0,
                    candidate: pending_candidate(),
                    reason: "inferred memory requires confirmation".into(),
                },
                10,
            )
            .unwrap();
        let memory_id = result.memory_id.unwrap();
        assert!(matches!(
            store.confirm_pending(&memory_id, 2, "review-confirm", 20),
            Err(StoreError::RevisionConflict { actual: 1, .. })
        ));
        assert_eq!(
            store
                .confirm_pending(&memory_id, 1, "review-confirm", 20)
                .unwrap(),
            2
        );
        let memory = store.get_memory(&memory_id).unwrap().unwrap();
        assert_eq!(memory.status, MemoryStatus::Active);
        assert_eq!(memory.assertion_mode, AssertionMode::Manual);
        let events = store.recent_events(10).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_type == MemoryEventType::MemoryConfirmed)
        );
        assert!(!format!("{events:?}").contains(&memory.display_content));
    }
}
