INSERT OR IGNORE INTO timeline_entries (
    id,
    idempotency_key,
    media_kind,
    source_kind,
    bundle_identifier,
    app_display_name,
    redacted_window_title,
    started_at_ms,
    ended_at_ms,
    text_content,
    capture_sequence,
    ax_sequence,
    sensitivity,
    created_at_ms,
    updated_at_ms
)
SELECT
    'legacy-source:' || se.id,
    'legacy-source-event:' || se.id,
    'text',
    CASE se.source_kind
        WHEN 'accessibility_segment' THEN 'accessibility'
        WHEN 'audio_transcript_segment' THEN 'audio_transcript'
        WHEN 'assistant_conversation' THEN 'assistant_conversation'
        WHEN 'manual_entry' THEN 'manual'
        ELSE 'structured_import'
    END,
    apps.bundle_identifier,
    apps.display_name,
    se.redacted_title,
    se.started_at,
    se.ended_at,
    NULLIF(se.evidence_text, ''),
    se.capture_sequence,
    se.ax_sequence,
    se.sensitivity,
    se.created_at,
    se.created_at
FROM source_events se
LEFT JOIN apps ON apps.id = se.app_id
WHERE se.source_kind IN (
    'accessibility_segment',
    'audio_transcript_segment',
    'assistant_conversation',
    'manual_entry',
    'structured_import'
);
