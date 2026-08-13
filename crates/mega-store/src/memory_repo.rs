use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use mega_memory::{
    AppId, AssertionMode, EntityId, EntityReference, EntityRole, Memory, MemoryCandidate, MemoryId,
    MemoryMutationPlan, MemoryMutationResult, MemoryScope, MemoryStatus, MemoryType, ScopeId,
    ScopeType, Sensitivity, SourceEventId, inspect_private_content,
};

use crate::operations_repo::{
    DeleteMode, MemoryEventType, enqueue_projection_refresh_tx, ensure_revision, record_event_tx,
};
use crate::source_repo::sensitivity_str;

use crate::{MIGRATION_0001, MIGRATION_0002, MIGRATION_0003, MIGRATION_0004, TAXONOMY_VERSION};

const CURRENT_SCHEMA_VERSION: i64 = 4;

#[derive(Clone)]
pub struct MemoryStoreConfig {
    path: PathBuf,
    encryption_key: Option<Zeroizing<[u8; 32]>>,
    allow_unencrypted_for_tests: bool,
}

impl std::fmt::Debug for MemoryStoreConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryStoreConfig")
            .field("path", &self.path)
            .field(
                "encryption_key",
                &self.encryption_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "allow_unencrypted_for_tests",
                &self.allow_unencrypted_for_tests,
            )
            .finish()
    }
}

impl MemoryStoreConfig {
    pub fn encrypted(path: impl Into<PathBuf>, encryption_key: [u8; 32]) -> Self {
        Self {
            path: path.into(),
            encryption_key: Some(Zeroizing::new(encryption_key)),
            allow_unencrypted_for_tests: false,
        }
    }

    #[cfg(test)]
    fn unencrypted_test() -> Self {
        Self::unencrypted_test_at(PathBuf::from(":memory:"))
    }

    #[cfg(test)]
    pub(crate) fn unencrypted_test_at(path: PathBuf) -> Self {
        Self {
            path,
            encryption_key: None,
            allow_unencrypted_for_tests: true,
        }
    }
}

#[derive(Debug)]
pub struct MemoryStore {
    connection: Connection,
}

#[derive(Clone, Debug)]
pub struct ManualMemoryInput {
    pub content: String,
    pub memory_type: MemoryType,
    pub scope_type: ScopeType,
    pub scope_key: String,
    pub scope_display_name: String,
    pub category_slugs: Vec<String>,
    pub applicable_app_ids: Vec<AppId>,
    pub importance: f32,
    pub sensitivity: Sensitivity,
    pub valid_from_ms: Option<i64>,
    pub valid_until_ms: Option<i64>,
    pub now_ms: i64,
}

#[derive(Clone, Debug, Default)]
pub struct MemorySearchFilter {
    pub query: Option<String>,
    pub app_id: Option<AppId>,
    pub app_role: MemoryAppFilterRole,
    pub category_slug: Option<String>,
    pub scope_type: Option<ScopeType>,
    pub scope_key: Option<String>,
    pub entity_id: Option<EntityId>,
    pub memory_type: Option<MemoryType>,
    pub status: Option<MemoryStatus>,
    pub include_history: bool,
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub limit: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MemoryAppFilterRole {
    #[default]
    Any,
    AppliesTo,
}

impl MemorySearchFilter {
    fn validate(&self) -> Result<(), StoreError> {
        if !(1..=200).contains(&self.limit) {
            return Err(StoreError::InvalidInput(
                "memory search limit must be 1..=200",
            ));
        }
        if self
            .query
            .as_ref()
            .is_some_and(|query| query.trim().is_empty() || query.chars().count() > 500)
        {
            return Err(StoreError::InvalidInput(
                "memory search query must be 1..=500 characters",
            ));
        }
        if let (Some(from), Some(until)) = (self.from_ms, self.until_ms)
            && until < from
        {
            return Err(StoreError::InvalidInput(
                "memory search date range is reversed",
            ));
        }
        Ok(())
    }
}

impl MemoryStore {
    pub fn open(config: MemoryStoreConfig) -> Result<Self, StoreError> {
        if config.encryption_key.is_none() && !config.allow_unencrypted_for_tests {
            return Err(StoreError::EncryptionRequired);
        }
        prepare_parent(&config.path)?;
        let connection = Connection::open_with_flags(
            &config.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if let Some(key) = config.encryption_key.as_deref() {
            let encoded_key = Zeroizing::new(format!("x'{}'", encode_hex(key)));
            connection.pragma_update(None, "key", &**encoded_key)?;
            let cipher_version: Option<String> = connection
                .query_row("PRAGMA cipher_version", [], |row| row.get(0))
                .optional()?;
            if cipher_version.as_deref().is_none_or(str::is_empty) {
                return Err(StoreError::CipherUnavailable);
            }
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "secure_delete", "ON")?;
        connection.pragma_update(None, "temp_store", "MEMORY")?;
        connection.pragma_update(None, "wal_autocheckpoint", 1_000)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Self { connection };
        store.migrate()?;
        store.seed_taxonomy()?;
        restrict_permissions(&config.path)?;
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Self, StoreError> {
        Self::open(MemoryStoreConfig::unencrypted_test())
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub fn integrity_check(&self) -> Result<(), StoreError> {
        let result: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if result != "ok" {
            return Err(StoreError::Integrity(result));
        }
        self.connection.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('integrity-check')",
            [],
        )?;
        Ok(())
    }

    pub fn rebuild_search_index(&self) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
            [],
        )?;
        Ok(())
    }

    pub fn upsert_app(
        &self,
        app_id: &AppId,
        bundle_identifier: &str,
        display_name: &str,
        identity_confidence: f32,
        observed_at: i64,
    ) -> Result<(), StoreError> {
        if bundle_identifier.trim().is_empty()
            || !identity_confidence.is_finite()
            || !(0.0..=1.0).contains(&identity_confidence)
        {
            return Err(StoreError::InvalidInput("invalid app identity"));
        }
        self.connection.execute(
            "INSERT INTO apps(id, bundle_identifier, display_name, identity_confidence, first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(bundle_identifier) DO UPDATE SET display_name=excluded.display_name,
               identity_confidence=MAX(apps.identity_confidence, excluded.identity_confidence), last_seen_at=MAX(apps.last_seen_at, excluded.last_seen_at)",
            rusqlite::params![app_id.as_str(), bundle_identifier, display_name, identity_confidence, observed_at],
        )?;
        Ok(())
    }

    pub fn create_manual(
        &mut self,
        input: &ManualMemoryInput,
        correlation_id: &str,
    ) -> Result<Memory, StoreError> {
        validate_manual(input)?;
        let memory_id = MemoryId::new(Uuid::now_v7().to_string());
        let tx = self.connection.transaction()?;
        insert_manual_tx(&tx, &memory_id, input, 1)?;
        enqueue_projection_refresh_tx(&tx, memory_id.as_str(), 1, input.now_ms)?;
        record_event_tx(
            &tx,
            MemoryEventType::ReconciliationApplied,
            correlation_id,
            Some(&memory_id),
            None,
            Some("manual_create"),
            input.now_ms,
        )?;
        tx.commit()?;
        self.get_memory(&memory_id)?
            .ok_or(StoreError::Invariant("new memory disappeared"))
    }

    pub fn apply_plan(
        &mut self,
        plan: &MemoryMutationPlan,
        now_ms: i64,
    ) -> Result<MemoryMutationResult, StoreError> {
        validate_plan_for_storage(&self.connection, plan)?;
        let (run_id, candidate_index) = plan.idempotency_key();
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some((memory_id, outcome)) = tx.query_row(
            "SELECT memory_id, outcome FROM extraction_candidates WHERE extraction_run_id=?1 AND candidate_index=?2",
            rusqlite::params![run_id.as_str(), candidate_index],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        ).optional()? {
            tx.commit()?;
            return Ok(MemoryMutationResult {
                memory_id: memory_id.map(MemoryId::new), status: outcome_status(&outcome), was_already_applied: true,
            });
        }
        let (memory_id, status, outcome, reason) = match plan {
            MemoryMutationPlan::Create {
                candidate, status, ..
            } => {
                let id = MemoryId::new(Uuid::now_v7().to_string());
                insert_candidate_tx(&tx, &id, candidate, *status, 1, now_ms)?;
                (Some(id), *status, "create", None)
            }
            MemoryMutationPlan::Duplicate {
                existing_memory_id,
                source_event_ids,
                confidence,
                ..
            } => {
                let current: Option<String> = tx
                    .query_row(
                        "SELECT status FROM memories WHERE id=?1",
                        [existing_memory_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if current.as_deref() != Some("active") {
                    return Err(StoreError::InvalidPlan("duplicate target is not active"));
                }
                for source_id in source_event_ids {
                    tx.execute("INSERT OR IGNORE INTO memory_sources(memory_id, source_event_id, support_kind) VALUES (?1, ?2, 'supporting')", rusqlite::params![existing_memory_id.as_str(), source_id.as_str()])?;
                }
                tx.execute(
                    "UPDATE memories SET confidence=MAX(confidence, ?2), revision=revision+1, updated_at=?3 WHERE id=?1",
                    rusqlite::params![existing_memory_id.as_str(), confidence, now_ms],
                )?;
                (
                    Some(existing_memory_id.clone()),
                    MemoryStatus::Active,
                    "duplicate",
                    None,
                )
            }
            MemoryMutationPlan::Update {
                existing_memory_id,
                candidate,
                status,
                ..
            } => {
                let (old_mode, old_revision): (String, u32) = tx.query_row("SELECT assertion_mode, revision FROM memories WHERE id=?1 AND status='active'", [existing_memory_id.as_str()], |row| Ok((row.get(0)?, row.get(1)?))).optional()?.ok_or(StoreError::InvalidPlan("update target is not active"))?;
                if candidate.assertion_mode.trust_rank()
                    < parse_assertion_mode(&old_mode)?.trust_rank()
                {
                    return Err(StoreError::InvalidPlan(
                        "lower-trust assertion cannot supersede higher-trust memory",
                    ));
                }
                if *status != MemoryStatus::Active {
                    return Err(StoreError::InvalidPlan(
                        "non-active candidate cannot supersede memory",
                    ));
                }
                let id = MemoryId::new(Uuid::now_v7().to_string());
                insert_candidate_tx(
                    &tx,
                    &id,
                    candidate,
                    *status,
                    old_revision.saturating_add(1),
                    now_ms,
                )?;
                tx.execute(
                    "UPDATE memories SET status='superseded', updated_at=?2 WHERE id=?1",
                    rusqlite::params![existing_memory_id.as_str(), now_ms],
                )?;
                tx.execute(
                    "DELETE FROM memory_search_documents WHERE memory_id=?1",
                    [existing_memory_id.as_str()],
                )?;
                tx.execute("INSERT INTO memory_relations(from_memory_id, to_memory_id, relation_type, confidence, created_at) VALUES (?1, ?2, 'updates', 1.0, ?3)", rusqlite::params![id.as_str(), existing_memory_id.as_str(), now_ms])?;
                (Some(id), *status, "update", None)
            }
            MemoryMutationPlan::Extend {
                existing_memory_id,
                candidate,
                status,
                ..
            } => {
                let _: i64 = tx
                    .query_row(
                        "SELECT 1 FROM memories WHERE id=?1 AND status='active'",
                        [existing_memory_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or(StoreError::InvalidPlan("extend target is not active"))?;
                let id = MemoryId::new(Uuid::now_v7().to_string());
                insert_candidate_tx(&tx, &id, candidate, *status, 1, now_ms)?;
                tx.execute("INSERT INTO memory_relations(from_memory_id, to_memory_id, relation_type, confidence, created_at) VALUES (?1, ?2, 'extends', 1.0, ?3)", rusqlite::params![id.as_str(), existing_memory_id.as_str(), now_ms])?;
                (Some(id), *status, "extend", None)
            }
            MemoryMutationPlan::Ignore { reason, .. } => (
                None,
                MemoryStatus::Rejected,
                "ignore",
                Some(reason.as_str()),
            ),
            MemoryMutationPlan::RequestReview {
                candidate, reason, ..
            } => {
                let id = MemoryId::new(Uuid::now_v7().to_string());
                insert_candidate_tx(&tx, &id, candidate, MemoryStatus::PendingReview, 1, now_ms)?;
                (
                    Some(id),
                    MemoryStatus::PendingReview,
                    "request_review",
                    Some(reason.as_str()),
                )
            }
        };
        tx.execute(
            "INSERT INTO extraction_candidates(extraction_run_id, candidate_index, outcome, memory_id, audit_reason, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, CASE WHEN ?3='ignore' THEN ?6 + 604800000 ELSE NULL END)",
            rusqlite::params![run_id.as_str(), candidate_index, outcome, memory_id.as_ref().map(MemoryId::as_str), reason, now_ms],
        )?;
        if let Some(memory_id) = &memory_id {
            let revision: u32 = tx.query_row(
                "SELECT revision FROM memories WHERE id=?1",
                [memory_id.as_str()],
                |row| row.get(0),
            )?;
            enqueue_projection_refresh_tx(&tx, memory_id.as_str(), revision, now_ms)?;
        }
        record_event_tx(
            &tx,
            MemoryEventType::ReconciliationApplied,
            run_id.as_str(),
            memory_id.as_ref(),
            None,
            Some(outcome),
            now_ms,
        )?;
        tx.commit()?;
        Ok(MemoryMutationResult {
            memory_id,
            status,
            was_already_applied: false,
        })
    }

    pub fn edit_manual(
        &mut self,
        existing_id: &MemoryId,
        expected_revision: u32,
        correlation_id: &str,
        input: &ManualMemoryInput,
    ) -> Result<Memory, StoreError> {
        validate_manual(input)?;
        let replacement_id = MemoryId::new(Uuid::now_v7().to_string());
        let tx = self.connection.transaction()?;
        ensure_revision(&tx, existing_id, expected_revision)?;
        let existing_status: String = tx.query_row(
            "SELECT status FROM memories WHERE id=?1",
            [existing_id.as_str()],
            |row| row.get(0),
        )?;
        if existing_status == "forgotten" || existing_status == "rejected" {
            return Err(StoreError::InvalidInput(
                "memory cannot be edited in its current state",
            ));
        }
        insert_manual_tx(
            &tx,
            &replacement_id,
            input,
            expected_revision.saturating_add(1),
        )?;
        tx.execute(
            "UPDATE memories SET status='superseded', revision=revision+1, updated_at=?2
             WHERE id=?1 AND revision=?3",
            rusqlite::params![existing_id.as_str(), input.now_ms, expected_revision],
        )?;
        tx.execute(
            "DELETE FROM memory_search_documents WHERE memory_id=?1",
            [existing_id.as_str()],
        )?;
        tx.execute(
            "INSERT INTO memory_relations(from_memory_id, to_memory_id, relation_type, confidence, created_at)
             VALUES (?1, ?2, 'updates', 1.0, ?3)",
            rusqlite::params![replacement_id.as_str(), existing_id.as_str(), input.now_ms],
        )?;
        enqueue_projection_refresh_tx(
            &tx,
            replacement_id.as_str(),
            expected_revision.saturating_add(1),
            input.now_ms,
        )?;
        record_event_tx(
            &tx,
            MemoryEventType::MemoryEdited,
            correlation_id,
            Some(&replacement_id),
            None,
            Some("manual_revision"),
            input.now_ms,
        )?;
        tx.commit()?;
        self.get_memory(&replacement_id)?
            .ok_or(StoreError::Invariant("replacement memory disappeared"))
    }

    pub fn get_memory(&self, id: &MemoryId) -> Result<Option<Memory>, StoreError> {
        let sql = BASE_MEMORY_SELECT.to_owned() + " WHERE m.id=?1";
        let raw = self
            .connection
            .query_row(&sql, [id.as_str()], RawMemory::from_row)
            .optional()?;
        raw.map(|raw| self.hydrate_memory(raw)).transpose()
    }

    pub fn search(&self, filter: &MemorySearchFilter) -> Result<Vec<Memory>, StoreError> {
        filter.validate()?;
        let limit = filter.limit as i64;
        let mut sql = String::from(BASE_MEMORY_SELECT);
        let mut clauses = vec![if filter.status.is_some() {
            "m.status=:status"
        } else if filter.include_history {
            "m.status NOT IN ('rejected', 'forgotten', 'pending_review')"
        } else {
            "m.status = 'active'"
        }];
        if filter.query.is_some() {
            clauses.push(
                "m.row_id IN (SELECT rowid FROM memories_fts WHERE memories_fts MATCH :query)",
            );
        }
        if filter.app_id.is_some() {
            clauses.push(match filter.app_role {
                MemoryAppFilterRole::Any => "EXISTS (SELECT 1 FROM memory_apps ma WHERE ma.memory_id=m.id AND ma.app_id=:app_id)",
                MemoryAppFilterRole::AppliesTo => "EXISTS (SELECT 1 FROM memory_apps ma WHERE ma.memory_id=m.id AND ma.app_id=:app_id AND ma.role='applies_to')",
            });
        }
        if filter.category_slug.is_some() {
            clauses.push("EXISTS (SELECT 1 FROM memory_categories mc JOIN categories c ON c.id=mc.category_id WHERE mc.memory_id=m.id AND (c.slug=:category OR c.slug LIKE :category_prefix))");
        }
        if filter.scope_type.is_some() {
            clauses.push("s.scope_type=:scope_type");
        }
        if filter.scope_key.is_some() {
            clauses.push("s.scope_key=:scope_key");
        }
        if filter.entity_id.is_some() {
            clauses.push("EXISTS (SELECT 1 FROM memory_entities me WHERE me.memory_id=m.id AND me.entity_id=:entity_id)");
        }
        if filter.memory_type.is_some() {
            clauses.push("m.memory_type=:memory_type");
        }
        if filter.from_ms.is_some() {
            clauses.push("m.updated_at >= :from_ms");
        }
        if filter.until_ms.is_some() {
            clauses.push("m.updated_at <= :until_ms");
        }
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
        sql.push_str(" ORDER BY m.updated_at DESC LIMIT :limit");
        let query = filter
            .query
            .as_deref()
            .map(fts_literal_query)
            .unwrap_or_default();
        let app_id = filter.app_id.as_ref().map(AppId::as_str).unwrap_or("");
        let category = filter.category_slug.as_deref().unwrap_or("");
        let category_prefix = format!("{category}.%");
        let scope_type = filter.scope_type.map(scope_type_str).unwrap_or("");
        let scope_key = filter.scope_key.as_deref().unwrap_or("");
        let entity_id = filter
            .entity_id
            .as_ref()
            .map(EntityId::as_str)
            .unwrap_or("");
        let memory_type = filter.memory_type.map(memory_type_str).unwrap_or("");
        let status = filter.status.map(status_str).unwrap_or("");
        let mut stmt = self.connection.prepare(&sql)?;
        macro_rules! bind_if_present {
            ($name:literal, $value:expr) => {
                if let Some(index) = stmt.parameter_index($name)? {
                    stmt.raw_bind_parameter(index, $value)?;
                }
            };
        }
        bind_if_present!(":query", query);
        bind_if_present!(":app_id", app_id);
        bind_if_present!(":category", category);
        bind_if_present!(":category_prefix", category_prefix);
        bind_if_present!(":scope_type", scope_type);
        bind_if_present!(":scope_key", scope_key);
        bind_if_present!(":entity_id", entity_id);
        bind_if_present!(":memory_type", memory_type);
        bind_if_present!(":status", status);
        bind_if_present!(":from_ms", filter.from_ms.unwrap_or(i64::MIN));
        bind_if_present!(":until_ms", filter.until_ms.unwrap_or(i64::MAX));
        bind_if_present!(":limit", limit);
        let mut rows = stmt.raw_query();
        let mut raws = Vec::new();
        while let Some(row) = rows.next()? {
            raws.push(RawMemory::from_row(row)?);
        }
        raws.into_iter()
            .map(|raw| self.hydrate_memory(raw))
            .collect()
    }

    pub fn delete_memory(
        &mut self,
        memory_id: &MemoryId,
        expected_revision: u32,
        mode: DeleteMode,
        correlation_id: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        ensure_revision(&tx, memory_id, expected_revision)?;
        if mode == DeleteMode::Forget {
            tx.execute(
                "UPDATE memories SET status='forgotten', updated_at=?2, revision=revision+1
                 WHERE id=?1 AND revision=?3",
                rusqlite::params![memory_id.as_str(), now, expected_revision],
            )?;
            remove_rebuildable_projections(&tx, memory_id.as_str())?;
            record_event_tx(
                &tx,
                MemoryEventType::MemoryForgotten,
                correlation_id,
                Some(memory_id),
                None,
                Some("forgotten"),
                now,
            )?;
            tx.commit()?;
            return Ok(());
        }
        let source_ids = query_strings(
            &tx,
            "SELECT source_event_id FROM memory_sources WHERE memory_id=?1",
            memory_id.as_str(),
        )?;
        invalidate_affected_profiles(&tx, memory_id.as_str())?;
        tx.execute("DELETE FROM memories WHERE id = ?1", [memory_id.as_str()])?;
        for source_id in source_ids {
            tx.execute(
                "DELETE FROM activity_segment_sources WHERE source_event_id=?1 AND NOT EXISTS (
                    SELECT 1 FROM memory_sources WHERE source_event_id=?1
                 )",
                [&source_id],
            )?;
            tx.execute(
                "DELETE FROM source_events WHERE id=?1 AND NOT EXISTS (
                    SELECT 1 FROM memory_sources WHERE source_event_id=?1
                 )",
                [&source_id],
            )?;
        }
        tx.execute(
            "DELETE FROM activity_segments WHERE NOT EXISTS (
                SELECT 1 FROM activity_segment_sources WHERE segment_id=activity_segments.id
             )",
            [],
        )?;
        tx.execute(
            "INSERT INTO memory_tombstones(memory_id, revision, deleted_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(memory_id) DO UPDATE SET revision = MAX(revision, excluded.revision), deleted_at = excluded.deleted_at",
            rusqlite::params![memory_id.as_str(), expected_revision + 1, now],
        )?;
        record_event_tx(
            &tx,
            MemoryEventType::MemoryDeleted,
            correlation_id,
            Some(memory_id),
            None,
            Some("permanent"),
            now,
        )?;
        tx.commit()?;
        Ok(())
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let has_migrations: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [], |row| row.get(0),
        )?;
        if !has_migrations {
            let tx = self.connection.transaction()?;
            tx.execute_batch(MIGRATION_0001)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
                [now_millis()?],
            )?;
            tx.commit()?;
        }
        let current: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if current > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::NewerSchema {
                found: current,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if current < 2 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(MIGRATION_0002)?;
            tx.execute(
                "UPDATE schema_migrations SET checksum=?1 WHERE version=1",
                [migration_checksum(MIGRATION_0001)],
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at, checksum) VALUES (2, ?1, ?2)",
                rusqlite::params![now_millis()?, migration_checksum(MIGRATION_0002)],
            )?;
            tx.commit()?;
        }
        let current: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if current < 3 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(MIGRATION_0003)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at, checksum) VALUES (3, ?1, ?2)",
                rusqlite::params![now_millis()?, migration_checksum(MIGRATION_0003)],
            )?;
            tx.commit()?;
        }
        let current: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if current < 4 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(MIGRATION_0004)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at, checksum) VALUES (4, ?1, ?2)",
                rusqlite::params![now_millis()?, migration_checksum(MIGRATION_0004)],
            )?;
            tx.commit()?;
        }
        self.verify_migrations()
    }

    fn verify_migrations(&self) -> Result<(), StoreError> {
        for (version, expected) in [
            (1_i64, migration_checksum(MIGRATION_0001)),
            (2_i64, migration_checksum(MIGRATION_0002)),
            (3_i64, migration_checksum(MIGRATION_0003)),
            (4_i64, migration_checksum(MIGRATION_0004)),
        ] {
            let actual: Option<String> = self
                .connection
                .query_row(
                    "SELECT checksum FROM schema_migrations WHERE version=?1",
                    [version],
                    |row| row.get(0),
                )
                .optional()?;
            if actual.as_deref() != Some(expected.as_str()) {
                return Err(StoreError::MigrationChecksum { version });
            }
        }
        Ok(())
    }

    fn seed_taxonomy(&mut self) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let global_scope_id = deterministic_id("scope:global:");
        tx.execute(
            "INSERT OR IGNORE INTO memory_scopes(id, scope_type, scope_key, display_name) VALUES (?1, 'global', '', 'Global')",
            [&global_scope_id],
        )?;
        for (slug, parent, display_name) in TAXONOMY {
            let id = deterministic_id(&format!("category:{slug}"));
            let parent_id = parent.map(|value| deterministic_id(&format!("category:{value}")));
            tx.execute(
                "INSERT INTO categories(id, parent_id, slug, display_name, description, taxonomy_version)
                 VALUES (?1, ?2, ?3, ?4, '', ?5)
                 ON CONFLICT(slug) DO UPDATE SET parent_id=excluded.parent_id, display_name=excluded.display_name, taxonomy_version=excluded.taxonomy_version",
                rusqlite::params![id, parent_id, slug, display_name, TAXONOMY_VERSION],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

struct RawMemory {
    id: String,
    normalized_content: String,
    display_content: String,
    memory_type: String,
    assertion_mode: String,
    status: String,
    scope_id: String,
    scope_type: String,
    scope_key: String,
    scope_name: String,
    importance: f32,
    confidence: f32,
    sensitivity: String,
    valid_from_ms: Option<i64>,
    valid_until_ms: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
    revision: u32,
}

impl RawMemory {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            normalized_content: row.get(1)?,
            display_content: row.get(2)?,
            memory_type: row.get(3)?,
            assertion_mode: row.get(4)?,
            status: row.get(5)?,
            scope_id: row.get(6)?,
            scope_type: row.get(7)?,
            scope_key: row.get(8)?,
            scope_name: row.get(9)?,
            importance: row.get(10)?,
            confidence: row.get(11)?,
            sensitivity: row.get(12)?,
            valid_from_ms: row.get(13)?,
            valid_until_ms: row.get(14)?,
            created_at_ms: row.get(15)?,
            updated_at_ms: row.get(16)?,
            revision: row.get(17)?,
        })
    }
}

const BASE_MEMORY_SELECT: &str =
    "SELECT m.id, m.normalized_content, m.display_content, m.memory_type,
 m.assertion_mode, m.status, s.id, s.scope_type, s.scope_key, s.display_name, m.importance,
 m.confidence, m.sensitivity, m.valid_from, m.valid_until, m.created_at, m.updated_at, m.revision
 FROM memories m JOIN memory_scopes s ON s.id=m.scope_id";

impl MemoryStore {
    fn hydrate_memory(&self, raw: RawMemory) -> Result<Memory, StoreError> {
        let id = MemoryId::new(raw.id);
        let category_slugs = query_strings(
            &self.connection,
            "SELECT c.slug FROM memory_categories mc JOIN categories c ON c.id=mc.category_id WHERE mc.memory_id=?1 ORDER BY c.slug",
            id.as_str(),
        )?;
        let source_app_ids = query_strings(
            &self.connection,
            "SELECT app_id FROM memory_apps WHERE memory_id=?1 AND role='source' ORDER BY app_id",
            id.as_str(),
        )?
        .into_iter()
        .map(AppId::new)
        .collect();
        let applicable_app_ids = query_strings(
            &self.connection,
            "SELECT app_id FROM memory_apps WHERE memory_id=?1 AND role='applies_to' ORDER BY app_id",
            id.as_str(),
        )?.into_iter().map(AppId::new).collect();
        let source_event_ids = query_strings(
            &self.connection,
            "SELECT source_event_id FROM memory_sources WHERE memory_id=?1 ORDER BY source_event_id",
            id.as_str(),
        )?.into_iter().map(SourceEventId::new).collect();
        let mut stmt = self.connection.prepare(
            "SELECT e.id, e.canonical_name, me.role FROM memory_entities me JOIN entities e ON e.id=me.entity_id WHERE me.memory_id=?1 ORDER BY e.canonical_name"
        )?;
        let entities = stmt
            .query_map([id.as_str()], |row| {
                let role: String = row.get(2)?;
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, role))
            })?
            .map(|row| {
                let (entity_id, canonical_name, role) = row?;
                Ok(EntityReference {
                    entity_id: EntityId::new(entity_id),
                    canonical_name,
                    role: parse_entity_role(&role)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(Memory {
            id,
            normalized_content: raw.normalized_content,
            display_content: raw.display_content,
            memory_type: parse_memory_type(&raw.memory_type)?,
            assertion_mode: parse_assertion_mode(&raw.assertion_mode)?,
            status: parse_status(&raw.status)?,
            scope: MemoryScope {
                id: ScopeId::new(raw.scope_id),
                scope_type: parse_scope_type(&raw.scope_type)?,
                scope_key: raw.scope_key,
                display_name: raw.scope_name,
            },
            source_app_ids,
            applicable_app_ids,
            category_slugs,
            entities,
            source_event_ids,
            importance: raw.importance,
            confidence: raw.confidence,
            sensitivity: parse_sensitivity(&raw.sensitivity)?,
            valid_from_ms: raw.valid_from_ms,
            valid_until_ms: raw.valid_until_ms,
            created_at_ms: raw.created_at_ms,
            updated_at_ms: raw.updated_at_ms,
            revision: raw.revision,
        })
    }
}

fn validate_manual(input: &ManualMemoryInput) -> Result<(), StoreError> {
    let normalized = normalize_content(&input.content);
    if !(8..=500).contains(&normalized.chars().count()) {
        return Err(StoreError::InvalidInput(
            "memory content must be 8..=500 characters",
        ));
    }
    if !input.importance.is_finite() || !(0.0..=1.0).contains(&input.importance) {
        return Err(StoreError::InvalidInput("importance must be in [0, 1]"));
    }
    if input.category_slugs.len() > 5 {
        return Err(StoreError::InvalidInput(
            "a memory may have at most five categories",
        ));
    }
    if input.scope_type != ScopeType::Global && input.scope_key.trim().is_empty() {
        return Err(StoreError::InvalidInput("non-global scope requires a key"));
    }
    if let (Some(from), Some(until)) = (input.valid_from_ms, input.valid_until_ms)
        && until < from
    {
        return Err(StoreError::InvalidInput("validity interval is reversed"));
    }
    Ok(())
}

fn validate_plan_for_storage(
    connection: &Connection,
    plan: &MemoryMutationPlan,
) -> Result<(), StoreError> {
    let candidate = match plan {
        MemoryMutationPlan::Create { candidate, .. }
        | MemoryMutationPlan::Update { candidate, .. }
        | MemoryMutationPlan::Extend { candidate, .. }
        | MemoryMutationPlan::RequestReview { candidate, .. } => Some(candidate),
        MemoryMutationPlan::Duplicate {
            source_event_ids,
            confidence,
            ..
        } => {
            if !confidence.is_finite() || !(0.0..=1.0).contains(confidence) {
                return Err(StoreError::InvalidPlan("invalid duplicate confidence"));
            }
            validate_references_exist(
                connection,
                "source_events",
                source_event_ids.iter().map(SourceEventId::as_str),
            )?;
            None
        }
        MemoryMutationPlan::Ignore { reason, .. } => {
            if reason.trim().is_empty() || reason.chars().count() > 256 {
                return Err(StoreError::InvalidPlan("invalid bounded audit reason"));
            }
            None
        }
    };
    let Some(candidate) = candidate else {
        return Ok(());
    };
    let normalized = normalize_content(&candidate.content);
    if !(8..=500).contains(&normalized.chars().count())
        || !candidate.importance.is_finite()
        || !(0.0..=1.0).contains(&candidate.importance)
        || !candidate.confidence.is_finite()
        || !(0.0..=candidate.assertion_mode.confidence_ceiling()).contains(&candidate.confidence)
        || candidate.category_slugs.len() > 5
    {
        return Err(StoreError::InvalidPlan(
            "candidate failed scalar validation",
        ));
    }
    if candidate.assertion_mode != AssertionMode::Manual
        && !(1..=20).contains(&candidate.supporting_source_event_ids.len())
    {
        return Err(StoreError::InvalidPlan(
            "candidate provenance is missing or unbounded",
        ));
    }
    if candidate.assertion_mode == AssertionMode::Manual
        && candidate.supporting_source_event_ids.len() > 20
    {
        return Err(StoreError::InvalidPlan(
            "manual candidate provenance is unbounded",
        ));
    }
    if candidate.scope.scope_type != ScopeType::Global
        && candidate.scope.scope_key.trim().is_empty()
        || matches!((candidate.valid_from_ms, candidate.valid_until_ms), (Some(from), Some(until)) if until < from)
    {
        return Err(StoreError::InvalidPlan(
            "candidate scope or validity is invalid",
        ));
    }
    inspect_private_content(
        &normalized,
        candidate.from_password_field,
        candidate.assertion_mode == AssertionMode::Inferred,
    )
    .map_err(|_| StoreError::InvalidPlan("candidate failed privacy policy"))?;
    validate_references_exist(
        connection,
        "source_events",
        candidate
            .supporting_source_event_ids
            .iter()
            .map(SourceEventId::as_str),
    )?;
    validate_references_exist(
        connection,
        "apps",
        candidate
            .source_app_ids
            .iter()
            .chain(&candidate.applicable_app_ids)
            .map(AppId::as_str),
    )?;
    for slug in &candidate.category_slugs {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM categories WHERE slug=?1)",
            [slug],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::UnknownCategory(slug.clone()));
        }
    }
    Ok(())
}

fn validate_references_exist<'a>(
    connection: &Connection,
    table: &'static str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<(), StoreError> {
    let sql = match table {
        "source_events" => "SELECT EXISTS(SELECT 1 FROM source_events WHERE id=?1)",
        "apps" => "SELECT EXISTS(SELECT 1 FROM apps WHERE id=?1)",
        _ => return Err(StoreError::Invariant("unsupported reference table")),
    };
    for id in ids {
        let exists: bool = connection.query_row(sql, [id], |row| row.get(0))?;
        if !exists {
            return Err(StoreError::InvalidPlan(
                "candidate references unknown records",
            ));
        }
    }
    Ok(())
}

fn normalize_content(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assign_categories(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    slugs: &[String],
) -> Result<(), StoreError> {
    let effective: Vec<&str> = if slugs.is_empty() {
        vec!["uncategorized"]
    } else {
        slugs.iter().map(String::as_str).collect()
    };
    for slug in effective {
        let category_id: Option<String> = tx
            .query_row("SELECT id FROM categories WHERE slug=?1", [slug], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(category_id) = category_id else {
            return Err(StoreError::UnknownCategory(slug.to_owned()));
        };
        tx.execute("INSERT OR IGNORE INTO memory_categories(memory_id, category_id, confidence) VALUES (?1, ?2, 1.0)", rusqlite::params![memory_id, category_id])?;
    }
    Ok(())
}

fn insert_manual_tx(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &MemoryId,
    input: &ManualMemoryInput,
    revision: u32,
) -> Result<(), StoreError> {
    let normalized = normalize_content(&input.content);
    let scope_id = ScopeId::new(deterministic_id(&format!(
        "scope:{}:{}",
        scope_type_str(input.scope_type),
        input.scope_key
    )));
    tx.execute(
        "INSERT INTO memory_scopes(id, scope_type, scope_key, display_name) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(scope_type, scope_key) DO UPDATE SET display_name=excluded.display_name",
        rusqlite::params![
            scope_id.as_str(),
            scope_type_str(input.scope_type),
            input.scope_key,
            input.scope_display_name
        ],
    )?;
    tx.execute(
        "INSERT INTO memories(id, normalized_content, display_content, memory_type, assertion_mode, status,
            scope_id, importance, confidence, sensitivity, valid_from, valid_until, revision, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'manual', 'active', ?5, ?6, 1.0, ?7, ?8, ?9, ?10, ?11, ?11)",
        rusqlite::params![memory_id.as_str(), normalized.to_lowercase(), normalized,
            memory_type_str(input.memory_type), scope_id.as_str(), input.importance,
            sensitivity_str(input.sensitivity), input.valid_from_ms, input.valid_until_ms,
            revision, input.now_ms],
    )?;
    assign_categories(tx, memory_id.as_str(), &input.category_slugs)?;
    for app_id in &input.applicable_app_ids {
        tx.execute(
            "INSERT INTO memory_apps(memory_id, app_id, role) VALUES (?1, ?2, 'applies_to')",
            rusqlite::params![memory_id.as_str(), app_id.as_str()],
        )?;
    }
    refresh_search_document(tx, memory_id.as_str())?;
    Ok(())
}

fn insert_candidate_tx(
    tx: &rusqlite::Transaction<'_>,
    id: &MemoryId,
    candidate: &MemoryCandidate,
    status: MemoryStatus,
    revision: u32,
    now_ms: i64,
) -> Result<(), StoreError> {
    let normalized = normalize_content(&candidate.content);
    let scope_id = ScopeId::new(deterministic_id(&format!(
        "scope:{}:{}",
        scope_type_str(candidate.scope.scope_type),
        candidate.scope.scope_key
    )));
    tx.execute(
        "INSERT INTO memory_scopes(id, scope_type, scope_key, display_name) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(scope_type, scope_key) DO UPDATE SET display_name=excluded.display_name",
        rusqlite::params![
            scope_id.as_str(),
            scope_type_str(candidate.scope.scope_type),
            candidate.scope.scope_key,
            candidate.scope.display_name
        ],
    )?;
    tx.execute(
        "INSERT INTO memories(id, normalized_content, display_content, memory_type, assertion_mode, status, scope_id,
         importance, confidence, sensitivity, valid_from, valid_until, revision, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
        rusqlite::params![id.as_str(), normalized.to_lowercase(), normalized, memory_type_str(candidate.memory_type),
            assertion_mode_str(candidate.assertion_mode), status_str(status), scope_id.as_str(), candidate.importance,
            candidate.confidence, sensitivity_str(candidate.sensitivity), candidate.valid_from_ms, candidate.valid_until_ms,
            revision, now_ms],
    )?;
    assign_categories(tx, id.as_str(), &candidate.category_slugs)?;
    for app_id in &candidate.source_app_ids {
        tx.execute(
            "INSERT INTO memory_apps(memory_id, app_id, role) VALUES (?1, ?2, 'source')",
            rusqlite::params![id.as_str(), app_id.as_str()],
        )?;
    }
    for app_id in &candidate.applicable_app_ids {
        tx.execute(
            "INSERT INTO memory_apps(memory_id, app_id, role) VALUES (?1, ?2, 'applies_to')",
            rusqlite::params![id.as_str(), app_id.as_str()],
        )?;
    }
    for (index, source_id) in candidate.supporting_source_event_ids.iter().enumerate() {
        tx.execute("INSERT INTO memory_sources(memory_id, source_event_id, support_kind) VALUES (?1, ?2, ?3)", rusqlite::params![id.as_str(), source_id.as_str(), if index == 0 { "primary" } else { "supporting" }])?;
    }
    resolve_candidate_entities(tx, id.as_str(), candidate)?;
    if status == MemoryStatus::Active {
        refresh_search_document(tx, id.as_str())?;
    }
    Ok(())
}

fn resolve_candidate_entities(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    candidate: &MemoryCandidate,
) -> Result<(), StoreError> {
    for mention in &candidate.entity_mentions {
        let normalized = normalize_content(&mention.mention).to_lowercase();
        let mut stmt = tx.prepare(
            "SELECT DISTINCT e.id FROM entities e LEFT JOIN entity_aliases ea ON ea.entity_id=e.id
             WHERE e.entity_type=?1 AND (e.normalized_name=?2 OR ea.normalized_alias=?2) LIMIT 2",
        )?;
        let ids = stmt
            .query_map(
                rusqlite::params![entity_type_str(mention.entity_type), normalized],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() == 1 {
            tx.execute(
                "INSERT INTO memory_entities(memory_id, entity_id, role) VALUES (?1, ?2, ?3)",
                rusqlite::params![memory_id, ids[0], entity_role_str(mention.role)],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn refresh_search_document(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO memory_search_documents(row_id, memory_id, display_content, category_text, entity_text, app_text)
         SELECT m.row_id, m.id, m.display_content,
           COALESCE((SELECT group_concat(c.slug, ' ') FROM memory_categories mc JOIN categories c ON c.id=mc.category_id WHERE mc.memory_id=m.id), ''),
           COALESCE((SELECT group_concat(e.canonical_name, ' ') FROM memory_entities me JOIN entities e ON e.id=me.entity_id WHERE me.memory_id=m.id), ''),
           COALESCE((SELECT group_concat(a.bundle_identifier || ' ' || a.display_name, ' ') FROM memory_apps ma JOIN apps a ON a.id=ma.app_id WHERE ma.memory_id=m.id), '')
         FROM memories m WHERE m.id=?1 AND m.status='active'
         ON CONFLICT(memory_id) DO UPDATE SET display_content=excluded.display_content, category_text=excluded.category_text,
           entity_text=excluded.entity_text, app_text=excluded.app_text",
        [memory_id],
    )?;
    Ok(())
}

fn remove_rebuildable_projections(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "DELETE FROM memory_search_documents WHERE memory_id=?1",
        [memory_id],
    )?;
    tx.execute(
        "DELETE FROM memory_embeddings WHERE memory_id=?1",
        [memory_id],
    )?;
    invalidate_affected_profiles(tx, memory_id)?;
    tx.execute(
        "DELETE FROM projection_jobs WHERE projection_key=?1",
        [memory_id],
    )?;
    Ok(())
}

fn invalidate_affected_profiles(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "DELETE FROM memory_profiles
         WHERE (projection_type='global' AND EXISTS (
                    SELECT 1 FROM memories m
                    JOIN memory_scopes s ON s.id=m.scope_id
                    WHERE m.id=?1 AND s.scope_type='global'
                ))
            OR (projection_type='app' AND (
                    EXISTS (
                        SELECT 1 FROM memories m
                        JOIN memory_scopes s ON s.id=m.scope_id
                        WHERE m.id=?1 AND s.scope_type='app'
                          AND s.scope_key=memory_profiles.projection_key
                    )
                    OR EXISTS (
                        SELECT 1 FROM memory_apps ma
                        WHERE ma.memory_id=?1 AND ma.role='applies_to'
                          AND ma.app_id=memory_profiles.projection_key
                    )
                ))
            OR (projection_type='project' AND EXISTS (
                    SELECT 1 FROM memories m
                    JOIN memory_scopes s ON s.id=m.scope_id
                    WHERE m.id=?1 AND s.scope_type='project'
                      AND s.scope_key=memory_profiles.projection_key
                ))
            OR (projection_type='category' AND EXISTS (
                    SELECT 1 FROM memory_categories mc
                    JOIN categories c ON c.id=mc.category_id
                    WHERE mc.memory_id=?1
                      AND (c.slug=memory_profiles.projection_key
                           OR (length(c.slug)>length(memory_profiles.projection_key)
                               AND substr(c.slug, 1, length(memory_profiles.projection_key))=
                                   memory_profiles.projection_key
                               AND substr(c.slug, length(memory_profiles.projection_key)+1, 1)='.'))
                ))
            OR (projection_type='entity' AND EXISTS (
                    SELECT 1 FROM memory_entities me
                    WHERE me.memory_id=?1
                      AND me.entity_id=memory_profiles.projection_key
                ))",
        [memory_id],
    )?;
    Ok(())
}

fn query_strings(connection: &Connection, sql: &str, id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = connection.prepare(sql)?;
    Ok(stmt
        .query_map([id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

fn fts_literal_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

const fn memory_type_str(value: MemoryType) -> &'static str {
    match value {
        MemoryType::Preference => "preference",
        MemoryType::Fact => "fact",
        MemoryType::Decision => "decision",
        MemoryType::Episode => "episode",
        MemoryType::Task => "task",
        MemoryType::Procedure => "procedure",
    }
}
const fn scope_type_str(value: ScopeType) -> &'static str {
    match value {
        ScopeType::Global => "global",
        ScopeType::App => "app",
        ScopeType::Project => "project",
        ScopeType::Entity => "entity",
    }
}
const fn assertion_mode_str(value: AssertionMode) -> &'static str {
    match value {
        AssertionMode::Explicit => "explicit",
        AssertionMode::Observed => "observed",
        AssertionMode::Inferred => "inferred",
        AssertionMode::Imported => "imported",
        AssertionMode::Manual => "manual",
    }
}
const fn status_str(value: MemoryStatus) -> &'static str {
    match value {
        MemoryStatus::Active => "active",
        MemoryStatus::Superseded => "superseded",
        MemoryStatus::Expired => "expired",
        MemoryStatus::Forgotten => "forgotten",
        MemoryStatus::PendingReview => "pending_review",
        MemoryStatus::Rejected => "rejected",
    }
}
const fn entity_type_str(value: mega_memory::EntityType) -> &'static str {
    match value {
        mega_memory::EntityType::Person => "person",
        mega_memory::EntityType::Organization => "organization",
        mega_memory::EntityType::Project => "project",
        mega_memory::EntityType::Place => "place",
        mega_memory::EntityType::Product => "product",
    }
}
const fn entity_role_str(value: EntityRole) -> &'static str {
    match value {
        EntityRole::Subject => "subject",
        EntityRole::Object => "object",
        EntityRole::Participant => "participant",
        EntityRole::Mentioned => "mentioned",
        EntityRole::Scope => "scope",
    }
}

fn outcome_status(outcome: &str) -> MemoryStatus {
    match outcome {
        "request_review" => MemoryStatus::PendingReview,
        "ignore" => MemoryStatus::Rejected,
        _ => MemoryStatus::Active,
    }
}

fn parse_memory_type(value: &str) -> Result<MemoryType, StoreError> {
    match value {
        "preference" => Ok(MemoryType::Preference),
        "fact" => Ok(MemoryType::Fact),
        "decision" => Ok(MemoryType::Decision),
        "episode" => Ok(MemoryType::Episode),
        "task" => Ok(MemoryType::Task),
        "procedure" => Ok(MemoryType::Procedure),
        _ => Err(StoreError::InvalidEnum("memory_type", value.to_owned())),
    }
}
fn parse_assertion_mode(value: &str) -> Result<AssertionMode, StoreError> {
    match value {
        "explicit" => Ok(AssertionMode::Explicit),
        "observed" => Ok(AssertionMode::Observed),
        "inferred" => Ok(AssertionMode::Inferred),
        "imported" => Ok(AssertionMode::Imported),
        "manual" => Ok(AssertionMode::Manual),
        _ => Err(StoreError::InvalidEnum("assertion_mode", value.to_owned())),
    }
}
pub(crate) fn parse_status(value: &str) -> Result<MemoryStatus, StoreError> {
    match value {
        "active" => Ok(MemoryStatus::Active),
        "superseded" => Ok(MemoryStatus::Superseded),
        "expired" => Ok(MemoryStatus::Expired),
        "forgotten" => Ok(MemoryStatus::Forgotten),
        "pending_review" => Ok(MemoryStatus::PendingReview),
        "rejected" => Ok(MemoryStatus::Rejected),
        _ => Err(StoreError::InvalidEnum("status", value.to_owned())),
    }
}
fn parse_scope_type(value: &str) -> Result<ScopeType, StoreError> {
    match value {
        "global" => Ok(ScopeType::Global),
        "app" => Ok(ScopeType::App),
        "project" => Ok(ScopeType::Project),
        "entity" => Ok(ScopeType::Entity),
        _ => Err(StoreError::InvalidEnum("scope_type", value.to_owned())),
    }
}
fn parse_sensitivity(value: &str) -> Result<Sensitivity, StoreError> {
    match value {
        "public" => Ok(Sensitivity::Public),
        "private" => Ok(Sensitivity::Private),
        "sensitive" => Ok(Sensitivity::Sensitive),
        _ => Err(StoreError::InvalidEnum("sensitivity", value.to_owned())),
    }
}
fn parse_entity_role(value: &str) -> Result<EntityRole, StoreError> {
    match value {
        "subject" => Ok(EntityRole::Subject),
        "object" => Ok(EntityRole::Object),
        "participant" => Ok(EntityRole::Participant),
        "mentioned" => Ok(EntityRole::Mentioned),
        "scope" => Ok(EntityRole::Scope),
        _ => Err(StoreError::InvalidEnum("entity_role", value.to_owned())),
    }
}

fn deterministic_id(name: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes()).to_string()
}

fn migration_checksum(sql: &str) -> String {
    encode_hex(&Sha256::digest(sql.as_bytes()))
}

fn now_millis() -> Result<i64, StoreError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StoreError::Clock)?
        .as_millis();
    i64::try_from(millis).map_err(|_| StoreError::Clock)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}

fn prepare_parent(path: &Path) -> Result<(), StoreError> {
    if path == Path::new(":memory:") {
        return Ok(());
    }
    let parent = path.parent().ok_or(StoreError::InvalidPath)?;
    fs::create_dir_all(parent)?;
    restrict_permissions(parent)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    if path == Path::new(":memory:") || !path.exists() {
        return Ok(());
    }
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("memory storage requires an encryption key")]
    EncryptionRequired,
    #[error("the SQLite build does not provide SQLCipher")]
    CipherUnavailable,
    #[error("invalid memory database path")]
    InvalidPath,
    #[error("invalid memory input: {0}")]
    InvalidInput(&'static str),
    #[error("history admission idempotency key conflicts with an existing entry")]
    HistoryIdempotencyConflict,
    #[error("history entry conflicts with an existing entry ID")]
    HistoryEntryConflict,
    #[error("audio asset recovery lease is missing or owned by another worker")]
    AudioRecoveryLeaseLost,
    #[error("memory was not found")]
    NotFound,
    #[error("unknown category: {0}")]
    UnknownCategory(String),
    #[error("invalid persisted {0} value: {1}")]
    InvalidEnum(&'static str, String),
    #[error("memory store invariant failed: {0}")]
    Invariant(&'static str),
    #[error("invalid memory mutation plan: {0}")]
    InvalidPlan(&'static str),
    #[error("memory database integrity check failed: {0}")]
    Integrity(String),
    #[error("memory database schema {found} is newer than supported schema {supported}")]
    NewerSchema { found: i64, supported: i64 },
    #[error("memory migration {version} checksum does not match the compiled migration")]
    MigrationChecksum { version: i64 },
    #[error("system clock is outside the supported Unix millisecond range")]
    Clock,
    #[error("memory revision conflict; expected {expected}, current revision is {actual}")]
    RevisionConflict { expected: u32, actual: u32 },
    #[error("extraction job lease is missing, expired, or owned by another worker")]
    LeaseLost,
    #[error("memory database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("memory storage filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid JSON metadata: {0}")]
    Json(#[from] serde_json::Error),
}

const TAXONOMY: &[(&str, Option<&str>, &str)] = &[
    ("uncategorized", None, "Uncategorized"),
    ("choices", None, "Choices"),
    ("choices.food", Some("choices"), "Food"),
    ("choices.food.cuisine", Some("choices.food"), "Cuisine"),
    (
        "choices.food.ingredients",
        Some("choices.food"),
        "Ingredients",
    ),
    ("choices.food.dietary", Some("choices.food"), "Dietary"),
    (
        "choices.food.restaurants",
        Some("choices.food"),
        "Restaurants",
    ),
    ("choices.food.drinks", Some("choices.food"), "Drinks"),
    ("choices.design", Some("choices"), "Design"),
    (
        "choices.design.visual_style",
        Some("choices.design"),
        "Visual style",
    ),
    (
        "choices.design.typography",
        Some("choices.design"),
        "Typography",
    ),
    ("choices.design.color", Some("choices.design"), "Color"),
    ("choices.design.layout", Some("choices.design"), "Layout"),
    (
        "choices.design.interaction",
        Some("choices.design"),
        "Interaction",
    ),
    (
        "choices.design.interface",
        Some("choices.design"),
        "Interface",
    ),
    ("choices.technology", Some("choices"), "Technology"),
    (
        "choices.technology.languages",
        Some("choices.technology"),
        "Languages",
    ),
    (
        "choices.technology.frameworks",
        Some("choices.technology"),
        "Frameworks",
    ),
    (
        "choices.technology.architecture",
        Some("choices.technology"),
        "Architecture",
    ),
    (
        "choices.technology.tools",
        Some("choices.technology"),
        "Tools",
    ),
    (
        "choices.technology.platforms",
        Some("choices.technology"),
        "Platforms",
    ),
    ("choices.communication", Some("choices"), "Communication"),
    (
        "choices.communication.tone",
        Some("choices.communication"),
        "Tone",
    ),
    (
        "choices.communication.length",
        Some("choices.communication"),
        "Length",
    ),
    (
        "choices.communication.formatting",
        Some("choices.communication"),
        "Formatting",
    ),
    (
        "choices.communication.channels",
        Some("choices.communication"),
        "Channels",
    ),
    ("choices.shopping", Some("choices"), "Shopping"),
    (
        "choices.shopping.brands",
        Some("choices.shopping"),
        "Brands",
    ),
    (
        "choices.shopping.budget",
        Some("choices.shopping"),
        "Budget",
    ),
    (
        "choices.shopping.product_features",
        Some("choices.shopping"),
        "Product features",
    ),
    ("people", None, "People"),
    ("people.family", Some("people"), "Family"),
    ("people.friends", Some("people"), "Friends"),
    ("people.colleagues", Some("people"), "Colleagues"),
    ("people.clients", Some("people"), "Clients"),
    ("people.acquaintances", Some("people"), "Acquaintances"),
    ("work", None, "Work"),
    ("work.projects", Some("work"), "Projects"),
    ("work.decisions", Some("work"), "Decisions"),
    ("work.goals", Some("work"), "Goals"),
    ("work.procedures", Some("work"), "Procedures"),
    ("work.commitments", Some("work"), "Commitments"),
    ("personal", None, "Personal"),
    ("personal.identity", Some("personal"), "Identity"),
    ("personal.routines", Some("personal"), "Routines"),
    ("personal.interests", Some("personal"), "Interests"),
    ("personal.goals", Some("personal"), "Goals"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProfileRecord, ProfileType, SourceEventInput, SourceKind};
    use mega_memory::{CandidateScope, ExtractionRunId};
    use serde_json::json;

    fn manual(content: &str, now_ms: i64) -> ManualMemoryInput {
        ManualMemoryInput {
            content: content.into(),
            memory_type: MemoryType::Preference,
            scope_type: ScopeType::Global,
            scope_key: String::new(),
            scope_display_name: "Global".into(),
            category_slugs: vec!["choices.food.ingredients".into()],
            applicable_app_ids: vec![],
            importance: 0.8,
            sensitivity: Sensitivity::Private,
            valid_from_ms: None,
            valid_until_ms: None,
            now_ms,
        }
    }

    fn profile(profile_type: ProfileType, key: &str) -> ProfileRecord {
        ProfileRecord {
            profile_type,
            key: key.into(),
            stable: json!([key]),
            current: json!([]),
            source_revision: 1,
            generated_at: 10,
        }
    }

    #[test]
    fn unencrypted_store_is_rejected_outside_test_escape_hatch() {
        let error = MemoryStore::open(MemoryStoreConfig {
            path: PathBuf::from(":memory:"),
            encryption_key: None,
            allow_unencrypted_for_tests: false,
        })
        .unwrap_err();
        assert!(matches!(error, StoreError::EncryptionRequired));
    }

    #[test]
    fn migration_seeds_taxonomy_and_fts_is_healthy() {
        let store = MemoryStore::in_memory().unwrap();
        let count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM categories", [], |row| row.get(0))
            .unwrap();
        assert!(count >= 40);
        store.integrity_check().unwrap();
    }

    #[test]
    fn version_one_database_migrates_and_compiled_checksums_are_verified() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_0001).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 1)",
                [],
            )
            .unwrap();
        drop(connection);

        let store =
            MemoryStore::open(MemoryStoreConfig::unencrypted_test_at(path.clone())).unwrap();
        let version: i64 = store
            .connection()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        store
            .connection()
            .execute(
                "UPDATE schema_migrations SET checksum='tampered' WHERE version=1",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.verify_migrations(),
            Err(StoreError::MigrationChecksum { version: 1 })
        ));
    }

    #[test]
    fn newer_database_schema_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.sqlite3");
        {
            let store =
                MemoryStore::open(MemoryStoreConfig::unencrypted_test_at(path.clone())).unwrap();
            store.connection().execute(
                "INSERT INTO schema_migrations(version, applied_at, checksum) VALUES (99, 1, 'future')", [],
            ).unwrap();
        }
        assert!(matches!(
            MemoryStore::open(MemoryStoreConfig::unencrypted_test_at(path)),
            Err(StoreError::NewerSchema { found: 99, .. })
        ));
    }

    #[test]
    fn concurrent_connections_reject_stale_user_mutations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.sqlite3");
        let mut first =
            MemoryStore::open(MemoryStoreConfig::unencrypted_test_at(path.clone())).unwrap();
        let mut second = MemoryStore::open(MemoryStoreConfig::unencrypted_test_at(path)).unwrap();
        let original = first
            .create_manual(
                &manual("User prefers explicit revision checks.", 10),
                "create-revision",
            )
            .unwrap();
        let stale = second.get_memory(&original.id).unwrap().unwrap();
        first
            .edit_manual(
                &original.id,
                original.revision,
                "edit-revision",
                &manual("User requires explicit revision checks.", 20),
            )
            .unwrap();
        assert!(matches!(
            second.delete_memory(
                &stale.id,
                stale.revision,
                DeleteMode::Forget,
                "stale-delete",
                30,
            ),
            Err(StoreError::RevisionConflict { .. })
        ));
    }

    #[test]
    fn manual_create_is_searchable_and_edit_retains_history() {
        let mut store = MemoryStore::in_memory().unwrap();
        let old = store
            .create_manual(
                &manual("User dislikes mushrooms on pizza.", 10),
                "manual-create",
            )
            .unwrap();
        let found = store
            .search(&MemorySearchFilter {
                query: Some("mushrooms".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        let replacement = store
            .edit_manual(
                &old.id,
                old.revision,
                "manual-edit",
                &manual("User now likes mushrooms on pizza.", 20),
            )
            .unwrap();
        assert_eq!(replacement.revision, 2);
        assert_eq!(
            store.get_memory(&old.id).unwrap().unwrap().status,
            MemoryStatus::Superseded
        );
        let active = store
            .search(&MemorySearchFilter {
                query: Some("mushrooms".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            active.iter().map(|memory| &memory.id).collect::<Vec<_>>(),
            vec![&replacement.id]
        );
    }

    #[test]
    fn forgetting_and_deleting_remove_rebuildable_projections() {
        let mut store = MemoryStore::in_memory().unwrap();
        let memory = store
            .create_manual(
                &manual("User dislikes mushrooms on pizza.", 10),
                "manual-create",
            )
            .unwrap();
        store.connection.execute("INSERT INTO memory_embeddings(memory_id, provider, model, dimensions, vector_f32, content_hash, embedded_at) VALUES (?1, 'local', 'test', 1, zeroblob(4), zeroblob(32), 10)", [memory.id.as_str()]).unwrap();
        store
            .delete_memory(
                &memory.id,
                memory.revision,
                DeleteMode::Forget,
                "forget-memory",
                20,
            )
            .unwrap();
        let embeddings: i64 = store
            .connection
            .query_row("SELECT count(*) FROM memory_embeddings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(embeddings, 0);
        store
            .delete_memory(
                &memory.id,
                memory.revision + 1,
                DeleteMode::Permanent,
                "delete-memory",
                30,
            )
            .unwrap();
        let tombstones: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM memory_tombstones WHERE memory_id=?1",
                [memory.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstones, 1);
        store.integrity_check().unwrap();
    }

    #[test]
    fn forgetting_invalidates_only_profiles_affected_by_the_memory() {
        let mut store = MemoryStore::in_memory().unwrap();
        let memory = store
            .create_manual(
                &manual("User dislikes mushrooms on pizza.", 10),
                "manual-create",
            )
            .unwrap();
        for (profile_type, key) in [
            (ProfileType::Global, "global"),
            (ProfileType::Category, "choices.food"),
            (ProfileType::Category, "choices.design"),
            (ProfileType::Category, "choices.%"),
        ] {
            store
                .put_profile(&profile(profile_type, key), "profile-create")
                .unwrap();
        }

        store
            .delete_memory(
                &memory.id,
                memory.revision,
                DeleteMode::Forget,
                "forget-memory",
                20,
            )
            .unwrap();

        assert!(
            store
                .get_profile(ProfileType::Global, "global")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_profile(ProfileType::Category, "choices.food")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_profile(ProfileType::Category, "choices.design")
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .get_profile(ProfileType::Category, "choices.%")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn permanent_delete_preserves_unrelated_profiles() {
        let mut store = MemoryStore::in_memory().unwrap();
        let mut input = manual("Stalky uses encrypted SQLite memory.", 10);
        input.scope_type = ScopeType::Project;
        input.scope_key = "stalky".into();
        input.scope_display_name = "Stalky".into();
        let memory = store.create_manual(&input, "manual-create").unwrap();
        for key in ["stalky", "unrelated-project"] {
            store
                .put_profile(&profile(ProfileType::Project, key), "profile-create")
                .unwrap();
        }

        store
            .delete_memory(
                &memory.id,
                memory.revision,
                DeleteMode::Permanent,
                "delete-memory",
                20,
            )
            .unwrap();

        assert!(
            store
                .get_profile(ProfileType::Project, "stalky")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_profile(ProfileType::Project, "unrelated-project")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn applying_the_same_extraction_plan_is_idempotent() {
        let mut store = MemoryStore::in_memory().unwrap();
        let candidate = MemoryCandidate {
            content: "User prefers concise status updates.".into(),
            memory_type: MemoryType::Preference,
            assertion_mode: AssertionMode::Manual,
            category_slugs: vec!["choices.communication.length".into()],
            scope: CandidateScope {
                scope_type: ScopeType::Global,
                scope_key: String::new(),
                display_name: "Global".into(),
            },
            source_app_ids: vec![],
            applicable_app_ids: vec![],
            entity_mentions: vec![],
            importance: 0.8,
            confidence: 1.0,
            valid_from_ms: None,
            valid_until_ms: None,
            supporting_source_event_ids: vec![],
            sensitivity: Sensitivity::Private,
            from_password_field: false,
        };
        let plan = MemoryMutationPlan::Create {
            extraction_run_id: ExtractionRunId::new("run-1"),
            candidate_index: 0,
            candidate,
            status: MemoryStatus::Active,
        };
        let first = store.apply_plan(&plan, 10).unwrap();
        let retry = store.apply_plan(&plan, 20).unwrap();
        assert!(!first.was_already_applied);
        assert!(retry.was_already_applied);
        assert_eq!(retry.memory_id, first.memory_id);
        let count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM memories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn permanent_delete_removes_exclusive_retained_evidence() {
        let mut store = MemoryStore::in_memory().unwrap();
        let source_id = SourceEventId::new("source-1");
        store
            .insert_source_event(&SourceEventInput {
                id: source_id.clone(),
                correlation_id: "correlation-1".into(),
                source_kind: SourceKind::AssistantConversation,
                app_id: None,
                started_at: 1,
                ended_at: 2,
                redacted_title: None,
                evidence_text: "I prefer concise status updates.".into(),
                sensitivity: Sensitivity::Private,
                redaction_flags: vec![],
                capture_sequence: None,
                ax_sequence: None,
                created_at: 3,
            })
            .unwrap();
        let candidate = MemoryCandidate {
            content: "User prefers concise status updates.".into(),
            memory_type: MemoryType::Preference,
            assertion_mode: AssertionMode::Explicit,
            category_slugs: vec![],
            scope: CandidateScope {
                scope_type: ScopeType::Global,
                scope_key: String::new(),
                display_name: "Global".into(),
            },
            source_app_ids: vec![],
            applicable_app_ids: vec![],
            entity_mentions: vec![],
            importance: 0.8,
            confidence: 1.0,
            valid_from_ms: None,
            valid_until_ms: None,
            supporting_source_event_ids: vec![source_id.clone()],
            sensitivity: Sensitivity::Private,
            from_password_field: false,
        };
        let applied = store
            .apply_plan(
                &MemoryMutationPlan::Create {
                    extraction_run_id: ExtractionRunId::new("run-with-source"),
                    candidate_index: 0,
                    candidate,
                    status: MemoryStatus::Active,
                },
                10,
            )
            .unwrap();
        store
            .delete_memory(
                &applied.memory_id.unwrap(),
                1,
                DeleteMode::Permanent,
                "delete-with-source",
                20,
            )
            .unwrap();
        let sources: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM source_events WHERE id=?1",
                [source_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sources, 0);
    }
}
