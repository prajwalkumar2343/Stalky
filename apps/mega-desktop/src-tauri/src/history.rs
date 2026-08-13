use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mega_capture::{CapturePolicy, CaptureService, PrivacyDecision};
use mega_memory::Sensitivity;
use mega_store::{
    HistoryMediaKind, HistoryRetentionPolicy, TimelineEntry, TimelineEntryInput,
    TimelineSearchFilter, TimelineSourceKind,
};
use serde::{Deserialize, Serialize};
use stalky_accessibility::{AccessibilityNode, AccessibilityService, AccessibilitySnapshot};
use tauri::State;
use uuid::Uuid;

use crate::{audio::AudioVaultService, memory::MemoryService};

const POLL_INTERVAL: Duration = Duration::from_millis(750);
const RETENTION_INTERVAL_MS: i64 = 60 * 60 * 1_000;
const DEFAULT_RETENTION_AGE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const DEFAULT_AUDIO_QUOTA_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const MAX_HISTORY_QUERY_CHARS: usize = 500;
const MAX_HISTORY_RESULTS: usize = 200;
const MAX_DERIVED_TEXT_CHARS: usize = 90_000;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStatus {
    running: bool,
    accessibility_entries: u64,
    ocr_entries: u64,
    rejected_private_observations: u64,
    storage_errors: u64,
    last_error: Option<String>,
    last_persisted_at_ms: Option<i64>,
}

pub struct HistoryService {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<HistoryStatus>>,
    worker: Option<JoinHandle<()>>,
}

impl HistoryService {
    pub fn start(
        memory: MemoryService,
        audio: AudioVaultService,
        capture: CaptureService,
        accessibility: AccessibilityService,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(HistoryStatus {
            running: true,
            ..HistoryStatus::default()
        }));
        let worker_stop = Arc::clone(&stop);
        let worker_status = Arc::clone(&status);
        let worker = thread::Builder::new()
            .name("stalky-history".into())
            .spawn(move || {
                run_history_loop(
                    &memory,
                    &audio,
                    &capture,
                    &accessibility,
                    &worker_stop,
                    &worker_status,
                );
            })
            .map_err(|error| format!("could not start history worker: {error}"))?;
        Ok(Self {
            stop,
            status,
            worker: Some(worker),
        })
    }

    fn status(&self) -> HistoryStatus {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Drop for HistoryService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySearchRequest {
    query: Option<String>,
    media_kind: Option<String>,
    source_kind: Option<String>,
    bundle_identifier: Option<String>,
    from_ms: Option<i64>,
    until_ms: Option<i64>,
    limit: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntryDto {
    id: String,
    media_kind: &'static str,
    source_kind: &'static str,
    bundle_identifier: Option<String>,
    app_display_name: Option<String>,
    started_at_ms: i64,
    ended_at_ms: i64,
    text_content: Option<String>,
    sensitivity: &'static str,
    audio_byte_size: Option<u64>,
    audio_duration_ms: Option<u64>,
}

impl From<TimelineEntry> for TimelineEntryDto {
    fn from(entry: TimelineEntry) -> Self {
        Self {
            id: entry.id,
            media_kind: media_kind_name(entry.media_kind),
            source_kind: source_kind_name(entry.source_kind),
            bundle_identifier: entry.bundle_identifier,
            app_display_name: entry.app_display_name,
            started_at_ms: entry.started_at_ms,
            ended_at_ms: entry.ended_at_ms,
            text_content: entry.text_content,
            sensitivity: match entry.sensitivity {
                Sensitivity::Public => "public",
                Sensitivity::Private => "private",
                Sensitivity::Sensitive => "sensitive",
            },
            audio_byte_size: entry.audio_asset.as_ref().map(|asset| asset.byte_size),
            audio_duration_ms: entry.audio_asset.map(|asset| asset.duration_ms),
        }
    }
}

#[tauri::command]
pub fn history_status(service: State<'_, HistoryService>) -> HistoryStatus {
    service.status()
}

#[tauri::command]
pub async fn history_search(
    memory: State<'_, MemoryService>,
    request: HistorySearchRequest,
) -> Result<Vec<TimelineEntryDto>, String> {
    validate_search(&request)?;
    let memory = memory.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let media_kind = request
            .media_kind
            .as_deref()
            .map(parse_media_kind)
            .transpose()?;
        let source_kind = request
            .source_kind
            .as_deref()
            .map(parse_source_kind)
            .transpose()?;
        memory
            .with_store(|store| {
                store.search_timeline(&TimelineSearchFilter {
                    query: request.query,
                    media_kind,
                    bundle_identifier: request.bundle_identifier,
                    source_kind,
                    from_ms: request.from_ms,
                    until_ms: request.until_ms,
                    include_deleted: false,
                    limit: request.limit,
                })
            })
            .map(|entries| entries.into_iter().map(Into::into).collect())
            .map_err(|error| format!("history search failed: {error:?}"))
    })
    .await
    .map_err(|error| format!("history search worker failed: {error}"))?
}

#[tauri::command]
pub async fn history_delete(memory: State<'_, MemoryService>, id: String) -> Result<(), String> {
    if id.trim().is_empty() || id.chars().count() > 256 {
        return Err("invalid history entry id".into());
    }
    let memory = memory.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        memory
            .with_store(|store| store.delete_timeline_entry(&id, now_millis()))
            .map_err(|error| format!("history deletion failed: {error:?}"))
    })
    .await
    .map_err(|error| format!("history deletion worker failed: {error}"))?
}

fn run_history_loop(
    memory: &MemoryService,
    audio: &AudioVaultService,
    capture: &CaptureService,
    accessibility: &AccessibilityService,
    stop: &AtomicBool,
    status: &Mutex<HistoryStatus>,
) {
    let session_id = Uuid::now_v7();
    let policy = CapturePolicy::default();
    let mut last_ax_generation = None;
    let mut last_rejected_generation = None;
    let mut last_ocr_sequence = None;
    // Preserve lazy Keychain access at launch. Retention begins after the
    // service has been alive for one interval or a history operation opens
    // the store first.
    let mut last_retention_ms = now_millis();

    while !stop.load(Ordering::Acquire) {
        let accessibility_status = accessibility.status().ok();
        let snapshot = accessibility_status
            .as_ref()
            .and_then(|value| value.snapshot.as_ref());
        let bundle = snapshot
            .and_then(|value| value.application.as_ref())
            .and_then(|value| value.bundle_identifier.as_deref());
        let secure = snapshot.is_some_and(snapshot_contains_secure_field);
        let allowed = matches!(
            policy.decide_accessibility(bundle, secure),
            PrivacyDecision::Allow | PrivacyDecision::Redact { .. }
        );
        let ax_text = snapshot.and_then(derived_accessibility_text);

        if !allowed
            && let Some(snapshot) = snapshot
            && last_rejected_generation != Some(snapshot.generation)
        {
            increment(status, |value| {
                value.rejected_private_observations =
                    value.rejected_private_observations.saturating_add(1);
            });
            last_rejected_generation = Some(snapshot.generation);
        } else if let (Some(snapshot), Some(text)) = (snapshot, ax_text.as_deref())
            && last_ax_generation != Some(snapshot.generation)
        {
            let result = persist_text(
                memory,
                &session_id,
                snapshot,
                text,
                TimelineSourceKind::Accessibility,
                None,
            );
            record_result(status, result, true);
            last_ax_generation = Some(snapshot.generation);
        }

        if allowed
            && ax_text.is_none()
            && let Ok(capture_status) = capture.status()
            && let Some(ocr) = capture_status.latest_ocr
            && last_ocr_sequence != Some(ocr.frame_sequence)
        {
            if let Some(snapshot) = snapshot {
                let result = persist_text(
                    memory,
                    &session_id,
                    snapshot,
                    &ocr.text,
                    TimelineSourceKind::Ocr,
                    Some(ocr.frame_sequence),
                );
                record_result(status, result, false);
            }
            last_ocr_sequence = Some(ocr.frame_sequence);
        }

        let now = now_millis();
        if now.saturating_sub(last_retention_ms) >= RETENTION_INTERVAL_MS {
            let result = memory.with_store(|store| {
                store.apply_history_retention(
                    &HistoryRetentionPolicy {
                        max_age_ms: Some(DEFAULT_RETENTION_AGE_MS),
                        max_audio_bytes: Some(DEFAULT_AUDIO_QUOTA_BYTES),
                    },
                    now,
                )
            });
            if let Err(error) = result {
                record_error(status, format!("history retention failed: {error:?}"));
            }
            reconcile_audio_deletions(memory, audio, status, now);
            last_retention_ms = now;
        }

        thread::park_timeout(POLL_INTERVAL);
    }
    increment(status, |value| value.running = false);
}

fn reconcile_audio_deletions(
    memory: &MemoryService,
    audio: &AudioVaultService,
    status: &Mutex<HistoryStatus>,
    now_ms: i64,
) {
    let owner = format!("desktop-history-{}", Uuid::now_v7());
    let assets = memory.with_store(|store| {
        store.recover_stale_audio_assets(now_ms, 0)?;
        store.claim_audio_asset_recovery(&owner, now_ms, 60_000, 100)
    });
    let Ok(assets) = assets else {
        record_error(
            status,
            format!("audio recovery claim failed: {:?}", assets.err()),
        );
        return;
    };
    for asset in assets {
        match crate::audio::delete_asset(audio, &asset.id) {
            Ok(()) => {
                if let Err(error) = memory.with_store(|store| {
                    store.finalize_audio_asset_deletion(&asset.id, &owner, now_millis())
                }) {
                    record_error(status, format!("audio deletion finalize failed: {error:?}"));
                }
            }
            Err(error) => {
                let now = now_millis();
                let _ = memory.with_store(|store| {
                    store.fail_audio_asset_recovery(&asset.id, &owner, &error, now)
                });
                record_error(status, format!("audio vault deletion failed: {error}"));
            }
        }
    }
}

fn persist_text(
    memory: &MemoryService,
    session_id: &Uuid,
    snapshot: &AccessibilitySnapshot,
    text: &str,
    source_kind: TimelineSourceKind,
    capture_sequence: Option<u64>,
) -> Result<i64, String> {
    let observed = i64::try_from(snapshot.observed_at_millis).unwrap_or(i64::MAX);
    let suffix = capture_sequence.unwrap_or(snapshot.generation);
    let source = source_kind_name(source_kind);
    let input = TimelineEntryInput {
        id: Uuid::now_v7().to_string(),
        idempotency_key: format!("{session_id}:{source}:{suffix}"),
        media_kind: HistoryMediaKind::Text,
        source_kind,
        bundle_identifier: snapshot
            .application
            .as_ref()
            .and_then(|value| value.bundle_identifier.clone()),
        app_display_name: snapshot
            .application
            .as_ref()
            .and_then(|value| value.name.clone()),
        redacted_window_title: None,
        started_at_ms: observed,
        ended_at_ms: observed,
        text_content: Some(text.to_owned()),
        capture_sequence: capture_sequence.and_then(|value| i64::try_from(value).ok()),
        ax_sequence: i64::try_from(snapshot.generation).ok(),
        sensitivity: Sensitivity::Private,
        created_at_ms: now_millis(),
        audio_asset: None,
    };
    memory
        .with_store(|store| store.admit_timeline_entry(&input))
        .map(|_| input.created_at_ms)
        .map_err(|error| format!("history admission failed: {error:?}"))
}

fn derived_accessibility_text(snapshot: &AccessibilitySnapshot) -> Option<String> {
    let root = snapshot
        .tree
        .as_ref()
        .or(snapshot.focused_window.as_ref())?;
    let mut output = String::new();
    append_node_text(root, &mut output);
    let output = output.trim();
    (!output.is_empty()).then(|| output.to_owned())
}

fn append_node_text(node: &AccessibilityNode, output: &mut String) {
    for value in [node.title.as_deref(), node.value.as_deref()]
        .into_iter()
        .flatten()
    {
        let value = value.trim();
        if value.is_empty() || output.lines().last() == Some(value) {
            continue;
        }
        let extra = usize::from(!output.is_empty()) + value.chars().count();
        if output.chars().count().saturating_add(extra) > MAX_DERIVED_TEXT_CHARS {
            return;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(value);
    }
    for child in &node.children {
        append_node_text(child, output);
    }
}

fn snapshot_contains_secure_field(snapshot: &AccessibilitySnapshot) -> bool {
    snapshot
        .tree
        .as_ref()
        .or(snapshot.focused_window.as_ref())
        .is_some_and(node_contains_secure_field)
}

fn node_contains_secure_field(node: &AccessibilityNode) -> bool {
    node.role
        .as_deref()
        .is_some_and(|role| role.eq_ignore_ascii_case("AXSecureTextField"))
        || node.children.iter().any(node_contains_secure_field)
}

fn record_result(status: &Mutex<HistoryStatus>, result: Result<i64, String>, accessibility: bool) {
    match result {
        Ok(persisted_at) => increment(status, |value| {
            if accessibility {
                value.accessibility_entries = value.accessibility_entries.saturating_add(1);
            } else {
                value.ocr_entries = value.ocr_entries.saturating_add(1);
            }
            value.last_persisted_at_ms = Some(persisted_at);
            value.last_error = None;
        }),
        Err(error) => record_error(status, error),
    }
}

fn record_error(status: &Mutex<HistoryStatus>, error: String) {
    increment(status, |value| {
        value.storage_errors = value.storage_errors.saturating_add(1);
        value.last_error = Some(error.chars().take(256).collect());
    });
}

fn increment(status: &Mutex<HistoryStatus>, update: impl FnOnce(&mut HistoryStatus)) {
    update(
        &mut status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
}

fn validate_search(request: &HistorySearchRequest) -> Result<(), String> {
    if request.limit == 0 || request.limit > MAX_HISTORY_RESULTS {
        return Err("history result limit must be between 1 and 200".into());
    }
    if request.query.as_ref().is_some_and(|query| {
        query.trim().is_empty() || query.chars().count() > MAX_HISTORY_QUERY_CHARS
    }) {
        return Err("history query must contain 1 to 500 characters".into());
    }
    if request
        .from_ms
        .zip(request.until_ms)
        .is_some_and(|(from, until)| until < from)
    {
        return Err("history date range is invalid".into());
    }
    Ok(())
}

fn parse_media_kind(value: &str) -> Result<HistoryMediaKind, String> {
    match value {
        "text" => Ok(HistoryMediaKind::Text),
        "audio" => Ok(HistoryMediaKind::Audio),
        _ => Err("history media kind must be text or audio".into()),
    }
}

fn parse_source_kind(value: &str) -> Result<TimelineSourceKind, String> {
    match value {
        "accessibility" => Ok(TimelineSourceKind::Accessibility),
        "ocr" => Ok(TimelineSourceKind::Ocr),
        "audio_capture" => Ok(TimelineSourceKind::AudioCapture),
        "audio_transcript" => Ok(TimelineSourceKind::AudioTranscript),
        "assistant_conversation" => Ok(TimelineSourceKind::AssistantConversation),
        "manual" => Ok(TimelineSourceKind::Manual),
        "structured_import" => Ok(TimelineSourceKind::StructuredImport),
        _ => Err("invalid history source kind".into()),
    }
}

const fn media_kind_name(value: HistoryMediaKind) -> &'static str {
    match value {
        HistoryMediaKind::Text => "text",
        HistoryMediaKind::Audio => "audio",
    }
}

const fn source_kind_name(value: TimelineSourceKind) -> &'static str {
    match value {
        TimelineSourceKind::Accessibility => "accessibility",
        TimelineSourceKind::Ocr => "ocr",
        TimelineSourceKind::AudioCapture => "audio_capture",
        TimelineSourceKind::AudioTranscript => "audio_transcript",
        TimelineSourceKind::AssistantConversation => "assistant_conversation",
        TimelineSourceKind::Manual => "manual",
        TimelineSourceKind::StructuredImport => "structured_import",
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stalky_accessibility::AccessibilityNode;

    fn node(role: &str, title: Option<&str>, value: Option<&str>) -> AccessibilityNode {
        AccessibilityNode {
            element: None,
            role: Some(role.into()),
            subrole: None,
            title: title.map(str::to_owned),
            value: value.map(str::to_owned),
            bounds: None,
            enabled: None,
            focused: None,
            children_count: 0,
            children: Vec::new(),
            truncated: false,
            supported_actions: Vec::new(),
            value_settable: false,
        }
    }

    #[test]
    fn derived_text_deduplicates_adjacent_values() {
        let mut root = node("AXGroup", Some("Inbox"), None);
        root.children
            .push(node("AXStaticText", Some("Inbox"), None));
        root.children
            .push(node("AXStaticText", Some("Message"), None));
        let mut output = String::new();
        append_node_text(&root, &mut output);
        assert_eq!(output, "Inbox\nMessage");
    }

    #[test]
    fn secure_fields_fail_closed() {
        let mut root = node("AXGroup", None, None);
        root.children
            .push(node("AXSecureTextField", None, Some("secret")));
        assert!(node_contains_secure_field(&root));
    }
}
