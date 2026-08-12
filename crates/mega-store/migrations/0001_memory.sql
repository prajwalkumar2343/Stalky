PRAGMA foreign_keys = ON;

CREATE TABLE apps (
    id TEXT PRIMARY KEY,
    bundle_identifier TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    identity_confidence REAL NOT NULL DEFAULT 1.0 CHECK (identity_confidence BETWEEN 0 AND 1),
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
);

CREATE TABLE memory_scopes (
    id TEXT PRIMARY KEY,
    scope_type TEXT NOT NULL CHECK (scope_type IN ('global', 'app', 'project', 'entity')),
    scope_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    UNIQUE (scope_type, scope_key)
);

CREATE TABLE source_events (
    id TEXT PRIMARY KEY,
    correlation_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN (
        'accessibility_segment', 'audio_transcript_segment', 'assistant_conversation',
        'manual_entry', 'structured_import'
    )),
    app_id TEXT REFERENCES apps(id),
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL CHECK (ended_at >= started_at),
    redacted_title TEXT,
    evidence_text TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    sensitivity TEXT NOT NULL CHECK (sensitivity IN ('public', 'private', 'sensitive')),
    redaction_flags TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(redaction_flags)),
    capture_sequence INTEGER,
    ax_sequence INTEGER,
    created_at INTEGER NOT NULL,
    UNIQUE (source_kind, content_hash, started_at)
);

CREATE TABLE activity_segments (
    id TEXT PRIMARY KEY,
    app_id TEXT REFERENCES apps(id),
    scope_id TEXT REFERENCES memory_scopes(id),
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL CHECK (ended_at >= started_at),
    close_reason TEXT NOT NULL CHECK (close_reason IN (
        'app_changed', 'project_changed', 'inactivity', 'session_ended',
        'maximum_duration', 'capture_paused', 'shutdown', 'sleep'
    )),
    extraction_state TEXT NOT NULL DEFAULT 'pending' CHECK (extraction_state IN (
        'pending', 'running', 'completed', 'needs_attention'
    )),
    extraction_attempts INTEGER NOT NULL DEFAULT 0 CHECK (extraction_attempts BETWEEN 0 AND 3)
);

CREATE TABLE activity_segment_sources (
    segment_id TEXT NOT NULL REFERENCES activity_segments(id) ON DELETE CASCADE,
    source_event_id TEXT NOT NULL REFERENCES source_events(id) ON DELETE CASCADE,
    PRIMARY KEY (segment_id, source_event_id)
);

CREATE TABLE memories (
    row_id INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    normalized_content TEXT NOT NULL,
    display_content TEXT NOT NULL,
    memory_type TEXT NOT NULL CHECK (memory_type IN ('preference', 'fact', 'decision', 'episode', 'task', 'procedure')),
    assertion_mode TEXT NOT NULL CHECK (assertion_mode IN ('explicit', 'observed', 'inferred', 'imported', 'manual')),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'expired', 'forgotten', 'pending_review', 'rejected')),
    scope_id TEXT NOT NULL REFERENCES memory_scopes(id),
    importance REAL NOT NULL CHECK (importance BETWEEN 0 AND 1),
    confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    sensitivity TEXT NOT NULL CHECK (sensitivity IN ('public', 'private', 'sensitive')),
    valid_from INTEGER,
    valid_until INTEGER,
    extractor_prompt_version TEXT,
    provider TEXT,
    model TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_accessed_at INTEGER,
    access_count INTEGER NOT NULL DEFAULT 0 CHECK (access_count >= 0),
    CHECK (valid_until IS NULL OR valid_from IS NULL OR valid_until >= valid_from)
);

CREATE INDEX memories_active_scope ON memories(scope_id, memory_type, updated_at DESC) WHERE status = 'active';

CREATE TABLE categories (
    id TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES categories(id),
    slug TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL,
    taxonomy_version INTEGER NOT NULL
);

CREATE TABLE memory_categories (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    category_id TEXT NOT NULL REFERENCES categories(id),
    confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    PRIMARY KEY (memory_id, category_id)
);

CREATE TABLE memory_apps (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    app_id TEXT NOT NULL REFERENCES apps(id),
    role TEXT NOT NULL CHECK (role IN ('source', 'applies_to')),
    PRIMARY KEY (memory_id, app_id, role)
);

CREATE TABLE memory_sources (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    source_event_id TEXT NOT NULL REFERENCES source_events(id),
    support_kind TEXT NOT NULL CHECK (support_kind IN ('primary', 'supporting', 'contradicting')),
    PRIMARY KEY (memory_id, source_event_id)
);

CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('person', 'organization', 'project', 'place', 'product')),
    canonical_name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    identity_confidence REAL NOT NULL CHECK (identity_confidence BETWEEN 0 AND 1),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX entities_normalized_name ON entities(entity_type, normalized_name);

CREATE TABLE entity_aliases (
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    normalized_alias TEXT NOT NULL,
    source_event_id TEXT REFERENCES source_events(id),
    confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    PRIMARY KEY (entity_id, normalized_alias)
);

CREATE TABLE memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES entities(id),
    role TEXT NOT NULL CHECK (role IN ('subject', 'object', 'participant', 'mentioned', 'scope')),
    PRIMARY KEY (memory_id, entity_id, role)
);

CREATE TABLE memory_relations (
    from_memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    to_memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    relation_type TEXT NOT NULL CHECK (relation_type IN ('updates', 'extends', 'supports', 'contradicts', 'derived_from')),
    confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (from_memory_id, to_memory_id, relation_type),
    CHECK (from_memory_id <> to_memory_id)
);

CREATE TABLE memory_search_documents (
    row_id INTEGER PRIMARY KEY,
    memory_id TEXT NOT NULL UNIQUE REFERENCES memories(id) ON DELETE CASCADE,
    display_content TEXT NOT NULL,
    category_text TEXT NOT NULL,
    entity_text TEXT NOT NULL,
    app_text TEXT NOT NULL
);

CREATE VIRTUAL TABLE memories_fts USING fts5(
    display_content, category_text, entity_text, app_text,
    content='memory_search_documents', content_rowid='row_id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER memory_search_documents_ai AFTER INSERT ON memory_search_documents BEGIN
    INSERT INTO memories_fts(rowid, display_content, category_text, entity_text, app_text)
    VALUES (new.row_id, new.display_content, new.category_text, new.entity_text, new.app_text);
END;
CREATE TRIGGER memory_search_documents_ad AFTER DELETE ON memory_search_documents BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, display_content, category_text, entity_text, app_text)
    VALUES ('delete', old.row_id, old.display_content, old.category_text, old.entity_text, old.app_text);
END;
CREATE TRIGGER memory_search_documents_au AFTER UPDATE ON memory_search_documents BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, display_content, category_text, entity_text, app_text)
    VALUES ('delete', old.row_id, old.display_content, old.category_text, old.entity_text, old.app_text);
    INSERT INTO memories_fts(rowid, display_content, category_text, entity_text, app_text)
    VALUES (new.row_id, new.display_content, new.category_text, new.entity_text, new.app_text);
END;

CREATE TABLE memory_embeddings (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions > 0),
    vector_f32 BLOB NOT NULL,
    content_hash BLOB NOT NULL CHECK (length(content_hash) = 32),
    embedded_at INTEGER NOT NULL
);

CREATE TABLE memory_profiles (
    id TEXT PRIMARY KEY,
    projection_type TEXT NOT NULL CHECK (projection_type IN ('global', 'app', 'project', 'category', 'entity')),
    projection_key TEXT NOT NULL,
    stable_json TEXT NOT NULL CHECK (json_valid(stable_json)),
    current_json TEXT NOT NULL CHECK (json_valid(current_json)),
    source_revision INTEGER NOT NULL,
    generated_at INTEGER NOT NULL,
    UNIQUE (projection_type, projection_key)
);

CREATE TABLE extraction_candidates (
    extraction_run_id TEXT NOT NULL,
    candidate_index INTEGER NOT NULL CHECK (candidate_index >= 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('create', 'duplicate', 'update', 'extend', 'ignore', 'request_review')),
    memory_id TEXT REFERENCES memories(id) ON DELETE SET NULL,
    audit_reason TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    PRIMARY KEY (extraction_run_id, candidate_index)
);

CREATE TABLE memory_tombstones (
    memory_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL,
    deleted_at INTEGER NOT NULL
);

CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);
