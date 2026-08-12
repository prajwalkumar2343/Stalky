ALTER TABLE schema_migrations ADD COLUMN checksum TEXT;
ALTER TABLE entities ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

CREATE TABLE extraction_jobs (
    id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL UNIQUE REFERENCES activity_segments(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'needs_attention')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 3),
    next_attempt_at INTEGER NOT NULL,
    lease_owner TEXT,
    lease_expires_at INTEGER,
    extractor_prompt_version TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    latency_ms INTEGER CHECK (latency_ms IS NULL OR latency_ms >= 0),
    input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
    output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
    private_content_left_device INTEGER NOT NULL DEFAULT 0 CHECK (private_content_left_device IN (0, 1)),
    last_error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK ((state = 'running') = (lease_owner IS NOT NULL)),
    CHECK ((state = 'running') = (lease_expires_at IS NOT NULL))
);

CREATE INDEX extraction_jobs_claimable
    ON extraction_jobs(state, next_attempt_at, lease_expires_at, created_at);

CREATE TABLE memory_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL CHECK (event_type IN (
        'segment_closed', 'extraction_started', 'extraction_completed', 'extraction_failed',
        'candidate_rejected', 'reconciliation_applied', 'reconciliation_failed',
        'profile_regenerated', 'profile_failed', 'context_assembled', 'memory_confirmed',
        'memory_rejected', 'memory_edited', 'memory_forgotten', 'memory_deleted', 'entity_merged'
    )),
    correlation_id TEXT NOT NULL,
    memory_id TEXT,
    segment_id TEXT,
    outcome TEXT,
    occurred_at INTEGER NOT NULL
);

CREATE INDEX memory_events_timeline ON memory_events(occurred_at DESC, event_type);

CREATE TABLE memory_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    extraction_paused INTEGER NOT NULL DEFAULT 0 CHECK (extraction_paused IN (0, 1)),
    source_retention_days INTEGER NOT NULL DEFAULT 30 CHECK (source_retention_days BETWEEN 1 AND 3650),
    audit_retention_days INTEGER NOT NULL DEFAULT 7 CHECK (audit_retention_days BETWEEN 1 AND 3650),
    updated_at INTEGER NOT NULL
);

INSERT INTO memory_settings(id, updated_at) VALUES (1, unixepoch('subsec') * 1000);

CREATE TABLE extraction_policies (
    policy_type TEXT NOT NULL CHECK (policy_type IN ('app', 'window', 'category')),
    policy_key TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (policy_type, policy_key)
);

CREATE TABLE projection_jobs (
    projection_type TEXT NOT NULL CHECK (projection_type IN ('fts', 'embedding', 'profile')),
    projection_key TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'running', 'completed', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (projection_type, projection_key)
);
