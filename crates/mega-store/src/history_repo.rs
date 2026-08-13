use std::path::PathBuf;

use mega_memory::Sensitivity;
use rusqlite::{OptionalExtension, Row, Transaction};

use crate::{MemoryStore, StoreError};

const MAX_TIMELINE_ID_CHARS: usize = 256;
const MAX_TIMELINE_TEXT_CHARS: usize = 100_000;
const MAX_AUDIO_RECOVERY_ERROR_CHARS: usize = 2_000;
const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "heic", "jpeg", "jpg", "png", "tif", "tiff", "webp",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryMediaKind {
    Text,
    Audio,
}

pub type TimelineMediaKind = HistoryMediaKind;
pub type HistorySourceKind = TimelineSourceKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineSourceKind {
    Accessibility,
    Ocr,
    AudioTranscript,
    AssistantConversation,
    Manual,
    StructuredImport,
}

impl TimelineSourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::Ocr => "ocr",
            Self::AudioTranscript => "audio_transcript",
            Self::AssistantConversation => "assistant_conversation",
            Self::Manual => "manual",
            Self::StructuredImport => "structured_import",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "accessibility" => Ok(Self::Accessibility),
            "ocr" => Ok(Self::Ocr),
            "audio_transcript" => Ok(Self::AudioTranscript),
            "assistant_conversation" => Ok(Self::AssistantConversation),
            "manual" => Ok(Self::Manual),
            "structured_import" => Ok(Self::StructuredImport),
            _ => Err(StoreError::InvalidEnum(
                "timeline_source_kind",
                value.into(),
            )),
        }
    }
}

fn sensitivity_as_str(value: Sensitivity) -> &'static str {
    match value {
        Sensitivity::Public => "public",
        Sensitivity::Private => "private",
        Sensitivity::Sensitive => "sensitive",
    }
}

fn parse_sensitivity(value: &str) -> Result<Sensitivity, StoreError> {
    match value {
        "public" => Ok(Sensitivity::Public),
        "private" => Ok(Sensitivity::Private),
        "sensitive" => Ok(Sensitivity::Sensitive),
        _ => Err(StoreError::InvalidEnum("sensitivity", value.into())),
    }
}

impl HistoryMediaKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Audio => "audio",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "text" => Ok(Self::Text),
            "audio" => Ok(Self::Audio),
            _ => Err(StoreError::InvalidEnum("history_media_kind", value.into())),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioAssetStatus {
    Staged,
    Ready,
    Deleting,
    Orphaned,
    Failed,
    Deleted,
}

impl AudioAssetStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Ready => "ready",
            Self::Deleting => "deleting",
            Self::Orphaned => "orphaned",
            Self::Failed => "failed",
            Self::Deleted => "deleted",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "staged" => Ok(Self::Staged),
            "ready" => Ok(Self::Ready),
            "deleting" => Ok(Self::Deleting),
            "orphaned" => Ok(Self::Orphaned),
            "failed" => Ok(Self::Failed),
            "deleted" => Ok(Self::Deleted),
            _ => Err(StoreError::InvalidEnum("audio_asset_status", value.into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioAssetInput {
    pub id: String,
    pub storage_path: Option<PathBuf>,
    pub object_key: Option<String>,
    pub byte_size: u64,
    pub duration_ms: u64,
    pub status: AudioAssetStatus,
}

#[derive(Clone, Debug)]
pub struct TimelineEntryInput {
    pub id: String,
    pub idempotency_key: String,
    pub media_kind: HistoryMediaKind,
    pub source_kind: TimelineSourceKind,
    pub bundle_identifier: Option<String>,
    pub app_display_name: Option<String>,
    pub redacted_window_title: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub text_content: Option<String>,
    pub capture_sequence: Option<i64>,
    pub ax_sequence: Option<i64>,
    pub sensitivity: Sensitivity,
    pub created_at_ms: i64,
    pub audio_asset: Option<AudioAssetInput>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioAsset {
    pub id: String,
    pub storage_path: Option<PathBuf>,
    pub object_key: Option<String>,
    pub byte_size: u64,
    pub duration_ms: u64,
    pub status: AudioAssetStatus,
    pub recovery_attempts: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub deleted_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineEntry {
    pub id: String,
    pub idempotency_key: String,
    pub media_kind: HistoryMediaKind,
    pub source_kind: TimelineSourceKind,
    pub bundle_identifier: Option<String>,
    pub app_display_name: Option<String>,
    pub redacted_window_title: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub text_content: Option<String>,
    pub capture_sequence: Option<i64>,
    pub ax_sequence: Option<i64>,
    pub sensitivity: Sensitivity,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub deleted_at_ms: Option<i64>,
    pub audio_asset: Option<AudioAsset>,
}

#[derive(Clone, Debug)]
pub struct HistoryAdmission {
    pub entry: TimelineEntry,
    pub was_already_present: bool,
}

#[derive(Clone, Debug)]
pub struct TimelineSearchFilter {
    pub query: Option<String>,
    pub media_kind: Option<HistoryMediaKind>,
    pub bundle_identifier: Option<String>,
    pub source_kind: Option<TimelineSourceKind>,
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub include_deleted: bool,
    pub limit: usize,
}

impl Default for TimelineSearchFilter {
    fn default() -> Self {
        Self {
            query: None,
            media_kind: None,
            bundle_identifier: None,
            source_kind: None,
            from_ms: None,
            until_ms: None,
            include_deleted: false,
            limit: 50,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HistoryRetentionPolicy {
    pub max_age_ms: Option<i64>,
    pub max_audio_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HistoryRetentionReport {
    pub entries_deleted_by_age: usize,
    pub entries_deleted_by_quota: usize,
    pub audio_bytes_scheduled: u64,
    pub remaining_audio_bytes: u64,
}

impl MemoryStore {
    /// Atomically admits a text or audio timeline entry and its metadata-only audio asset.
    /// Retrying the same idempotency key returns the original entry without another insert.
    pub fn admit_timeline_entry(
        &mut self,
        input: &TimelineEntryInput,
    ) -> Result<HistoryAdmission, StoreError> {
        let (storage_path, object_key, byte_size, duration_ms) = validate_input(input)?;
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        if let Some((row_id, existing_id)) = tx
            .query_row(
                "SELECT row_id, id FROM timeline_entries WHERE idempotency_key=?1",
                [input.idempotency_key.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if existing_id != input.id
                || !timeline_matches_input(
                    &tx,
                    row_id,
                    input,
                    storage_path.as_deref(),
                    object_key.as_deref(),
                    byte_size,
                    duration_ms,
                )?
            {
                return Err(StoreError::HistoryIdempotencyConflict);
            }
            tx.commit()?;
            let entry = self
                .timeline_entry_by_row_id(row_id)?
                .ok_or(StoreError::Invariant("admitted history entry disappeared"))?;
            return Ok(HistoryAdmission {
                entry,
                was_already_present: true,
            });
        }

        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM timeline_entries WHERE id=?1)",
            [input.id.as_str()],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(StoreError::HistoryEntryConflict);
        }

        tx.execute(
            "INSERT INTO timeline_entries(
                id, idempotency_key, media_kind, source_kind, bundle_identifier,
                app_display_name, redacted_window_title, started_at_ms, ended_at_ms,
                text_content, capture_sequence, ax_sequence, sensitivity,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
            rusqlite::params![
                input.id,
                input.idempotency_key,
                input.media_kind.as_str(),
                input.source_kind.as_str(),
                input.bundle_identifier,
                input.app_display_name,
                input.redacted_window_title,
                input.started_at_ms,
                input.ended_at_ms,
                input.text_content,
                input.capture_sequence,
                input.ax_sequence,
                sensitivity_as_str(input.sensitivity),
                input.created_at_ms,
            ],
        )?;
        let row_id = tx.last_insert_rowid();
        if let Some(audio) = &input.audio_asset {
            tx.execute(
                "INSERT INTO audio_assets(
                    id, timeline_entry_id, storage_path, object_key, byte_size,
                    duration_ms, status, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                rusqlite::params![
                    audio.id,
                    row_id,
                    storage_path,
                    object_key,
                    byte_size,
                    duration_ms,
                    audio.status.as_str(),
                    input.created_at_ms,
                ],
            )?;
        }
        tx.commit()?;
        let entry = self
            .timeline_entry_by_row_id(row_id)?
            .ok_or(StoreError::Invariant("new history entry disappeared"))?;
        Ok(HistoryAdmission {
            entry,
            was_already_present: false,
        })
    }

    pub fn get_timeline_entry(&self, id: &str) -> Result<Option<TimelineEntry>, StoreError> {
        let raw = self
            .connection()
            .query_row(
                "SELECT row_id, id, idempotency_key, media_kind, source_kind,
                        bundle_identifier, app_display_name, redacted_window_title,
                        started_at_ms, ended_at_ms, text_content, capture_sequence, ax_sequence,
                        sensitivity, created_at_ms, updated_at_ms, deleted_at_ms
                 FROM timeline_entries WHERE id=?1",
                [id],
                RawTimelineEntry::from_row,
            )
            .optional()?;
        raw.map(|raw| self.hydrate_timeline_entry(raw)).transpose()
    }

    pub fn search_timeline(
        &self,
        filter: &TimelineSearchFilter,
    ) -> Result<Vec<TimelineEntry>, StoreError> {
        validate_search_filter(filter)?;
        let fts_query = filter.query.as_deref().map(fts_query);
        let mut sql = String::from(
            "SELECT e.row_id, e.id, e.idempotency_key, e.media_kind, e.source_kind,
                    e.bundle_identifier, e.app_display_name, e.redacted_window_title,
                    e.started_at_ms, e.ended_at_ms, e.text_content,
                    e.capture_sequence, e.ax_sequence, e.sensitivity,
                    e.created_at_ms, e.updated_at_ms, e.deleted_at_ms
             FROM timeline_entries e",
        );
        if fts_query.is_some() {
            sql.push_str(" JOIN timeline_fts ON timeline_fts.rowid=e.row_id");
        }
        sql.push_str(" WHERE ");
        if !filter.include_deleted {
            sql.push_str("e.status='active' AND ");
        }
        if fts_query.is_some() {
            sql.push_str("timeline_fts MATCH :query AND ");
        }
        sql.push_str(
            "e.started_at_ms >= :from_ms AND e.started_at_ms <= :until_ms
             AND (:media_kind = '' OR e.media_kind = :media_kind)
             AND (:bundle_identifier = '' OR e.bundle_identifier = :bundle_identifier)
             AND (:source_kind = '' OR e.source_kind = :source_kind)
             ORDER BY e.started_at_ms DESC, e.row_id DESC LIMIT :limit",
        );

        let mut stmt = self.connection().prepare(&sql)?;
        bind_named(&mut stmt, ":query", fts_query.as_deref())?;
        bind_named(&mut stmt, ":from_ms", filter.from_ms.unwrap_or(i64::MIN))?;
        bind_named(&mut stmt, ":until_ms", filter.until_ms.unwrap_or(i64::MAX))?;
        bind_named(
            &mut stmt,
            ":media_kind",
            filter.media_kind.map_or("", HistoryMediaKind::as_str),
        )?;
        bind_named(
            &mut stmt,
            ":bundle_identifier",
            filter.bundle_identifier.as_deref().unwrap_or(""),
        )?;
        bind_named(
            &mut stmt,
            ":source_kind",
            filter.source_kind.map_or("", TimelineSourceKind::as_str),
        )?;
        bind_named(&mut stmt, ":limit", filter.limit as i64)?;
        let mut rows = stmt.raw_query();
        let mut raw_entries = Vec::new();
        while let Some(row) = rows.next()? {
            raw_entries.push(RawTimelineEntry::from_row(row)?);
        }
        drop(rows);
        drop(stmt);
        raw_entries
            .into_iter()
            .map(|raw| self.hydrate_timeline_entry(raw))
            .collect()
    }

    pub fn delete_timeline_entry(&mut self, id: &str, now_ms: i64) -> Result<(), StoreError> {
        let tx = self.connection_mut().transaction()?;
        let row_id: Option<i64> = tx
            .query_row(
                "SELECT row_id FROM timeline_entries WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let row_id = row_id.ok_or(StoreError::NotFound)?;
        tx.execute(
            "UPDATE timeline_entries SET status='deleted', deleted_at_ms=?2, updated_at_ms=?2
             WHERE row_id=?1 AND status='active'",
            rusqlite::params![row_id, now_ms],
        )?;
        tx.execute(
            "UPDATE audio_assets SET status='deleting', lease_owner=NULL,
                    lease_expires_at_ms=NULL, last_error=NULL, updated_at_ms=?2
             WHERE timeline_entry_id=?1 AND status NOT IN ('deleted', 'deleting')",
            rusqlite::params![row_id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_audio_asset_ready(
        &mut self,
        asset_id: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let changed = self.connection_mut().execute(
            "UPDATE audio_assets SET status='ready', lease_owner=NULL,
                    lease_expires_at_ms=NULL, last_error=NULL, updated_at_ms=?2
             WHERE id=?1 AND status IN ('staged', 'orphaned')",
            rusqlite::params![asset_id, now_ms],
        )?;
        if changed == 1 {
            return Ok(());
        }
        let status: Option<String> = self
            .connection()
            .query_row(
                "SELECT status FROM audio_assets WHERE id=?1",
                [asset_id],
                |row| row.get(0),
            )
            .optional()?;
        match status.as_deref() {
            Some("ready") => Ok(()),
            Some(_) => Err(StoreError::InvalidInput("audio asset is not readyable")),
            None => Err(StoreError::NotFound),
        }
    }

    pub fn recover_stale_audio_assets(
        &mut self,
        now_ms: i64,
        stale_after_ms: i64,
    ) -> Result<usize, StoreError> {
        if stale_after_ms < 0 {
            return Err(StoreError::InvalidInput(
                "audio recovery grace period is negative",
            ));
        }
        let cutoff = now_ms.saturating_sub(stale_after_ms);
        Ok(self.connection_mut().execute(
            "UPDATE audio_assets SET status='orphaned', lease_owner=NULL,
                    lease_expires_at_ms=NULL, updated_at_ms=?1,
                    last_error=COALESCE(last_error, 'stale audio asset requires recovery')
             WHERE status IN ('staged', 'deleting') AND updated_at_ms<=?2
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms<=?1)",
            rusqlite::params![now_ms, cutoff],
        )?)
    }

    pub fn claim_audio_asset_recovery(
        &mut self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<AudioAsset>, StoreError> {
        if owner.trim().is_empty() || owner.chars().count() > 128 || lease_ms <= 0 {
            return Err(StoreError::InvalidInput("invalid audio recovery lease"));
        }
        if !(1..=100).contains(&limit) {
            return Err(StoreError::InvalidInput(
                "audio recovery limit must be 1..=100",
            ));
        }
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut stmt = tx.prepare(
            "SELECT id FROM audio_assets
             WHERE status IN ('orphaned', 'failed')
                OR (status='deleting' AND lease_expires_at_ms<=?1)
             ORDER BY updated_at_ms, id LIMIT ?2",
        )?;
        let ids = stmt
            .query_map(rusqlite::params![now_ms, limit as i64], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let lease_expires_at_ms = now_ms.saturating_add(lease_ms);
        let mut assets = Vec::with_capacity(ids.len());
        for id in ids {
            tx.execute(
                "UPDATE audio_assets SET status='deleting', recovery_attempts=recovery_attempts+1,
                        lease_owner=?2, lease_expires_at_ms=?3, updated_at_ms=?4
                 WHERE id=?1",
                rusqlite::params![id, owner, lease_expires_at_ms, now_ms],
            )?;
            assets.push(
                load_audio_asset_tx(&tx, &id)?
                    .ok_or(StoreError::Invariant("claimed audio asset disappeared"))?,
            );
        }
        tx.commit()?;
        Ok(assets)
    }

    pub fn finalize_audio_asset_deletion(
        &mut self,
        asset_id: &str,
        owner: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let tx = self.connection_mut().transaction()?;
        let lease: Option<(Option<String>, Option<i64>, String)> = tx
            .query_row(
                "SELECT lease_owner, lease_expires_at_ms, status FROM audio_assets WHERE id=?1",
                [asset_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((lease_owner, lease_expires_at_ms, status)) = lease else {
            return Err(StoreError::NotFound);
        };
        if status == "deleted" {
            tx.commit()?;
            return Ok(());
        }
        if status != "deleting"
            || lease_owner.as_deref() != Some(owner)
            || lease_expires_at_ms.is_none_or(|expires| expires <= now_ms)
        {
            return Err(StoreError::AudioRecoveryLeaseLost);
        }
        tx.execute(
            "UPDATE audio_assets SET status='deleted', deleted_at_ms=?2,
                    lease_owner=NULL, lease_expires_at_ms=NULL, last_error=NULL, updated_at_ms=?2
             WHERE id=?1",
            rusqlite::params![asset_id, now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn fail_audio_asset_recovery(
        &mut self,
        asset_id: &str,
        owner: &str,
        error: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        if error.trim().is_empty() || error.chars().count() > MAX_AUDIO_RECOVERY_ERROR_CHARS {
            return Err(StoreError::InvalidInput("invalid audio recovery error"));
        }
        let changed = self.connection_mut().execute(
            "UPDATE audio_assets SET status='failed', last_error=?3,
                    lease_owner=NULL, lease_expires_at_ms=NULL, updated_at_ms=?4
             WHERE id=?1 AND status='deleting' AND lease_owner=?2
               AND lease_expires_at_ms>?4",
            rusqlite::params![asset_id, owner, error, now_ms],
        )?;
        match changed {
            1 => Ok(()),
            _ => Err(StoreError::AudioRecoveryLeaseLost),
        }
    }

    pub fn apply_history_retention(
        &mut self,
        policy: &HistoryRetentionPolicy,
        now_ms: i64,
    ) -> Result<HistoryRetentionReport, StoreError> {
        validate_retention_policy(policy)?;
        let tx = self.connection_mut().transaction()?;
        let mut report = HistoryRetentionReport::default();

        if let Some(max_age_ms) = policy.max_age_ms {
            let cutoff = now_ms.saturating_sub(max_age_ms);
            let row_ids = entry_ids_before(&tx, cutoff)?;
            for row_id in row_ids {
                let bytes = schedule_entry_delete_tx(&tx, row_id, now_ms)?;
                report.entries_deleted_by_age += 1;
                report.audio_bytes_scheduled = report.audio_bytes_scheduled.saturating_add(bytes);
            }
        }

        if let Some(max_audio_bytes) = policy.max_audio_bytes {
            let mut remaining = audio_bytes_in_use(&tx)?;
            if remaining > max_audio_bytes {
                for (row_id, bytes) in active_audio_entries(&tx)? {
                    if remaining <= max_audio_bytes {
                        break;
                    }
                    let scheduled = schedule_entry_delete_tx(&tx, row_id, now_ms)?;
                    let bytes = u64::try_from(bytes)
                        .map_err(|_| StoreError::Invariant("negative audio bytes persisted"))?;
                    remaining = remaining.saturating_sub(bytes);
                    report.entries_deleted_by_quota += 1;
                    report.audio_bytes_scheduled =
                        report.audio_bytes_scheduled.saturating_add(scheduled);
                }
            }
        }
        report.remaining_audio_bytes = audio_bytes_in_use(&tx)?;
        tx.commit()?;
        Ok(report)
    }

    fn timeline_entry_by_row_id(&self, row_id: i64) -> Result<Option<TimelineEntry>, StoreError> {
        let raw = self
            .connection()
            .query_row(
                "SELECT row_id, id, idempotency_key, media_kind, source_kind,
                        bundle_identifier, app_display_name, redacted_window_title,
                        started_at_ms, ended_at_ms, text_content, capture_sequence, ax_sequence,
                        sensitivity, created_at_ms, updated_at_ms, deleted_at_ms
                 FROM timeline_entries WHERE row_id=?1",
                [row_id],
                RawTimelineEntry::from_row,
            )
            .optional()?;
        raw.map(|raw| self.hydrate_timeline_entry(raw)).transpose()
    }

    fn hydrate_timeline_entry(&self, raw: RawTimelineEntry) -> Result<TimelineEntry, StoreError> {
        let media_kind = HistoryMediaKind::parse(&raw.media_kind)?;
        let source_kind = TimelineSourceKind::parse(&raw.source_kind)?;
        let sensitivity = parse_sensitivity(&raw.sensitivity)?;
        let audio_asset = load_audio_asset(self.connection(), raw.row_id)?;
        Ok(TimelineEntry {
            id: raw.id,
            idempotency_key: raw.idempotency_key,
            media_kind,
            source_kind,
            bundle_identifier: raw.bundle_identifier,
            app_display_name: raw.app_display_name,
            redacted_window_title: raw.redacted_window_title,
            started_at_ms: raw.started_at_ms,
            ended_at_ms: raw.ended_at_ms,
            text_content: raw.text_content,
            capture_sequence: raw.capture_sequence,
            ax_sequence: raw.ax_sequence,
            sensitivity,
            created_at_ms: raw.created_at_ms,
            updated_at_ms: raw.updated_at_ms,
            deleted_at_ms: raw.deleted_at_ms,
            audio_asset,
        })
    }
}

struct RawTimelineEntry {
    row_id: i64,
    id: String,
    idempotency_key: String,
    media_kind: String,
    source_kind: String,
    bundle_identifier: Option<String>,
    app_display_name: Option<String>,
    redacted_window_title: Option<String>,
    started_at_ms: i64,
    ended_at_ms: i64,
    text_content: Option<String>,
    capture_sequence: Option<i64>,
    ax_sequence: Option<i64>,
    sensitivity: String,
    created_at_ms: i64,
    updated_at_ms: i64,
    deleted_at_ms: Option<i64>,
}

impl RawTimelineEntry {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            row_id: row.get(0)?,
            id: row.get(1)?,
            idempotency_key: row.get(2)?,
            media_kind: row.get(3)?,
            source_kind: row.get(4)?,
            bundle_identifier: row.get(5)?,
            app_display_name: row.get(6)?,
            redacted_window_title: row.get(7)?,
            started_at_ms: row.get(8)?,
            ended_at_ms: row.get(9)?,
            text_content: row.get(10)?,
            capture_sequence: row.get(11)?,
            ax_sequence: row.get(12)?,
            sensitivity: row.get(13)?,
            created_at_ms: row.get(14)?,
            updated_at_ms: row.get(15)?,
            deleted_at_ms: row.get(16)?,
        })
    }
}

struct RawAudioAsset {
    id: String,
    storage_path: Option<String>,
    object_key: Option<String>,
    byte_size: i64,
    duration_ms: i64,
    status: String,
    recovery_attempts: i64,
    lease_owner: Option<String>,
    lease_expires_at_ms: Option<i64>,
    last_error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    deleted_at_ms: Option<i64>,
}

type AudioAdmissionFingerprint = (String, Option<String>, Option<String>, i64, i64);

impl RawAudioAsset {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            storage_path: row.get(1)?,
            object_key: row.get(2)?,
            byte_size: row.get(3)?,
            duration_ms: row.get(4)?,
            status: row.get(5)?,
            recovery_attempts: row.get(6)?,
            lease_owner: row.get(7)?,
            lease_expires_at_ms: row.get(8)?,
            last_error: row.get(9)?,
            created_at_ms: row.get(10)?,
            updated_at_ms: row.get(11)?,
            deleted_at_ms: row.get(12)?,
        })
    }

    fn into_asset(self) -> Result<AudioAsset, StoreError> {
        Ok(AudioAsset {
            id: self.id,
            storage_path: self.storage_path.map(PathBuf::from),
            object_key: self.object_key,
            byte_size: u64::try_from(self.byte_size)
                .map_err(|_| StoreError::Invariant("negative audio byte size persisted"))?,
            duration_ms: u64::try_from(self.duration_ms)
                .map_err(|_| StoreError::Invariant("negative audio duration persisted"))?,
            status: AudioAssetStatus::parse(&self.status)?,
            recovery_attempts: u32::try_from(self.recovery_attempts)
                .map_err(|_| StoreError::Invariant("invalid audio recovery attempts persisted"))?,
            lease_owner: self.lease_owner,
            lease_expires_at_ms: self.lease_expires_at_ms,
            last_error: self.last_error,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            deleted_at_ms: self.deleted_at_ms,
        })
    }
}

fn validate_input(
    input: &TimelineEntryInput,
) -> Result<(Option<String>, Option<String>, i64, i64), StoreError> {
    if input.id.trim().is_empty()
        || input.id.chars().count() > MAX_TIMELINE_ID_CHARS
        || input.idempotency_key.trim().is_empty()
        || input.idempotency_key.chars().count() > MAX_TIMELINE_ID_CHARS
    {
        return Err(StoreError::InvalidInput(
            "history ID or idempotency key is invalid",
        ));
    }
    if input.ended_at_ms < input.started_at_ms {
        return Err(StoreError::InvalidInput("timeline end precedes start"));
    }
    if input
        .bundle_identifier
        .as_ref()
        .is_some_and(|value| value.chars().count() > 512 || value.contains('\0'))
        || input
            .app_display_name
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500 || value.contains('\0'))
        || input
            .redacted_window_title
            .as_ref()
            .is_some_and(|title| title.chars().count() > 500 || title.contains('\0'))
        || input
            .text_content
            .as_ref()
            .is_some_and(|text| text.chars().count() > MAX_TIMELINE_TEXT_CHARS)
        || input
            .redacted_window_title
            .as_ref()
            .is_some_and(|value| value.contains('\0'))
        || input
            .text_content
            .as_ref()
            .is_some_and(|value| value.contains('\0'))
    {
        return Err(StoreError::InvalidInput(
            "timeline provenance or text is invalid",
        ));
    }
    if input.capture_sequence.is_some_and(|value| value < 0)
        || input.ax_sequence.is_some_and(|value| value < 0)
    {
        return Err(StoreError::InvalidInput("timeline sequence is negative"));
    }
    match input.media_kind {
        HistoryMediaKind::Text
            if input
                .text_content
                .as_deref()
                .is_none_or(|text| text.trim().is_empty()) =>
        {
            return Err(StoreError::InvalidInput(
                "text timeline entry requires text",
            ));
        }
        HistoryMediaKind::Text if input.audio_asset.is_some() => {
            return Err(StoreError::InvalidInput(
                "text timeline entry cannot have audio metadata",
            ));
        }
        HistoryMediaKind::Audio if input.audio_asset.is_none() => {
            return Err(StoreError::InvalidInput(
                "audio timeline entry requires audio metadata",
            ));
        }
        HistoryMediaKind::Text => {}
        HistoryMediaKind::Audio => {}
    }

    let Some(audio) = input.audio_asset.as_ref() else {
        return Ok((None, None, 0, 0));
    };
    if audio.id.trim().is_empty() || audio.id.chars().count() > MAX_TIMELINE_ID_CHARS {
        return Err(StoreError::InvalidInput("audio asset ID is invalid"));
    }
    if !matches!(
        audio.status,
        AudioAssetStatus::Staged | AudioAssetStatus::Ready
    ) {
        return Err(StoreError::InvalidInput(
            "new audio asset must be staged or ready",
        ));
    }
    let storage_path = audio
        .storage_path
        .as_ref()
        .map(|path| {
            path.to_str()
                .ok_or(StoreError::InvalidInput("audio path is not UTF-8"))
        })
        .transpose()?
        .map(str::to_owned);
    let object_key = audio.object_key.clone();
    if storage_path.is_none() && object_key.is_none() {
        return Err(StoreError::InvalidInput(
            "audio asset needs a path or object key",
        ));
    }
    if let Some(path) = storage_path.as_deref() {
        validate_audio_location(path, "audio storage path")?;
    }
    if let Some(key) = object_key.as_deref() {
        validate_audio_location(key, "audio object key")?;
    }
    let byte_size = i64::try_from(audio.byte_size)
        .map_err(|_| StoreError::InvalidInput("audio byte size is too large"))?;
    let duration_ms = i64::try_from(audio.duration_ms)
        .map_err(|_| StoreError::InvalidInput("audio duration is too large"))?;
    Ok((storage_path, object_key, byte_size, duration_ms))
}

fn validate_audio_location(value: &str, field: &'static str) -> Result<(), StoreError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || value.contains('\0')
        || normalized.chars().count() > 4096
        || normalized.contains("screenshot")
        || normalized.contains("screen_capture")
        || normalized
            .rsplit_once('.')
            .is_some_and(|(_, extension)| IMAGE_EXTENSIONS.contains(&extension))
    {
        return Err(StoreError::InvalidInput(field));
    }
    Ok(())
}

fn timeline_matches_input(
    tx: &Transaction<'_>,
    row_id: i64,
    input: &TimelineEntryInput,
    storage_path: Option<&str>,
    object_key: Option<&str>,
    byte_size: i64,
    duration_ms: i64,
) -> Result<bool, StoreError> {
    let existing = tx.query_row(
        "SELECT id, media_kind, source_kind, bundle_identifier, app_display_name,
                redacted_window_title, started_at_ms, ended_at_ms, text_content,
                capture_sequence, ax_sequence, sensitivity
         FROM timeline_entries WHERE row_id=?1",
        [row_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, String>(11)?,
            ))
        },
    )?;
    if existing
        != (
            input.id.clone(),
            input.media_kind.as_str().into(),
            input.source_kind.as_str().into(),
            input.bundle_identifier.clone(),
            input.app_display_name.clone(),
            input.redacted_window_title.clone(),
            input.started_at_ms,
            input.ended_at_ms,
            input.text_content.clone(),
            input.capture_sequence,
            input.ax_sequence,
            sensitivity_as_str(input.sensitivity).into(),
        )
    {
        return Ok(false);
    }
    let existing_audio: Option<AudioAdmissionFingerprint> = tx
        .query_row(
            "SELECT id, storage_path, object_key, byte_size, duration_ms
             FROM audio_assets WHERE timeline_entry_id=?1",
            [row_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    Ok(match input.audio_asset.as_ref() {
        Some(audio) => existing_audio.is_some_and(|value| {
            value
                == (
                    audio.id.clone(),
                    storage_path.map(str::to_owned),
                    object_key.map(str::to_owned),
                    byte_size,
                    duration_ms,
                )
        }),
        None => existing_audio.is_none(),
    })
}

fn validate_search_filter(filter: &TimelineSearchFilter) -> Result<(), StoreError> {
    if !(1..=200).contains(&filter.limit) {
        return Err(StoreError::InvalidInput(
            "timeline search limit must be 1..=200",
        ));
    }
    if filter
        .query
        .as_ref()
        .is_some_and(|query| query.trim().is_empty() || query.chars().count() > 500)
    {
        return Err(StoreError::InvalidInput(
            "timeline search query must be 1..=500 characters",
        ));
    }
    if filter
        .bundle_identifier
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 512)
    {
        return Err(StoreError::InvalidInput("timeline app filter is invalid"));
    }
    if let (Some(from), Some(until)) = (filter.from_ms, filter.until_ms)
        && until < from
    {
        return Err(StoreError::InvalidInput(
            "timeline search date range is reversed",
        ));
    }
    Ok(())
}

fn validate_retention_policy(policy: &HistoryRetentionPolicy) -> Result<(), StoreError> {
    if policy.max_age_ms.is_none() && policy.max_audio_bytes.is_none() {
        return Err(StoreError::InvalidInput(
            "history retention policy is empty",
        ));
    }
    if policy.max_age_ms.is_some_and(|value| value < 0) {
        return Err(StoreError::InvalidInput(
            "history retention age is negative",
        ));
    }
    Ok(())
}

fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn bind_named<T: rusqlite::ToSql>(
    statement: &mut rusqlite::Statement<'_>,
    name: &str,
    value: T,
) -> Result<(), StoreError> {
    if let Some(index) = statement.parameter_index(name)? {
        statement.raw_bind_parameter(index, value)?;
    }
    Ok(())
}

fn load_audio_asset(
    connection: &rusqlite::Connection,
    row_id: i64,
) -> Result<Option<AudioAsset>, StoreError> {
    connection
        .query_row(
            "SELECT id, storage_path, object_key, byte_size, duration_ms, status,
                    recovery_attempts, lease_owner, lease_expires_at_ms, last_error,
                    created_at_ms, updated_at_ms, deleted_at_ms
             FROM audio_assets WHERE timeline_entry_id=?1",
            [row_id],
            RawAudioAsset::from_row,
        )
        .optional()?
        .map(RawAudioAsset::into_asset)
        .transpose()
}

fn load_audio_asset_tx(
    tx: &Transaction<'_>,
    asset_id: &str,
) -> Result<Option<AudioAsset>, StoreError> {
    tx.query_row(
        "SELECT id, storage_path, object_key, byte_size, duration_ms, status,
                recovery_attempts, lease_owner, lease_expires_at_ms, last_error,
                created_at_ms, updated_at_ms, deleted_at_ms
         FROM audio_assets WHERE id=?1",
        [asset_id],
        RawAudioAsset::from_row,
    )
    .optional()?
    .map(RawAudioAsset::into_asset)
    .transpose()
}

fn entry_ids_before(tx: &Transaction<'_>, cutoff: i64) -> Result<Vec<i64>, StoreError> {
    let mut stmt = tx.prepare(
        "SELECT row_id FROM timeline_entries
         WHERE status='active' AND started_at_ms<?1
         ORDER BY started_at_ms, row_id",
    )?;
    Ok(stmt
        .query_map([cutoff], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?)
}

fn active_audio_entries(tx: &Transaction<'_>) -> Result<Vec<(i64, i64)>, StoreError> {
    let mut stmt = tx.prepare(
        "SELECT e.row_id, a.byte_size FROM timeline_entries e
         JOIN audio_assets a ON a.timeline_entry_id=e.row_id
         WHERE e.status='active' AND a.status<>'deleted'
         ORDER BY e.started_at_ms, e.row_id",
    )?;
    Ok(stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<(i64, i64)>, _>>()?)
}

fn audio_bytes_in_use(tx: &Transaction<'_>) -> Result<u64, StoreError> {
    let bytes: i64 = tx.query_row(
        "SELECT COALESCE(SUM(byte_size), 0) FROM audio_assets WHERE status<>'deleted'",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(bytes).map_err(|_| StoreError::Invariant("negative audio quota sum persisted"))
}

fn schedule_entry_delete_tx(
    tx: &Transaction<'_>,
    row_id: i64,
    now_ms: i64,
) -> Result<u64, StoreError> {
    tx.execute(
        "UPDATE timeline_entries SET status='deleted', deleted_at_ms=?2, updated_at_ms=?2
         WHERE row_id=?1 AND status='active'",
        rusqlite::params![row_id, now_ms],
    )?;
    let bytes: i64 = tx.query_row(
        "SELECT COALESCE(SUM(byte_size), 0) FROM audio_assets
         WHERE timeline_entry_id=?1 AND status<>'deleted'",
        [row_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "UPDATE audio_assets SET status='deleting', lease_owner=NULL,
                lease_expires_at_ms=NULL, last_error=NULL, updated_at_ms=?2
         WHERE timeline_entry_id=?1 AND status NOT IN ('deleted', 'deleting')",
        rusqlite::params![row_id, now_ms],
    )?;
    u64::try_from(bytes).map_err(|_| StoreError::Invariant("negative audio bytes persisted"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryStore, MemoryStoreConfig};

    fn text(id: &str, key: &str, started_at_ms: i64, value: &str) -> TimelineEntryInput {
        TimelineEntryInput {
            id: id.into(),
            idempotency_key: key.into(),
            media_kind: HistoryMediaKind::Text,
            source_kind: TimelineSourceKind::Accessibility,
            bundle_identifier: Some("com.example.app".into()),
            app_display_name: Some("Example".into()),
            redacted_window_title: Some("Timeline".into()),
            started_at_ms,
            ended_at_ms: started_at_ms + 10,
            text_content: Some(value.into()),
            capture_sequence: Some(started_at_ms),
            ax_sequence: Some(started_at_ms + 1),
            sensitivity: Sensitivity::Private,
            created_at_ms: started_at_ms,
            audio_asset: None,
        }
    }

    fn audio(id: &str, key: &str, started_at_ms: i64, bytes: u64) -> TimelineEntryInput {
        TimelineEntryInput {
            id: id.into(),
            idempotency_key: key.into(),
            media_kind: HistoryMediaKind::Audio,
            source_kind: TimelineSourceKind::AudioTranscript,
            bundle_identifier: None,
            app_display_name: None,
            redacted_window_title: None,
            started_at_ms,
            ended_at_ms: started_at_ms + 10,
            text_content: Some("spoken project update".into()),
            capture_sequence: None,
            ax_sequence: None,
            sensitivity: Sensitivity::Sensitive,
            created_at_ms: started_at_ms,
            audio_asset: Some(AudioAssetInput {
                id: format!("asset-{id}"),
                storage_path: Some(PathBuf::from(format!("/audio/{id}.m4a"))),
                object_key: None,
                byte_size: bytes,
                duration_ms: 1_000,
                status: AudioAssetStatus::Ready,
            }),
        }
    }

    #[test]
    fn migration_creates_metadata_only_history_tables() {
        let store = MemoryStore::in_memory().unwrap();
        let tables: Vec<String> = store
            .connection()
            .prepare(
                "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')
                 AND name IN ('timeline_entries', 'audio_assets', 'timeline_fts') ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            tables,
            vec!["audio_assets", "timeline_entries", "timeline_fts"]
        );
        let columns: Vec<String> = store
            .connection()
            .prepare("PRAGMA table_info(audio_assets)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!columns.iter().any(|column| column.contains("blob")));
    }

    #[test]
    fn admission_is_atomic_and_idempotent() {
        let mut store = MemoryStore::in_memory().unwrap();
        let input = audio("audio-1", "admission-1", 100, 42);
        let first = store.admit_timeline_entry(&input).unwrap();
        let retry = store.admit_timeline_entry(&input).unwrap();
        assert!(!first.was_already_present);
        assert!(retry.was_already_present);
        assert_eq!(first.entry, retry.entry);
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM timeline_entries", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );

        let mut conflicting = input.clone();
        conflicting.text_content = Some("different transcript".into());
        assert!(matches!(
            store.admit_timeline_entry(&conflicting),
            Err(StoreError::HistoryIdempotencyConflict)
        ));

        let mut invalid = audio("audio-2", "admission-2", 200, 42);
        invalid.audio_asset = None;
        assert!(store.admit_timeline_entry(&invalid).is_err());
        assert_eq!(
            store
                .connection()
                .query_row("SELECT count(*) FROM timeline_entries", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn search_uses_fts_and_rejects_image_media() {
        let mut store = MemoryStore::in_memory().unwrap();
        store
            .admit_timeline_entry(&text(
                "text-1",
                "text-key-1",
                20,
                "Discussed the encrypted audio archive.",
            ))
            .unwrap();
        store
            .admit_timeline_entry(&text(
                "text-2",
                "text-key-2",
                10,
                "Discussed a different project.",
            ))
            .unwrap();
        let found = store
            .search_timeline(&TimelineSearchFilter {
                query: Some("encrypted archive".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            found
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["text-1"]
        );

        let image = audio("image-1", "image-key-1", 30, 10);
        let mut image_path = image;
        image_path.audio_asset.as_mut().unwrap().storage_path =
            Some(PathBuf::from("/tmp/frame.png"));
        assert!(matches!(
            store.admit_timeline_entry(&image_path),
            Err(StoreError::InvalidInput("audio storage path"))
        ));
        assert!(
            store
                .connection()
                .execute(
                    "INSERT INTO timeline_entries(id, idempotency_key, media_kind, started_at_ms,
                 ended_at_ms, text_content, created_at_ms, updated_at_ms)
                 VALUES ('image', 'image', 'image', 1, 1, 'raw', 1, 1)",
                    [],
                )
                .is_err()
        );
    }

    #[test]
    fn provenance_round_trips_and_filters_by_app_source_and_time() {
        let mut store = MemoryStore::in_memory().unwrap();
        let mut input = text("ocr-1", "ocr-key-1", 500, "Redacted OCR timeline text");
        input.source_kind = TimelineSourceKind::Ocr;
        input.bundle_identifier = Some("com.example.editor".into());
        input.app_display_name = Some("Editor".into());
        input.redacted_window_title = Some("Document — redacted".into());
        input.capture_sequence = None;
        input.ax_sequence = Some(77);
        input.sensitivity = Sensitivity::Sensitive;
        store.admit_timeline_entry(&input).unwrap();

        let found = store
            .search_timeline(&TimelineSearchFilter {
                bundle_identifier: Some("com.example.editor".into()),
                source_kind: Some(TimelineSourceKind::Ocr),
                from_ms: Some(400),
                until_ms: Some(600),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_kind, TimelineSourceKind::Ocr);
        assert_eq!(found[0].app_display_name.as_deref(), Some("Editor"));
        assert_eq!(
            found[0].redacted_window_title.as_deref(),
            Some("Document — redacted")
        );
        assert_eq!(found[0].ax_sequence, Some(77));
        assert_eq!(found[0].sensitivity, Sensitivity::Sensitive);
    }

    #[test]
    fn retention_by_age_and_quota_schedules_audio_cleanup() {
        let mut store = MemoryStore::in_memory().unwrap();
        store
            .admit_timeline_entry(&audio("old", "old-key", 10, 40))
            .unwrap();
        store
            .admit_timeline_entry(&audio("new", "new-key", 100, 70))
            .unwrap();
        let report = store
            .apply_history_retention(
                &HistoryRetentionPolicy {
                    max_age_ms: Some(50),
                    max_audio_bytes: Some(50),
                },
                120,
            )
            .unwrap();
        assert_eq!(report.entries_deleted_by_age, 1);
        assert_eq!(report.entries_deleted_by_quota, 1);
        assert_eq!(report.audio_bytes_scheduled, 110);
        assert_eq!(
            store
                .search_timeline(&TimelineSearchFilter::default())
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            store
                .connection()
                .query_row(
                    "SELECT count(*) FROM audio_assets WHERE status='deleting'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn orphan_recovery_is_lease_checked_and_idempotent() {
        let mut store = MemoryStore::in_memory().unwrap();
        store
            .admit_timeline_entry(&audio("recover", "recover-key", 10, 5))
            .unwrap();
        store
            .connection()
            .execute(
                "UPDATE audio_assets SET status='staged', updated_at_ms=10 WHERE id='asset-recover'",
                [],
            )
            .unwrap();
        assert_eq!(store.recover_stale_audio_assets(100, 50).unwrap(), 1);
        let claimed = store
            .claim_audio_asset_recovery("worker-a", 101, 100, 10)
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(matches!(
            store.finalize_audio_asset_deletion("asset-recover", "worker-b", 102),
            Err(StoreError::AudioRecoveryLeaseLost)
        ));
        store
            .finalize_audio_asset_deletion("asset-recover", "worker-a", 102)
            .unwrap();
        store
            .finalize_audio_asset_deletion("asset-recover", "worker-a", 103)
            .unwrap();
        assert_eq!(
            store
                .get_timeline_entry("recover")
                .unwrap()
                .unwrap()
                .audio_asset
                .unwrap()
                .status,
            AudioAssetStatus::Deleted
        );
    }

    #[test]
    fn legacy_source_events_are_migrated_into_searchable_text_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(crate::MIGRATION_0001).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO source_events(
                    id, correlation_id, source_kind, started_at, ended_at, evidence_text,
                    content_hash, sensitivity, created_at
                 ) VALUES ('legacy-1', 'correlation', 'assistant_conversation', 1, 2,
                    'Legacy searchable transcript', zeroblob(32), 'private', 3)",
                [],
            )
            .unwrap();
        drop(connection);

        let store = MemoryStore::open(MemoryStoreConfig::unencrypted_test_at(path)).unwrap();
        let found = store
            .search_timeline(&TimelineSearchFilter {
                query: Some("searchable transcript".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "legacy-source:legacy-1");
    }
}
