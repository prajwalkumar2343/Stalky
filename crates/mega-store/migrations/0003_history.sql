CREATE TABLE timeline_entries (
    row_id INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    idempotency_key TEXT NOT NULL UNIQUE,
    media_kind TEXT NOT NULL CHECK (media_kind IN ('text', 'audio')),
    source_kind TEXT NOT NULL CHECK (source_kind IN (
        'accessibility', 'ocr', 'audio_transcript', 'assistant_conversation',
        'manual', 'structured_import'
    )),
    bundle_identifier TEXT,
    app_display_name TEXT,
    redacted_window_title TEXT,
    started_at_ms INTEGER NOT NULL,
    ended_at_ms INTEGER NOT NULL CHECK (ended_at_ms >= started_at_ms),
    text_content TEXT,
    capture_sequence INTEGER,
    ax_sequence INTEGER,
    sensitivity TEXT NOT NULL CHECK (sensitivity IN ('public', 'private', 'sensitive')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'deleted')),
    deleted_at_ms INTEGER,
    CHECK (bundle_identifier IS NULL OR length(bundle_identifier) <= 512),
    CHECK (app_display_name IS NULL OR length(app_display_name) <= 500),
    CHECK (redacted_window_title IS NULL OR length(redacted_window_title) <= 500),
    CHECK (text_content IS NULL OR length(text_content) <= 100000),
    CHECK ((status = 'active' AND deleted_at_ms IS NULL) OR
           (status = 'deleted' AND deleted_at_ms IS NOT NULL))
);

CREATE INDEX timeline_entries_order
    ON timeline_entries(status, started_at_ms DESC, row_id DESC);

CREATE TABLE audio_assets (
    id TEXT PRIMARY KEY,
    timeline_entry_id INTEGER NOT NULL UNIQUE REFERENCES timeline_entries(row_id),
    storage_path TEXT,
    object_key TEXT,
    byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
    duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0),
    status TEXT NOT NULL CHECK (status IN (
        'staged', 'ready', 'deleting', 'orphaned', 'failed', 'deleted'
    )),
    recovery_attempts INTEGER NOT NULL DEFAULT 0 CHECK (recovery_attempts >= 0),
    lease_owner TEXT,
    lease_expires_at_ms INTEGER,
    last_error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    deleted_at_ms INTEGER,
    CHECK (storage_path IS NOT NULL OR object_key IS NOT NULL),
    CHECK (storage_path IS NULL OR length(storage_path) <= 4096),
    CHECK (object_key IS NULL OR length(object_key) <= 1024),
    CHECK (last_error IS NULL OR length(last_error) <= 2000),
    CHECK ((status = 'deleted' AND deleted_at_ms IS NOT NULL) OR
           (status <> 'deleted' AND deleted_at_ms IS NULL))
);

CREATE INDEX audio_assets_recovery
    ON audio_assets(status, lease_expires_at_ms, updated_at_ms, id);

CREATE TRIGGER audio_assets_audio_parent_insert
BEFORE INSERT ON audio_assets
WHEN (SELECT media_kind FROM timeline_entries WHERE row_id = NEW.timeline_entry_id) <> 'audio'
BEGIN
    SELECT RAISE(ABORT, 'audio asset requires an audio timeline entry');
END;

CREATE TRIGGER audio_assets_audio_parent_update
BEFORE UPDATE OF timeline_entry_id ON audio_assets
WHEN (SELECT media_kind FROM timeline_entries WHERE row_id = NEW.timeline_entry_id) <> 'audio'
BEGIN
    SELECT RAISE(ABORT, 'audio asset requires an audio timeline entry');
END;

CREATE TABLE timeline_search_documents (
    row_id INTEGER PRIMARY KEY REFERENCES timeline_entries(row_id) ON DELETE CASCADE,
    entry_id TEXT NOT NULL UNIQUE REFERENCES timeline_entries(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    text_content TEXT NOT NULL
);

CREATE VIRTUAL TABLE timeline_fts USING fts5(
    title,
    text_content,
    content='timeline_search_documents',
    content_rowid='row_id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER timeline_search_documents_ai AFTER INSERT ON timeline_search_documents BEGIN
    INSERT INTO timeline_fts(rowid, title, text_content)
    VALUES (new.row_id, new.title, new.text_content);
END;

CREATE TRIGGER timeline_search_documents_ad AFTER DELETE ON timeline_search_documents BEGIN
    INSERT INTO timeline_fts(timeline_fts, rowid, title, text_content)
    VALUES ('delete', old.row_id, old.title, old.text_content);
END;

CREATE TRIGGER timeline_search_documents_au AFTER UPDATE ON timeline_search_documents BEGIN
    INSERT INTO timeline_fts(timeline_fts, rowid, title, text_content)
    VALUES ('delete', old.row_id, old.title, old.text_content);
    INSERT INTO timeline_fts(rowid, title, text_content)
    VALUES (new.row_id, new.title, new.text_content);
END;

CREATE TRIGGER timeline_entries_search_ai AFTER INSERT ON timeline_entries BEGIN
    INSERT INTO timeline_search_documents(row_id, entry_id, title, text_content)
    VALUES (new.row_id, new.id, COALESCE(new.redacted_window_title, ''), COALESCE(new.text_content, ''));
END;

CREATE TRIGGER timeline_entries_search_au AFTER UPDATE OF id, redacted_window_title, text_content ON timeline_entries BEGIN
    INSERT INTO timeline_search_documents(row_id, entry_id, title, text_content)
    VALUES (new.row_id, new.id, COALESCE(new.redacted_window_title, ''), COALESCE(new.text_content, ''))
    ON CONFLICT(row_id) DO UPDATE SET
        entry_id = excluded.entry_id,
        title = excluded.title,
        text_content = excluded.text_content;
END;
