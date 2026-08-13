use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub struct MemoryCreate {
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

const fn default_search_limit() -> usize {
    8
}

#[derive(Clone, Debug, Serialize)]
pub struct Memory {
    pub id: Uuid,
    pub title: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemorySearchResult {
    pub memory_id: Uuid,
    pub title: String,
    pub chunk_text: String,
    pub score: f32,
    pub source_type: &'static str,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TodoCreate {
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct TodoUpdate {
    pub title: Option<String>,
    pub done: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Todo {
    pub id: Uuid,
    pub title: String,
    pub done: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MiniAppRecordCreate {
    #[serde(rename = "recordType", default = "default_record_type")]
    pub record_type: String,
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

fn default_record_type() -> String {
    "record".to_owned()
}

#[derive(Clone, Debug, Deserialize)]
pub struct MiniAppRecordUpdate {
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MiniAppRecord {
    pub id: Uuid,
    #[serde(rename = "miniAppId")]
    pub mini_app_id: String,
    #[serde(rename = "recordType")]
    pub record_type: String,
    pub values: BTreeMap<String, Value>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentRunRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub memories: Vec<Value>,
    #[serde(default)]
    pub todos: Vec<Value>,
    #[serde(default)]
    pub apps: Vec<Value>,
    #[serde(default)]
    pub mini_apps: Vec<Value>,
    #[serde(default)]
    pub automations: Vec<Value>,
    #[serde(default)]
    pub context_files: Vec<String>,
    #[serde(default)]
    pub image_base64: Option<String>,
    #[serde(default)]
    pub image_mime_type: Option<String>,
}

fn default_provider() -> String {
    "gemini".to_owned()
}

impl AgentRunRequest {
    pub fn session_id(&self) -> Uuid {
        self.session_id
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok())
            .unwrap_or_else(Uuid::now_v7)
    }

    pub fn without_secret(&self) -> Self {
        let mut request = self.clone();
        request.api_key.clear();
        request
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentRunAccepted {
    pub run_id: Uuid,
    pub session_id: Uuid,
    pub state: RunState,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentRunSnapshot {
    pub id: Uuid,
    pub session_id: Uuid,
    pub state: RunState,
    pub phase: RunPhase,
    pub reply: Option<String>,
    pub emotion: String,
    pub created_emotion: Option<String>,
    pub actions: Vec<Value>,
    pub children: Vec<Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct AgentLease {
    pub run_id: Uuid,
    pub user_id: Uuid,
    pub worker_id: String,
    pub fence_token: Uuid,
    pub attempt: i32,
    pub expires_at: DateTime<Utc>,
    pub request: AgentRunRequest,
    pub credential_ciphertext: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AgentRunOutput {
    pub reply: String,
    pub emotion: String,
    pub created_emotion: Option<String>,
    pub actions: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct AgentRunFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantChatResponse {
    pub reply: String,
    pub session_id: Uuid,
    pub emotion: String,
    pub created_emotion: Option<String>,
    pub actions: Vec<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl RunState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunPhase {
    Admitted,
    Planning,
    Delegating,
    Synthesizing,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl RunPhase {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Planning => "planning",
            Self::Delegating => "delegating",
            Self::Synthesizing => "synthesizing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentRunEvent {
    pub sequence: i64,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProfilePatch {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "avatarUrl")]
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Profile {
    pub id: Uuid,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "avatarUrl", skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OkResponse {
    pub ok: bool,
}
