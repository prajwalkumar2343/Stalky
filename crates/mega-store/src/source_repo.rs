use mega_memory::{AppId, ExtractionBatch, ExtractionRunId, ScopeId, Sensitivity, SourceEventId};
use sha2::{Digest, Sha256};

use crate::{MemoryStore, StoreError};

pub const MAX_EVIDENCE_CHARS: usize = 12_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    AccessibilitySegment,
    AudioTranscriptSegment,
    AssistantConversation,
    ManualEntry,
    StructuredImport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentCloseReason {
    AppChanged,
    ProjectChanged,
    Inactivity,
    SessionEnded,
    MaximumDuration,
    CapturePaused,
    Shutdown,
    Sleep,
}

impl SegmentCloseReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::AppChanged => "app_changed",
            Self::ProjectChanged => "project_changed",
            Self::Inactivity => "inactivity",
            Self::SessionEnded => "session_ended",
            Self::MaximumDuration => "maximum_duration",
            Self::CapturePaused => "capture_paused",
            Self::Shutdown => "shutdown",
            Self::Sleep => "sleep",
        }
    }
}

impl SourceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AccessibilitySegment => "accessibility_segment",
            Self::AudioTranscriptSegment => "audio_transcript_segment",
            Self::AssistantConversation => "assistant_conversation",
            Self::ManualEntry => "manual_entry",
            Self::StructuredImport => "structured_import",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceEventInput {
    pub id: SourceEventId,
    pub correlation_id: String,
    pub source_kind: SourceKind,
    pub app_id: Option<AppId>,
    pub started_at: i64,
    pub ended_at: i64,
    pub redacted_title: Option<String>,
    pub evidence_text: String,
    pub sensitivity: Sensitivity,
    pub redaction_flags: Vec<String>,
    pub capture_sequence: Option<i64>,
    pub ax_sequence: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceEventAdmission {
    Inserted(SourceEventId),
    Duplicate { existing_id: SourceEventId },
}

#[derive(Clone, Debug)]
pub struct ActivitySegmentInput {
    pub id: String,
    pub app_id: Option<AppId>,
    pub scope_id: Option<ScopeId>,
    pub started_at: i64,
    pub ended_at: i64,
    pub close_reason: SegmentCloseReason,
    pub source_event_ids: Vec<SourceEventId>,
}

impl MemoryStore {
    /// Persists only bounded derived text. This API has no raw frame/audio variant.
    pub fn insert_source_event(
        &self,
        input: &SourceEventInput,
    ) -> Result<SourceEventAdmission, StoreError> {
        validate_source_event(input)?;
        if source_is_denied(self, input)? {
            return Err(StoreError::InvalidInput(
                "source is disabled by extraction policy",
            ));
        }
        let flags = serde_json::to_string(&input.redaction_flags)?;
        let hash = content_hash(&input.evidence_text, input.app_id.as_ref());
        let inserted = self.connection().execute(
            "INSERT OR IGNORE INTO source_events (
                id, correlation_id, source_kind, app_id, started_at, ended_at,
                redacted_title, evidence_text, content_hash, sensitivity,
                redaction_flags, capture_sequence, ax_sequence, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                input.id.as_str(),
                input.correlation_id,
                input.source_kind.as_str(),
                input.app_id.as_ref().map(AppId::as_str),
                input.started_at,
                input.ended_at,
                input.redacted_title,
                input.evidence_text,
                hash.as_slice(),
                sensitivity_str(input.sensitivity),
                flags,
                input.capture_sequence,
                input.ax_sequence,
                input.created_at,
            ],
        )?;
        if inserted == 1 {
            return Ok(SourceEventAdmission::Inserted(input.id.clone()));
        }
        let existing_id: String = self.connection().query_row(
            "SELECT id FROM source_events WHERE source_kind=?1 AND content_hash=?2 AND started_at=?3",
            rusqlite::params![input.source_kind.as_str(), hash.as_slice(), input.started_at],
            |row| row.get(0),
        )?;
        Ok(SourceEventAdmission::Duplicate {
            existing_id: SourceEventId::new(existing_id),
        })
    }

    pub fn insert_activity_segment(
        &mut self,
        input: &ActivitySegmentInput,
    ) -> Result<(), StoreError> {
        if input.ended_at < input.started_at {
            return Err(StoreError::InvalidInput("segment end precedes start"));
        }
        if input.ended_at - input.started_at > 15 * 60 * 1_000 {
            return Err(StoreError::InvalidInput("segment exceeds 15 minutes"));
        }
        if input.source_event_ids.is_empty() {
            return Err(StoreError::InvalidInput(
                "segment requires at least one source",
            ));
        }
        if input.source_event_ids.len() > 1_000 {
            return Err(StoreError::InvalidInput(
                "segment source count exceeds 1,000",
            ));
        }
        let tx = self.connection_mut().transaction()?;
        tx.execute(
            "INSERT INTO activity_segments (id, app_id, scope_id, started_at, ended_at, close_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                input.id,
                input.app_id.as_ref().map(AppId::as_str),
                input.scope_id.as_ref().map(ScopeId::as_str),
                input.started_at,
                input.ended_at,
                input.close_reason.as_str(),
            ],
        )?;
        for source_id in &input.source_event_ids {
            tx.execute(
                "INSERT INTO activity_segment_sources (segment_id, source_event_id) VALUES (?1, ?2)",
                rusqlite::params![input.id, source_id.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn expire_source_text(&self, older_than: i64) -> Result<usize, StoreError> {
        Ok(self.connection().execute(
            "UPDATE source_events SET evidence_text = '' WHERE ended_at < ?1 AND evidence_text <> ''",
            [older_than],
        )?)
    }

    pub fn load_extraction_batches(
        &self,
        job_id: &str,
        segment_id: &str,
    ) -> Result<Vec<ExtractionBatch>, StoreError> {
        let mut stmt = self.connection().prepare(
            "SELECT se.id, se.evidence_text FROM activity_segment_sources ass
             JOIN source_events se ON se.id=ass.source_event_id
             WHERE ass.segment_id=?1 AND se.evidence_text<>'' ORDER BY se.started_at, se.id",
        )?;
        let sources = stmt
            .query_map([segment_id], |row| {
                Ok((
                    SourceEventId::new(row.get::<_, String>(0)?),
                    row.get::<_, String>(1)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut batches = Vec::new();
        let mut ids = Vec::new();
        let mut texts = Vec::new();
        let mut chars = 0_usize;
        for (source_id, text) in sources {
            let text_chars = text.chars().count();
            if text_chars > MAX_EVIDENCE_CHARS {
                return Err(StoreError::Invariant(
                    "persisted source exceeds extraction bound",
                ));
            }
            if !ids.is_empty() && chars.saturating_add(text_chars) > MAX_EVIDENCE_CHARS {
                push_batch(&mut batches, job_id, segment_id, &mut ids, &mut texts);
                chars = 0;
            }
            chars = chars.saturating_add(text_chars);
            ids.push(source_id);
            texts.push(text);
        }
        if !ids.is_empty() {
            push_batch(&mut batches, job_id, segment_id, &mut ids, &mut texts);
        }
        Ok(batches)
    }
}

fn push_batch(
    batches: &mut Vec<ExtractionBatch>,
    job_id: &str,
    segment_id: &str,
    ids: &mut Vec<SourceEventId>,
    texts: &mut Vec<String>,
) {
    let index = batches.len();
    batches.push(ExtractionBatch {
        extraction_run_id: ExtractionRunId::new(format!("{job_id}:{index}")),
        activity_segment_ids: vec![segment_id.to_owned()],
        source_event_ids: std::mem::take(ids),
        privacy_filtered_text: std::mem::take(texts).join("\n"),
    });
}

fn validate_source_event(input: &SourceEventInput) -> Result<(), StoreError> {
    if input.ended_at < input.started_at {
        return Err(StoreError::InvalidInput("source end precedes start"));
    }
    if input.evidence_text.chars().count() > MAX_EVIDENCE_CHARS {
        return Err(StoreError::InvalidInput(
            "source evidence exceeds 12,000 characters",
        ));
    }
    if input.source_kind != SourceKind::ManualEntry && input.evidence_text.trim().is_empty() {
        return Err(StoreError::InvalidInput(
            "captured source evidence is empty",
        ));
    }
    if input.correlation_id.trim().is_empty() || input.correlation_id.chars().count() > 128 {
        return Err(StoreError::InvalidInput(
            "source correlation ID must be 1..=128 characters",
        ));
    }
    if input
        .redacted_title
        .as_ref()
        .is_some_and(|title| title.chars().count() > 500)
    {
        return Err(StoreError::InvalidInput(
            "redacted title exceeds 500 characters",
        ));
    }
    if input.redaction_flags.len() > 32
        || input
            .redaction_flags
            .iter()
            .any(|flag| flag.is_empty() || flag.chars().count() > 64 || !flag.is_ascii())
    {
        return Err(StoreError::InvalidInput(
            "redaction flags are unbounded or invalid",
        ));
    }
    if input
        .redaction_flags
        .iter()
        .any(|flag| flag == "password_field" || flag == "denied_source")
    {
        return Err(StoreError::InvalidInput(
            "source is prohibited by privacy policy",
        ));
    }
    Ok(())
}

fn source_is_denied(store: &MemoryStore, input: &SourceEventInput) -> Result<bool, StoreError> {
    let Some(app_id) = &input.app_id else {
        return Ok(false);
    };
    Ok(store.connection().query_row(
        "SELECT EXISTS(
            SELECT 1 FROM extraction_policies p JOIN apps a ON a.bundle_identifier=p.policy_key
            WHERE p.policy_type='app' AND p.enabled=0 AND a.id=?1
         )",
        [app_id.as_str()],
        |row| row.get(0),
    )?)
}

fn content_hash(content: &str, app_id: Option<&AppId>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(
        content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_bytes(),
    );
    hasher.update([0]);
    if let Some(app_id) = app_id {
        hasher.update(app_id.as_str().as_bytes());
    }
    hasher.finalize().into()
}

pub(crate) const fn sensitivity_str(value: Sensitivity) -> &'static str {
    match value {
        Sensitivity::Public => "public",
        Sensitivity::Private => "private",
        Sensitivity::Sensitive => "sensitive",
    }
}
