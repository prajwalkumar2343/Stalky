use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    routing::{delete, get, patch, post},
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    auth::Principal,
    credentials::ProviderCredentialVault,
    domain::{
        principal_user_id, validate_agent_run, validate_idempotency_key, validate_memory_create,
        validate_memory_search, validate_profile_patch, validate_record_create,
        validate_record_update, validate_todo_create, validate_todo_update,
    },
    error::AppError,
    protocol::*,
    providers::{ProviderError, ProviderRegistry, ProviderRequest},
    store::{StoreError, StoreHandle},
};

#[derive(Debug, Deserialize)]
pub struct RecordQuery {
    #[serde(rename = "recordType")]
    pub record_type: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeDeps {
    pub providers: ProviderRegistry,
    pub vault: Option<Arc<ProviderCredentialVault>>,
}

pub fn canonical_routes(store: StoreHandle) -> Router {
    resource_routes(store, None).route("/me", get(me))
}

pub fn legacy_routes(store: StoreHandle) -> Router {
    resource_routes(store, None)
        .route("/auth/me", get(me))
        .route("/auth/register", post(provider_unavailable))
        .route("/auth/login", post(provider_unavailable))
        .route("/auth/google", post(provider_unavailable))
        .route("/auth/google/challenge", post(provider_unavailable))
        .route("/auth/refresh", post(provider_unavailable))
        .route("/auth/logout", post(provider_unavailable))
}

pub fn canonical_routes_with_runtime(
    store: StoreHandle,
    providers: ProviderRegistry,
    vault: Option<Arc<ProviderCredentialVault>>,
) -> Router {
    resource_routes(store, Some(RuntimeDeps { providers, vault })).route("/me", get(me))
}

pub fn legacy_routes_with_runtime(
    store: StoreHandle,
    providers: ProviderRegistry,
    vault: Option<Arc<ProviderCredentialVault>>,
) -> Router {
    resource_routes(store, Some(RuntimeDeps { providers, vault }))
        .route("/auth/me", get(me))
        .route("/auth/register", post(provider_unavailable))
        .route("/auth/login", post(provider_unavailable))
        .route("/auth/google", post(provider_unavailable))
        .route("/auth/google/challenge", post(provider_unavailable))
        .route("/auth/refresh", post(provider_unavailable))
        .route("/auth/logout", post(provider_unavailable))
}

fn resource_routes(store: StoreHandle, runtime: Option<RuntimeDeps>) -> Router {
    let mut router = Router::new()
        .route("/profile", get(get_profile).patch(update_profile))
        .route("/memories", get(list_memories).post(create_memory))
        .route("/memories/search", post(search_memories))
        .route("/memories/{memory_id}", delete(delete_memory))
        .route("/todos", get(list_todos).post(create_todo))
        .route("/todos/{todo_id}", patch(update_todo).delete(delete_todo))
        .route(
            "/mini-apps/{mini_app_id}/records",
            get(list_records).post(create_record),
        )
        .route(
            "/mini-apps/{mini_app_id}/records/{record_id}",
            patch(update_record).delete(delete_record),
        )
        .route("/assistant/runs", post(create_agent_run))
        .route("/assistant/runs/{run_id}", get(get_agent_run))
        .route("/assistant/runs/{run_id}/cancel", post(cancel_agent_run))
        .route("/assistant/runs/{run_id}/events", get(list_agent_events))
        .route("/providers/openrouter/models", post(provider_unavailable))
        .route("/mini-apps/build", post(provider_unavailable))
        .route("/mini-apps/revise", post(provider_unavailable))
        .route("/mini-apps/widgets/build", post(provider_unavailable))
        .route("/transcribe", post(provider_unavailable))
        .layer(Extension(store));
    if let Some(runtime) = runtime {
        router = router
            .route("/assistant/chat", post(assistant_chat))
            .layer(Extension(runtime));
    } else {
        router = router.route("/assistant/chat", post(provider_unavailable));
    }
    router
}

async fn me(Extension(principal): Extension<Principal>) -> Result<Json<Principal>, AppError> {
    Ok(Json(principal))
}

async fn get_profile(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Profile>, AppError> {
    let user_id = user_id(&principal)?;
    store
        .get_profile(user_id)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn update_profile(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Json(input): Json<ProfilePatch>,
) -> Result<Json<Profile>, AppError> {
    validate_profile_patch(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    store
        .update_profile(user_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn list_memories(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Memory>>, AppError> {
    let user_id = user_id(&principal)?;
    store
        .list_memories(user_id)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn create_memory(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Json(input): Json<MemoryCreate>,
) -> Result<Json<Memory>, AppError> {
    validate_memory_create(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    store
        .create_memory(user_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn search_memories(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Json(input): Json<MemorySearchRequest>,
) -> Result<Json<Vec<MemorySearchResult>>, AppError> {
    validate_memory_search(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    store
        .search_memories(user_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn delete_memory(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(memory_id): Path<String>,
) -> Result<Json<OkResponse>, AppError> {
    let user_id = user_id(&principal)?;
    let memory_id = parse_uuid(&memory_id, "memory_id")?;
    let deleted = store
        .delete_memory(user_id, memory_id)
        .await
        .map_err(store_error)?;
    if !deleted {
        return Err(AppError::resource_not_found("Memory"));
    }
    Ok(Json(OkResponse { ok: true }))
}

async fn list_todos(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<Todo>>, AppError> {
    let user_id = user_id(&principal)?;
    store
        .list_todos(user_id)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn create_todo(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Json(input): Json<TodoCreate>,
) -> Result<Json<Todo>, AppError> {
    validate_todo_create(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    store
        .create_todo(user_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn update_todo(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(todo_id): Path<String>,
    Json(input): Json<TodoUpdate>,
) -> Result<Json<Todo>, AppError> {
    validate_todo_update(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    let todo_id = parse_uuid(&todo_id, "todo_id")?;
    store
        .update_todo(user_id, todo_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn delete_todo(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(todo_id): Path<String>,
) -> Result<Json<OkResponse>, AppError> {
    let user_id = user_id(&principal)?;
    let todo_id = parse_uuid(&todo_id, "todo_id")?;
    let deleted = store
        .delete_todo(user_id, todo_id)
        .await
        .map_err(store_error)?;
    if !deleted {
        return Err(AppError::resource_not_found("Todo"));
    }
    Ok(Json(OkResponse { ok: true }))
}

async fn list_records(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(mini_app_id): Path<String>,
    Query(query): Query<RecordQuery>,
) -> Result<Json<Vec<MiniAppRecord>>, AppError> {
    validate_mini_app_id(&mini_app_id)?;
    let user_id = user_id(&principal)?;
    store
        .list_records(user_id, &mini_app_id, query.record_type.as_deref())
        .await
        .map(Json)
        .map_err(store_error)
}

async fn create_record(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(mini_app_id): Path<String>,
    Json(mut input): Json<MiniAppRecordCreate>,
) -> Result<Json<MiniAppRecord>, AppError> {
    validate_mini_app_id(&mini_app_id)?;
    input.values = validate_record_create(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    store
        .create_record(user_id, mini_app_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn update_record(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path((mini_app_id, record_id)): Path<(String, String)>,
    Json(mut input): Json<MiniAppRecordUpdate>,
) -> Result<Json<MiniAppRecord>, AppError> {
    validate_mini_app_id(&mini_app_id)?;
    input.values = validate_record_update(&input).map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    let record_id = parse_uuid(&record_id, "record_id")?;
    store
        .update_record(user_id, &mini_app_id, record_id, input)
        .await
        .map(Json)
        .map_err(store_error)
}

async fn delete_record(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path((mini_app_id, record_id)): Path<(String, String)>,
) -> Result<Json<OkResponse>, AppError> {
    validate_mini_app_id(&mini_app_id)?;
    let user_id = user_id(&principal)?;
    let record_id = parse_uuid(&record_id, "record_id")?;
    let deleted = store
        .delete_record(user_id, &mini_app_id, record_id)
        .await
        .map_err(store_error)?;
    if !deleted {
        return Err(AppError::resource_not_found("Mini app record"));
    }
    Ok(Json(OkResponse { ok: true }))
}

async fn create_agent_run(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    runtime: Option<Extension<RuntimeDeps>>,
    headers: axum::http::HeaderMap,
    Json(mut input): Json<AgentRunRequest>,
) -> Result<(StatusCode, Json<AgentRunAccepted>), AppError> {
    validate_agent_run(&input).map_err(validation_error)?;
    let idempotency_key = validate_idempotency_key(
        headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok()),
    )
    .map_err(validation_error)?;
    let user_id = user_id(&principal)?;
    let credential_ciphertext = seal_credential(runtime.as_ref(), &input.api_key)?;
    input.api_key.clear();
    let run = store
        .create_agent_run(user_id, input, idempotency_key, credential_ciphertext)
        .await
        .map_err(store_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AgentRunAccepted {
            run_id: run.id,
            session_id: run.session_id,
            state: run.state,
        }),
    ))
}

async fn assistant_chat(
    Extension(runtime): Extension<RuntimeDeps>,
    Json(input): Json<AgentRunRequest>,
) -> Result<Json<AssistantChatResponse>, AppError> {
    validate_agent_run(&input).map_err(validation_error)?;
    let Some(vault) = runtime.vault.as_ref() else {
        return Err(AppError::service_unavailable(
            "provider.credentials_unconfigured",
            "Provider credential encryption is not configured on this server.",
        ));
    };
    let ciphertext = vault.seal(&input.api_key).map_err(|_| {
        AppError::bad_request(
            "provider.missing_credential",
            "A provider API key is required.",
        )
    })?;
    let secret = vault.open(&ciphertext).map_err(|_| AppError::internal())?;
    let adapter = runtime.providers.get(&input.provider).ok_or_else(|| {
        AppError::bad_request(
            "provider.unsupported",
            "The requested provider is unsupported.",
        )
    })?;
    let response = adapter
        .complete(
            ProviderRequest::from_run(&input),
            &secret,
            CancellationToken::new(),
        )
        .await
        .map_err(provider_error)?;
    Ok(Json(AssistantChatResponse {
        reply: response.reply,
        session_id: input.session_id(),
        emotion: response.emotion,
        created_emotion: response.created_emotion,
        actions: response.actions,
    }))
}

async fn get_agent_run(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<String>,
) -> Result<Json<AgentRunSnapshot>, AppError> {
    let user_id = user_id(&principal)?;
    let run_id = parse_uuid(&run_id, "run_id")?;
    store
        .get_agent_run(user_id, run_id)
        .await
        .map_err(store_error)?
        .map(Json)
        .ok_or_else(|| AppError::resource_not_found("Agent run"))
}

async fn cancel_agent_run(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<String>,
) -> Result<Json<AgentRunSnapshot>, AppError> {
    let user_id = user_id(&principal)?;
    let run_id = parse_uuid(&run_id, "run_id")?;
    store
        .cancel_agent_run(user_id, run_id)
        .await
        .map_err(store_error)?
        .map(Json)
        .ok_or_else(|| AppError::resource_not_found("Agent run"))
}

async fn list_agent_events(
    Extension(store): Extension<StoreHandle>,
    Extension(principal): Extension<Principal>,
    Path(run_id): Path<String>,
) -> Result<Json<Vec<AgentRunEvent>>, AppError> {
    let user_id = user_id(&principal)?;
    let run_id = parse_uuid(&run_id, "run_id")?;
    store
        .list_agent_events(user_id, run_id)
        .await
        .map_err(store_error)?
        .map(Json)
        .ok_or_else(|| AppError::resource_not_found("Agent run"))
}

async fn provider_unavailable() -> Result<(), AppError> {
    Err(AppError::not_implemented(
        "provider.integration_required",
        "This provider-backed endpoint is intentionally unavailable until its Rust provider adapter is configured.",
    ))
}

fn seal_credential(
    runtime: Option<&Extension<RuntimeDeps>>,
    api_key: &str,
) -> Result<Option<String>, AppError> {
    if api_key.trim().is_empty() {
        return Ok(None);
    }
    let Some(runtime) = runtime else {
        // The bare route builder is also used by store-level integration
        // tests. It deliberately drops the client credential and leaves the
        // run for a worker to reject, while the production app always passes
        // the configured vault above.
        return Ok(None);
    };
    let Some(vault) = runtime.vault.as_ref() else {
        return Err(AppError::service_unavailable(
            "provider.credentials_unconfigured",
            "Provider credential encryption is not configured on this server.",
        ));
    };
    vault
        .seal(api_key)
        .map(Some)
        .map_err(|_| AppError::internal())
}

fn provider_error(error: ProviderError) -> AppError {
    match error {
        ProviderError::MissingCredential => AppError::bad_request(
            "provider.missing_credential",
            "A provider API key is required.",
        ),
        ProviderError::Unsupported => AppError::bad_request(
            "provider.unsupported",
            "The requested provider is unsupported.",
        ),
        ProviderError::Cancelled => AppError::conflict(
            "run.cancelled",
            "The provider request was cancelled before it completed.",
        ),
        _ => AppError::service_unavailable(
            error.code(),
            "The provider request did not complete successfully.",
        ),
    }
}

fn user_id(principal: &Principal) -> Result<Uuid, AppError> {
    principal_user_id(principal).map_err(|_| AppError::unauthorized())
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| {
        AppError::bad_request(
            "request.invalid_id",
            format!("{field} must be a valid UUID"),
        )
    })
}

fn validate_mini_app_id(value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > 120 {
        return Err(AppError::bad_request(
            "request.invalid_mini_app_id",
            "mini_app_id must be between 1 and 120 characters",
        ));
    }
    Ok(())
}

fn validation_error(error: impl std::fmt::Display) -> AppError {
    AppError::bad_request("request.invalid", error.to_string())
}

fn store_error(error: StoreError) -> AppError {
    match error {
        StoreError::NotFound => AppError::resource_not_found("Resource"),
        StoreError::Conflict => AppError::conflict(
            "idempotency.conflict",
            "The idempotency key was already used with a different request.",
        ),
        StoreError::Database(_) => AppError::storage_unavailable(),
        StoreError::InvalidData(_) => AppError::internal(),
        StoreError::StaleFence => AppError::conflict(
            "run.stale_fence",
            "The worker lease is no longer valid for this run.",
        ),
        StoreError::LateResult => AppError::conflict(
            "run.late_result",
            "The run already reached a terminal state.",
        ),
        StoreError::Cancelled => AppError::conflict("run.cancelled", "The run was cancelled."),
    }
}
