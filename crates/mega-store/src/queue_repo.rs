use rusqlite::{OptionalExtension, Transaction};
use uuid::Uuid;

use crate::operations_repo::{MemoryEventType, record_event_tx};
use crate::{MemoryStore, StoreError};

pub const MAX_EXTRACTION_ATTEMPTS: u32 = 3;
pub const MAX_LEASE_MILLIS: i64 = 15 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Pending,
    Running,
    Completed,
    NeedsAttention,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionJob {
    pub id: String,
    pub segment_id: String,
    pub state: JobState,
    pub attempts: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub extractor_prompt_version: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionJobCompletion {
    pub provider: String,
    pub model: String,
    pub latency_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub private_content_left_device: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionJobFailure {
    pub error_code: String,
    pub retry_at: i64,
}

impl MemoryStore {
    pub fn enqueue_extraction(
        &mut self,
        segment_id: &str,
        prompt_version: &str,
        correlation_id: &str,
        now_ms: i64,
    ) -> Result<ExtractionJob, StoreError> {
        if prompt_version.trim().is_empty() || prompt_version.chars().count() > 64 {
            return Err(StoreError::InvalidInput("invalid extractor prompt version"));
        }
        let tx = self.connection_mut().transaction()?;
        let segment_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM activity_segments WHERE id=?1)",
            [segment_id],
            |row| row.get(0),
        )?;
        if !segment_exists {
            return Err(StoreError::InvalidInput("unknown activity segment"));
        }
        let id = Uuid::now_v7().to_string();
        let inserted = tx.execute(
            "INSERT INTO extraction_jobs(id, segment_id, state, next_attempt_at,
             extractor_prompt_version, created_at, updated_at)
             VALUES (?1, ?2, 'pending', ?3, ?4, ?3, ?3)
             ON CONFLICT(segment_id) DO NOTHING",
            rusqlite::params![id, segment_id, now_ms, prompt_version],
        )?;
        if inserted == 1 {
            record_event_tx(
                &tx,
                MemoryEventType::SegmentClosed,
                correlation_id,
                None,
                Some(segment_id),
                Some("queued"),
                now_ms,
            )?;
        }
        let job =
            load_job_tx(&tx, segment_id)?.ok_or(StoreError::Invariant("queued job disappeared"))?;
        tx.commit()?;
        Ok(job)
    }

    pub fn claim_extraction(
        &mut self,
        worker_id: &str,
        now_ms: i64,
        lease_millis: i64,
    ) -> Result<Option<ExtractionJob>, StoreError> {
        validate_worker_and_lease(worker_id, lease_millis)?;
        let tx = self.connection_mut().transaction()?;
        let paused: bool = tx.query_row(
            "SELECT extraction_paused FROM memory_settings WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        if paused {
            tx.commit()?;
            return Ok(None);
        }
        tx.execute(
            "UPDATE extraction_jobs SET state='needs_attention', lease_owner=NULL,
             lease_expires_at=NULL, updated_at=?1, last_error_code='lease_exhausted'
             WHERE state='running' AND lease_expires_at<=?1 AND attempts>=?2",
            rusqlite::params![now_ms, MAX_EXTRACTION_ATTEMPTS],
        )?;
        tx.execute(
            "UPDATE extraction_jobs SET state='pending', lease_owner=NULL, lease_expires_at=NULL,
             next_attempt_at=?1, updated_at=?1, last_error_code='lease_expired'
             WHERE state='running' AND lease_expires_at<=?1 AND attempts<?2",
            rusqlite::params![now_ms, MAX_EXTRACTION_ATTEMPTS],
        )?;
        let lease_expires = now_ms
            .checked_add(lease_millis)
            .ok_or(StoreError::InvalidInput("lease timestamp overflow"))?;
        let claimed = tx
            .query_row(
                "UPDATE extraction_jobs SET state='running', attempts=attempts+1, lease_owner=?1,
             lease_expires_at=?2, updated_at=?3
             WHERE id=(SELECT id FROM extraction_jobs WHERE state='pending' AND next_attempt_at<=?3
               AND attempts<?4 ORDER BY next_attempt_at, created_at LIMIT 1)
             RETURNING id, segment_id, state, attempts, lease_owner, lease_expires_at,
               extractor_prompt_version, created_at",
                rusqlite::params![worker_id, lease_expires, now_ms, MAX_EXTRACTION_ATTEMPTS],
                job_from_row,
            )
            .optional()?;
        if let Some(job) = &claimed {
            record_event_tx(
                &tx,
                MemoryEventType::ExtractionStarted,
                &job.id,
                None,
                Some(&job.segment_id),
                Some("running"),
                now_ms,
            )?;
        }
        tx.commit()?;
        Ok(claimed)
    }

    /// Extends a running extraction lease only while it is still owned by the
    /// supplied worker and has not expired. A renewal racing with claim
    /// recovery must fail closed so a stale worker can never extend a lease
    /// that another worker may already own.
    pub fn renew_extraction_lease(
        &mut self,
        job_id: &str,
        worker_id: &str,
        now_ms: i64,
        lease_millis: i64,
    ) -> Result<(), StoreError> {
        validate_worker_and_lease(worker_id, lease_millis)?;
        let lease_expires = now_ms
            .checked_add(lease_millis)
            .ok_or(StoreError::InvalidInput("lease timestamp overflow"))?;
        let tx = self.connection_mut().transaction()?;
        let changed = tx.execute(
            "UPDATE extraction_jobs SET lease_expires_at=?4, updated_at=?3
             WHERE id=?1 AND state='running' AND lease_owner=?2 AND lease_expires_at>?3",
            rusqlite::params![job_id, worker_id, now_ms, lease_expires],
        )?;
        if changed != 1 {
            return Err(StoreError::LeaseLost);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn complete_extraction(
        &mut self,
        job_id: &str,
        worker_id: &str,
        completion: &ExtractionJobCompletion,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        validate_completion(completion)?;
        let tx = self.connection_mut().transaction()?;
        let segment_id = leased_segment(&tx, job_id, worker_id, now_ms)?;
        let changed = tx.execute(
            "UPDATE extraction_jobs SET state='completed', lease_owner=NULL, lease_expires_at=NULL,
             provider=?3, model=?4, latency_ms=?5, input_tokens=?6, output_tokens=?7,
             private_content_left_device=?8, updated_at=?9, last_error_code=NULL
             WHERE id=?1 AND lease_owner=?2 AND state='running'",
            rusqlite::params![
                job_id,
                worker_id,
                completion.provider,
                completion.model,
                i64::try_from(completion.latency_ms)
                    .map_err(|_| StoreError::InvalidInput("latency overflow"))?,
                completion
                    .input_tokens
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| StoreError::InvalidInput("token count overflow"))?,
                completion
                    .output_tokens
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| StoreError::InvalidInput("token count overflow"))?,
                completion.private_content_left_device,
                now_ms
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::LeaseLost);
        }
        tx.execute(
            "UPDATE activity_segments SET extraction_state='completed' WHERE id=?1",
            [&segment_id],
        )?;
        record_event_tx(
            &tx,
            MemoryEventType::ExtractionCompleted,
            job_id,
            None,
            Some(&segment_id),
            Some("completed"),
            now_ms,
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn fail_extraction(
        &mut self,
        job_id: &str,
        worker_id: &str,
        failure: &ExtractionJobFailure,
        now_ms: i64,
    ) -> Result<JobState, StoreError> {
        validate_error_code(&failure.error_code)?;
        if failure.retry_at < now_ms {
            return Err(StoreError::InvalidInput("retry time precedes failure"));
        }
        let tx = self.connection_mut().transaction()?;
        let segment_id = leased_segment(&tx, job_id, worker_id, now_ms)?;
        let attempts: u32 = tx.query_row(
            "SELECT attempts FROM extraction_jobs WHERE id=?1",
            [job_id],
            |row| row.get(0),
        )?;
        let state = if attempts >= MAX_EXTRACTION_ATTEMPTS {
            JobState::NeedsAttention
        } else {
            JobState::Pending
        };
        tx.execute(
            "UPDATE extraction_jobs SET state=?3, lease_owner=NULL, lease_expires_at=NULL,
             next_attempt_at=?4, last_error_code=?5, updated_at=?6 WHERE id=?1 AND lease_owner=?2",
            rusqlite::params![
                job_id,
                worker_id,
                state.as_str(),
                failure.retry_at,
                failure.error_code,
                now_ms
            ],
        )?;
        tx.execute(
            "UPDATE activity_segments SET extraction_state=?2, extraction_attempts=?3 WHERE id=?1",
            rusqlite::params![segment_id, state.as_segment_state(), attempts],
        )?;
        record_event_tx(
            &tx,
            MemoryEventType::ExtractionFailed,
            job_id,
            None,
            Some(&segment_id),
            Some(state.as_str()),
            now_ms,
        )?;
        tx.commit()?;
        Ok(state)
    }
}

impl JobState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::NeedsAttention => "needs_attention",
        }
    }
    const fn as_segment_state(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::NeedsAttention => "needs_attention",
        }
    }
}

fn validate_worker_and_lease(worker_id: &str, lease_millis: i64) -> Result<(), StoreError> {
    if worker_id.trim().is_empty() || worker_id.chars().count() > 128 {
        return Err(StoreError::InvalidInput("invalid extraction worker ID"));
    }
    if !(1_000..=MAX_LEASE_MILLIS).contains(&lease_millis) {
        return Err(StoreError::InvalidInput(
            "lease must be between one second and fifteen minutes",
        ));
    }
    Ok(())
}

fn validate_completion(value: &ExtractionJobCompletion) -> Result<(), StoreError> {
    if value.provider.trim().is_empty()
        || value.provider.chars().count() > 128
        || value.model.trim().is_empty()
        || value.model.chars().count() > 128
    {
        return Err(StoreError::InvalidInput(
            "invalid extraction provider metadata",
        ));
    }
    Ok(())
}

fn validate_error_code(value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return Err(StoreError::InvalidInput(
            "error code must be bounded ASCII metadata",
        ));
    }
    Ok(())
}

fn leased_segment(
    tx: &Transaction<'_>,
    job_id: &str,
    worker_id: &str,
    now_ms: i64,
) -> Result<String, StoreError> {
    tx.query_row(
        "SELECT segment_id FROM extraction_jobs WHERE id=?1 AND state='running'
         AND lease_owner=?2 AND lease_expires_at>?3",
        rusqlite::params![job_id, worker_id, now_ms],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(StoreError::LeaseLost)
}

fn load_job_tx(
    tx: &Transaction<'_>,
    segment_id: &str,
) -> Result<Option<ExtractionJob>, StoreError> {
    Ok(tx
        .query_row(
            "SELECT id, segment_id, state, attempts, lease_owner, lease_expires_at,
         extractor_prompt_version, created_at FROM extraction_jobs WHERE segment_id=?1",
            [segment_id],
            job_from_row,
        )
        .optional()?)
}

fn job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExtractionJob> {
    let state: String = row.get(2)?;
    Ok(ExtractionJob {
        id: row.get(0)?,
        segment_id: row.get(1)?,
        state: parse_job_state_sql(&state)?,
        attempts: row.get(3)?,
        lease_owner: row.get(4)?,
        lease_expires_at: row.get(5)?,
        extractor_prompt_version: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn parse_job_state_sql(value: &str) -> rusqlite::Result<JobState> {
    match value {
        "pending" => Ok(JobState::Pending),
        "running" => Ok(JobState::Running),
        "completed" => Ok(JobState::Completed),
        "needs_attention" => Ok(JobState::NeedsAttention),
        _ => Err(rusqlite::Error::InvalidColumnType(
            2,
            "state".into(),
            rusqlite::types::Type::Text,
        )),
    }
}

#[cfg(test)]
mod tests {
    use mega_memory::{Sensitivity, SourceEventId};

    use super::*;
    use crate::{ActivitySegmentInput, SegmentCloseReason, SourceEventInput, SourceKind};

    fn store_with_segment() -> MemoryStore {
        let mut store = MemoryStore::in_memory().unwrap();
        let source_id = SourceEventId::new("queue-source");
        store
            .insert_source_event(&SourceEventInput {
                id: source_id.clone(),
                correlation_id: "queue-correlation".into(),
                source_kind: SourceKind::AssistantConversation,
                app_id: None,
                started_at: 1,
                ended_at: 2,
                redacted_title: None,
                evidence_text: "User explicitly selected local structured memory.".into(),
                sensitivity: Sensitivity::Private,
                redaction_flags: vec![],
                capture_sequence: None,
                ax_sequence: None,
                created_at: 2,
            })
            .unwrap();
        store
            .insert_activity_segment(&ActivitySegmentInput {
                id: "segment-1".into(),
                app_id: None,
                scope_id: None,
                started_at: 1,
                ended_at: 2,
                close_reason: SegmentCloseReason::SessionEnded,
                source_event_ids: vec![source_id],
            })
            .unwrap();
        store
    }

    #[test]
    fn jobs_are_idempotent_leased_and_completed_with_metadata() {
        let mut store = store_with_segment();
        let first = store
            .enqueue_extraction("segment-1", "memory-v1", "correlation-1", 10)
            .unwrap();
        let retry = store
            .enqueue_extraction("segment-1", "memory-v1", "correlation-1", 11)
            .unwrap();
        assert_eq!(first.id, retry.id);
        let claimed = store
            .claim_extraction("worker-1", 20, 5_000)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.attempts, 1);
        let batches = store
            .load_extraction_batches(&claimed.id, &claimed.segment_id)
            .unwrap();
        assert_eq!(batches.len(), 1);
        assert!(
            batches[0]
                .privacy_filtered_text
                .contains("structured memory")
        );
        assert!(
            store
                .claim_extraction("worker-2", 21, 5_000)
                .unwrap()
                .is_none()
        );
        store
            .complete_extraction(
                &claimed.id,
                "worker-1",
                &ExtractionJobCompletion {
                    provider: "local".into(),
                    model: "fixture".into(),
                    latency_ms: 25,
                    input_tokens: Some(12),
                    output_tokens: Some(4),
                    private_content_left_device: false,
                },
                30,
            )
            .unwrap();
        assert!(
            store
                .claim_extraction("worker-1", 31, 5_000)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .recent_events(10)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == MemoryEventType::SegmentClosed)
                .count(),
            1
        );
    }

    #[test]
    fn lease_renewal_is_owner_checked_and_expiry_checked() {
        let mut store = store_with_segment();
        store
            .enqueue_extraction("segment-1", "memory-v1", "correlation-1", 10)
            .unwrap();
        let job = store
            .claim_extraction("worker-1", 20, 1_000)
            .unwrap()
            .unwrap();
        assert_eq!(job.lease_expires_at, Some(1_020));

        store
            .renew_extraction_lease(&job.id, "worker-1", 500, 2_000)
            .unwrap();
        let renewed_expiry: i64 = store
            .connection()
            .query_row(
                "SELECT lease_expires_at FROM extraction_jobs WHERE id=?1",
                [&job.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(renewed_expiry, 2_500);

        assert!(matches!(
            store.renew_extraction_lease(&job.id, "worker-2", 600, 1_000),
            Err(StoreError::LeaseLost)
        ));
        assert!(matches!(
            store.renew_extraction_lease(&job.id, "worker-1", 2_500, 1_000),
            Err(StoreError::LeaseLost)
        ));
        assert!(
            store
                .claim_extraction("worker-2", 2_501, 1_000)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn expired_leases_retry_then_stop_after_three_attempts() {
        let mut store = store_with_segment();
        store
            .enqueue_extraction("segment-1", "memory-v1", "correlation-1", 10)
            .unwrap();
        for attempt in 0..3 {
            let now = 20 + i64::from(attempt) * 2_000;
            let job = store
                .claim_extraction("worker", now, 1_000)
                .unwrap()
                .unwrap();
            assert_eq!(job.attempts, attempt + 1);
        }
        assert!(
            store
                .claim_extraction("worker", 7_000, 1_000)
                .unwrap()
                .is_none()
        );
        let state: String = store
            .connection()
            .query_row(
                "SELECT state FROM extraction_jobs WHERE segment_id='segment-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "needs_attention");
    }

    #[test]
    fn pause_is_a_durable_kill_switch() {
        let mut store = store_with_segment();
        store
            .enqueue_extraction("segment-1", "memory-v1", "correlation-1", 10)
            .unwrap();
        store.set_extraction_paused(true, 11).unwrap();
        assert!(
            store
                .claim_extraction("worker", 20, 1_000)
                .unwrap()
                .is_none()
        );
        store.set_extraction_paused(false, 21).unwrap();
        assert!(
            store
                .claim_extraction("worker", 22, 1_000)
                .unwrap()
                .is_some()
        );
    }
}
