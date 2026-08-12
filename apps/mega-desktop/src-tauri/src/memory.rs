use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use mega_ipc::{
    ConfirmMemoryRequest, CreateManualMemoryRequest, DeleteMemoryRequest, EditMemoryRequest,
    MemoryCommandError, MemoryContextDto, MemoryContextRequestDto, MemoryDeleteMode,
    MemoryErrorCode, MemorySearchRequest, RejectMemoryRequest,
};
use mega_memory::{HistoricalMode, Memory, MemoryContextRequest};
use mega_store::{
    DeleteMode, ManualMemoryInput, MemorySearchFilter, MemoryStore, MemoryStoreConfig, StoreError,
};
use tauri::{AppHandle, Manager, State};

const KEYCHAIN_SERVICE: &str = "com.stalky.desktop.memory";
const KEYCHAIN_ACCOUNT: &str = "sqlite-key-v1";

pub struct MemoryService {
    store: Arc<Mutex<Result<MemoryStore, String>>>,
}

impl MemoryService {
    pub fn initialize(app: &AppHandle) -> Self {
        let store = memory_database_path(app).and_then(open_encrypted_store);
        if let Err(error) = &store {
            eprintln!("Structured memory is unavailable; capture remains operational: {error}");
        }
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut MemoryStore) -> Result<T, StoreError>,
    ) -> Result<T, MemoryCommandError> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| memory_unavailable("Memory storage lock is unavailable.".into()))?;
        let store = guard
            .as_mut()
            .map_err(|error| memory_unavailable(error.clone()))?;
        operation(store).map_err(memory_error_from_store)
    }
}

impl Clone for MemoryService {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
        }
    }
}

fn memory_unavailable(message: String) -> MemoryCommandError {
    MemoryCommandError {
        code: MemoryErrorCode::MemoryUnavailable,
        message,
        retryable: true,
    }
}

fn memory_error_from_store(error: StoreError) -> MemoryCommandError {
    match error {
        StoreError::InvalidInput(message) => MemoryCommandError {
            code: MemoryErrorCode::InvalidRequest,
            message: message.to_owned(),
            retryable: false,
        },
        StoreError::NotFound => MemoryCommandError {
            code: MemoryErrorCode::NotFound,
            message: "The memory no longer exists.".into(),
            retryable: false,
        },
        StoreError::RevisionConflict { .. } => MemoryCommandError {
            code: MemoryErrorCode::RevisionConflict,
            message: error.to_string(),
            retryable: false,
        },
        StoreError::EncryptionRequired | StoreError::CipherUnavailable => MemoryCommandError {
            code: MemoryErrorCode::EncryptionUnavailable,
            message: "Encrypted memory storage is unavailable.".into(),
            retryable: true,
        },
        StoreError::LeaseLost => MemoryCommandError {
            code: MemoryErrorCode::LeaseLost,
            message: error.to_string(),
            retryable: true,
        },
        _ => MemoryCommandError {
            code: MemoryErrorCode::StorageFailure,
            message: "Stalky could not complete the memory operation.".into(),
            retryable: true,
        },
    }
}

#[tauri::command]
pub async fn memory_create_manual(
    service: State<'_, MemoryService>,
    request: CreateManualMemoryRequest,
) -> Result<Memory, MemoryCommandError> {
    validate_correlation_id(&request.correlation_id)?;
    let now_ms = now_millis()?;
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            store.create_manual(
                &ManualMemoryInput {
                    content: request.content,
                    memory_type: request.memory_type,
                    scope_type: request.scope_type,
                    scope_key: request.scope_key,
                    scope_display_name: request.scope_display_name,
                    category_slugs: request.category_slugs,
                    applicable_app_ids: request.applicable_app_ids,
                    importance: request.importance,
                    sensitivity: request.sensitivity,
                    valid_from_ms: request.valid_from_ms,
                    valid_until_ms: request.valid_until_ms,
                    now_ms,
                },
                &request.correlation_id,
            )
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

#[tauri::command]
pub async fn memory_search(
    service: State<'_, MemoryService>,
    request: MemorySearchRequest,
) -> Result<Vec<Memory>, MemoryCommandError> {
    if request
        .query
        .as_ref()
        .is_some_and(|query| query.chars().count() > 500)
    {
        return Err(MemoryCommandError {
            code: MemoryErrorCode::InvalidRequest,
            message: "Memory search query exceeds 500 characters.".into(),
            retryable: false,
        });
    }
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            store.search(&MemorySearchFilter {
                query: request.query,
                app_id: request.app_id,
                app_role: Default::default(),
                category_slug: request.category_slug,
                scope_type: request.scope_type,
                scope_key: request.scope_key,
                entity_id: request.entity_id,
                memory_type: request.memory_type,
                status: request.status,
                include_history: request.historical_mode == HistoricalMode::IncludeHistory,
                from_ms: request.from_ms,
                until_ms: request.until_ms,
                limit: request.limit,
            })
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

#[tauri::command]
pub async fn memory_delete(
    service: State<'_, MemoryService>,
    request: DeleteMemoryRequest,
) -> Result<(), MemoryCommandError> {
    validate_correlation_id(&request.correlation_id)?;
    let now = now_millis()?;
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            store.delete_memory(
                &request.memory_id,
                request.expected_revision,
                match request.mode {
                    MemoryDeleteMode::Forget => DeleteMode::Forget,
                    MemoryDeleteMode::Permanent => DeleteMode::Permanent,
                },
                &request.correlation_id,
                now,
            )
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

#[tauri::command]
pub async fn memory_edit(
    service: State<'_, MemoryService>,
    request: EditMemoryRequest,
) -> Result<Memory, MemoryCommandError> {
    validate_correlation_id(&request.replacement.correlation_id)?;
    let now_ms = now_millis()?;
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            let input = manual_input(request.replacement, now_ms);
            store.edit_manual(
                &request.memory_id,
                request.expected_revision,
                &input.1,
                &input.0,
            )
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

#[tauri::command]
pub async fn memory_confirm(
    service: State<'_, MemoryService>,
    request: ConfirmMemoryRequest,
) -> Result<u32, MemoryCommandError> {
    validate_correlation_id(&request.correlation_id)?;
    let now_ms = now_millis()?;
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            store.confirm_pending(
                &request.memory_id,
                request.expected_revision,
                &request.correlation_id,
                now_ms,
            )
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

#[tauri::command]
pub async fn memory_reject(
    service: State<'_, MemoryService>,
    request: RejectMemoryRequest,
) -> Result<u32, MemoryCommandError> {
    validate_correlation_id(&request.correlation_id)?;
    let now_ms = now_millis()?;
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            store.reject_pending(
                &request.memory_id,
                request.expected_revision,
                &request.correlation_id,
                now_ms,
            )
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

#[tauri::command]
pub async fn memory_context(
    service: State<'_, MemoryService>,
    request: MemoryContextRequestDto,
) -> Result<MemoryContextDto, MemoryCommandError> {
    validate_correlation_id(&request.correlation_id)?;
    let now_ms = now_millis()?;
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_store(|store| {
            let assembly = store.assemble_context(
                &MemoryContextRequest {
                    current_app_id: request.current_app_id,
                    active_project_key: request.project_key,
                    query_text: request.query_text,
                    mentioned_entity_ids: request.mentioned_entity_ids,
                    historical_mode: request.historical_mode,
                    sensitivity_allowance: request.sensitivity_allowance,
                    total_token_budget: request.token_budget,
                    temporal_query: request.temporal_query,
                },
                &request.correlation_id,
                now_ms,
            )?;
            Ok(MemoryContextDto {
                rendered: assembly.rendered,
                memory_ids: assembly.memory_ids,
                estimated_tokens: assembly.estimated_tokens,
            })
        })
    })
    .await
    .map_err(|_| memory_unavailable("The memory worker stopped unexpectedly.".into()))?
}

fn manual_input(request: CreateManualMemoryRequest, now_ms: i64) -> (ManualMemoryInput, String) {
    let correlation_id = request.correlation_id;
    (
        ManualMemoryInput {
            content: request.content,
            memory_type: request.memory_type,
            scope_type: request.scope_type,
            scope_key: request.scope_key,
            scope_display_name: request.scope_display_name,
            category_slugs: request.category_slugs,
            applicable_app_ids: request.applicable_app_ids,
            importance: request.importance,
            sensitivity: request.sensitivity,
            valid_from_ms: request.valid_from_ms,
            valid_until_ms: request.valid_until_ms,
            now_ms,
        },
        correlation_id,
    )
}

fn validate_correlation_id(value: &str) -> Result<(), MemoryCommandError> {
    if value.trim().is_empty() || value.chars().count() > 128 {
        return Err(MemoryCommandError {
            code: MemoryErrorCode::InvalidRequest,
            message: "A bounded correlation ID is required.".into(),
            retryable: false,
        });
    }
    Ok(())
}

fn memory_database_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|directory| directory.join("memory").join("stalky-memory.sqlite3"))
        .map_err(|error| format!("could not resolve the protected memory directory: {error}"))
}

#[cfg(target_os = "macos")]
fn open_encrypted_store(path: PathBuf) -> Result<MemoryStore, String> {
    use getrandom::fill;
    use security_framework::passwords::{get_generic_password, set_generic_password};

    let bytes = match get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(bytes) => bytes,
        Err(error) if error.code() == -25300 => {
            let mut generated = vec![0_u8; 32];
            fill(&mut generated)
                .map_err(|error| format!("could not generate a memory encryption key: {error}"))?;
            set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &generated)
                .map_err(|error| format!("could not store the memory key in Keychain: {error}"))?;
            generated
        }
        Err(error) => {
            return Err(format!(
                "could not read the memory key from Keychain: {error}"
            ));
        }
    };
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "the Keychain memory key has an invalid length".to_owned())?;
    MemoryStore::open(MemoryStoreConfig::encrypted(path, key)).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn open_encrypted_store(_path: PathBuf) -> Result<MemoryStore, String> {
    Err("encrypted structured memory is available only in the macOS desktop build".to_owned())
}

fn now_millis() -> Result<i64, MemoryCommandError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| memory_unavailable("The system clock is invalid.".into()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| memory_unavailable("The system clock is outside the supported range.".into()))
}
