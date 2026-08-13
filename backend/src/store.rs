use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx_core::{query::query, row::Row, transaction::Transaction};
use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{domain::request_fingerprint, protocol::*};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("resource not found")]
    NotFound,
    #[error("idempotency key was already used with a different request")]
    Conflict,
    #[error("database error")]
    Database(#[source] sqlx_core::Error),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
    #[error("run lease fence is stale")]
    StaleFence,
    #[error("run result arrived after cancellation or terminal transition")]
    LateResult,
    #[error("run was cancelled")]
    Cancelled,
}

#[async_trait]
pub trait BackendStore: Send + Sync {
    async fn get_profile(&self, user_id: Uuid) -> Result<Profile, StoreError>;
    async fn update_profile(
        &self,
        user_id: Uuid,
        patch: ProfilePatch,
    ) -> Result<Profile, StoreError>;

    async fn list_memories(&self, user_id: Uuid) -> Result<Vec<Memory>, StoreError>;
    async fn create_memory(&self, user_id: Uuid, input: MemoryCreate)
    -> Result<Memory, StoreError>;
    async fn search_memories(
        &self,
        user_id: Uuid,
        input: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchResult>, StoreError>;
    async fn delete_memory(&self, user_id: Uuid, memory_id: Uuid) -> Result<bool, StoreError>;

    async fn list_todos(&self, user_id: Uuid) -> Result<Vec<Todo>, StoreError>;
    async fn create_todo(&self, user_id: Uuid, input: TodoCreate) -> Result<Todo, StoreError>;
    async fn update_todo(
        &self,
        user_id: Uuid,
        todo_id: Uuid,
        input: TodoUpdate,
    ) -> Result<Todo, StoreError>;
    async fn delete_todo(&self, user_id: Uuid, todo_id: Uuid) -> Result<bool, StoreError>;

    async fn list_records(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_type: Option<&str>,
    ) -> Result<Vec<MiniAppRecord>, StoreError>;
    async fn create_record(
        &self,
        user_id: Uuid,
        mini_app_id: String,
        input: MiniAppRecordCreate,
    ) -> Result<MiniAppRecord, StoreError>;
    async fn update_record(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_id: Uuid,
        input: MiniAppRecordUpdate,
    ) -> Result<MiniAppRecord, StoreError>;
    async fn delete_record(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_id: Uuid,
    ) -> Result<bool, StoreError>;

    async fn create_agent_run(
        &self,
        user_id: Uuid,
        input: AgentRunRequest,
        idempotency_key: Option<String>,
        credential_ciphertext: Option<String>,
    ) -> Result<AgentRunSnapshot, StoreError>;
    async fn get_agent_run(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<AgentRunSnapshot>, StoreError>;
    async fn cancel_agent_run(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<AgentRunSnapshot>, StoreError>;
    async fn list_agent_events(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<Vec<AgentRunEvent>>, StoreError>;
    async fn claim_agent_run(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<Option<AgentLease>, StoreError>;
    async fn heartbeat_agent_run(
        &self,
        lease: &AgentLease,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<AgentLease, StoreError>;
    async fn complete_agent_run(
        &self,
        lease: &AgentLease,
        output: AgentRunOutput,
        now: DateTime<Utc>,
    ) -> Result<AgentRunSnapshot, StoreError>;
    async fn fail_agent_run(
        &self,
        lease: &AgentLease,
        failure: AgentRunFailure,
        now: DateTime<Utc>,
    ) -> Result<AgentRunSnapshot, StoreError>;
}

pub type StoreHandle = Arc<dyn BackendStore>;

#[derive(Default)]
struct MemoryState {
    profiles: HashMap<Uuid, Profile>,
    memories: HashMap<Uuid, Vec<Memory>>,
    todos: HashMap<Uuid, Vec<Todo>>,
    records: HashMap<(Uuid, String), Vec<MiniAppRecord>>,
    runs: HashMap<Uuid, AgentRunSnapshot>,
    run_owners: HashMap<Uuid, Uuid>,
    run_requests: HashMap<Uuid, AgentRunRequest>,
    run_credentials: HashMap<Uuid, Option<String>>,
    run_attempts: HashMap<Uuid, i32>,
    run_next_attempt: HashMap<Uuid, DateTime<Utc>>,
    leases: HashMap<Uuid, LeaseRecord>,
    events: HashMap<Uuid, Vec<AgentRunEvent>>,
    idempotency: HashMap<(Uuid, String), (String, Uuid)>,
}

#[derive(Clone, Debug)]
struct LeaseRecord {
    worker_id: String,
    fence_token: Uuid,
    attempt: i32,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct InMemoryStore {
    state: Mutex<MemoryState>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn new_profile(user_id: Uuid) -> Profile {
        let now = Utc::now();
        Profile {
            id: user_id,
            display_name: String::new(),
            avatar_url: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn new_run(input: &AgentRunRequest) -> AgentRunSnapshot {
        let now = Utc::now();
        AgentRunSnapshot {
            id: Uuid::now_v7(),
            session_id: input.session_id(),
            state: RunState::Queued,
            phase: RunPhase::Admitted,
            reply: None,
            emotion: "neutral".to_owned(),
            created_emotion: None,
            actions: Vec::new(),
            children: Vec::new(),
            error: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn append_event(state: &mut MemoryState, run_id: Uuid, event_type: &str, payload: Value) {
        let events = state.events.entry(run_id).or_default();
        events.push(AgentRunEvent {
            sequence: events.len() as i64,
            event_type: event_type.to_owned(),
            payload,
            created_at: Utc::now(),
        });
    }

    fn ensure_lease(
        state: &MemoryState,
        lease: &AgentLease,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let Some(record) = state.leases.get(&lease.run_id) else {
            return Err(StoreError::StaleFence);
        };
        if record.worker_id != lease.worker_id
            || record.fence_token != lease.fence_token
            || record.attempt != lease.attempt
            || record.expires_at <= now
        {
            return Err(StoreError::StaleFence);
        }
        Ok(())
    }
}

#[async_trait]
impl BackendStore for InMemoryStore {
    async fn get_profile(&self, user_id: Uuid) -> Result<Profile, StoreError> {
        let mut state = self.state.lock().await;
        Ok(state
            .profiles
            .entry(user_id)
            .or_insert_with(|| Self::new_profile(user_id))
            .clone())
    }

    async fn update_profile(
        &self,
        user_id: Uuid,
        patch: ProfilePatch,
    ) -> Result<Profile, StoreError> {
        let mut state = self.state.lock().await;
        let profile = state
            .profiles
            .entry(user_id)
            .or_insert_with(|| Self::new_profile(user_id));
        if let Some(display_name) = patch.display_name {
            profile.display_name = display_name;
        }
        if let Some(avatar_url) = patch.avatar_url {
            profile.avatar_url = Some(avatar_url);
        }
        profile.updated_at = Utc::now();
        Ok(profile.clone())
    }

    async fn list_memories(&self, user_id: Uuid) -> Result<Vec<Memory>, StoreError> {
        let state = self.state.lock().await;
        let mut items = state.memories.get(&user_id).cloned().unwrap_or_default();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
        Ok(items)
    }

    async fn create_memory(
        &self,
        user_id: Uuid,
        input: MemoryCreate,
    ) -> Result<Memory, StoreError> {
        let memory = Memory {
            id: Uuid::now_v7(),
            title: input.title.trim().to_owned(),
            content: input.content.trim().to_owned(),
            created_at: Utc::now(),
        };
        self.state
            .lock()
            .await
            .memories
            .entry(user_id)
            .or_default()
            .push(memory.clone());
        Ok(memory)
    }

    async fn search_memories(
        &self,
        user_id: Uuid,
        input: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchResult>, StoreError> {
        let query = input.query.trim().to_lowercase();
        let state = self.state.lock().await;
        let mut matches: Vec<_> = state
            .memories
            .get(&user_id)
            .into_iter()
            .flatten()
            .filter_map(|memory| {
                let haystack = format!("{} {}", memory.title, memory.content).to_lowercase();
                haystack.contains(&query).then(|| MemorySearchResult {
                    memory_id: memory.id,
                    title: memory.title.clone(),
                    chunk_text: memory.content.clone(),
                    score: if memory.title.to_lowercase().contains(&query) {
                        1.0
                    } else {
                        0.75
                    },
                    source_type: "memory",
                })
            })
            .collect();
        matches.truncate(input.limit);
        Ok(matches)
    }

    async fn delete_memory(&self, user_id: Uuid, memory_id: Uuid) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(items) = state.memories.get_mut(&user_id) else {
            return Ok(false);
        };
        let before = items.len();
        items.retain(|item| item.id != memory_id);
        Ok(items.len() != before)
    }

    async fn list_todos(&self, user_id: Uuid) -> Result<Vec<Todo>, StoreError> {
        let state = self.state.lock().await;
        let mut items = state.todos.get(&user_id).cloned().unwrap_or_default();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
        Ok(items)
    }

    async fn create_todo(&self, user_id: Uuid, input: TodoCreate) -> Result<Todo, StoreError> {
        let todo = Todo {
            id: Uuid::now_v7(),
            title: input.title.trim().to_owned(),
            done: false,
            created_at: Utc::now(),
        };
        self.state
            .lock()
            .await
            .todos
            .entry(user_id)
            .or_default()
            .push(todo.clone());
        Ok(todo)
    }

    async fn update_todo(
        &self,
        user_id: Uuid,
        todo_id: Uuid,
        input: TodoUpdate,
    ) -> Result<Todo, StoreError> {
        let mut state = self.state.lock().await;
        let item = state
            .todos
            .get_mut(&user_id)
            .and_then(|items| items.iter_mut().find(|item| item.id == todo_id))
            .ok_or(StoreError::NotFound)?;
        if let Some(title) = input.title {
            item.title = title.trim().to_owned();
        }
        if let Some(done) = input.done {
            item.done = done;
        }
        Ok(item.clone())
    }

    async fn delete_todo(&self, user_id: Uuid, todo_id: Uuid) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(items) = state.todos.get_mut(&user_id) else {
            return Ok(false);
        };
        let before = items.len();
        items.retain(|item| item.id != todo_id);
        Ok(items.len() != before)
    }

    async fn list_records(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_type: Option<&str>,
    ) -> Result<Vec<MiniAppRecord>, StoreError> {
        let state = self.state.lock().await;
        let mut items = state
            .records
            .get(&(user_id, mini_app_id.to_owned()))
            .cloned()
            .unwrap_or_default();
        if let Some(record_type) = record_type {
            items.retain(|item| item.record_type == record_type);
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
        Ok(items)
    }

    async fn create_record(
        &self,
        user_id: Uuid,
        mini_app_id: String,
        input: MiniAppRecordCreate,
    ) -> Result<MiniAppRecord, StoreError> {
        let now = Utc::now();
        let record = MiniAppRecord {
            id: Uuid::now_v7(),
            mini_app_id: mini_app_id.clone(),
            record_type: input.record_type.trim().to_owned(),
            values: input.values,
            created_at: now,
            updated_at: now,
        };
        self.state
            .lock()
            .await
            .records
            .entry((user_id, mini_app_id))
            .or_default()
            .push(record.clone());
        Ok(record)
    }

    async fn update_record(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_id: Uuid,
        input: MiniAppRecordUpdate,
    ) -> Result<MiniAppRecord, StoreError> {
        let mut state = self.state.lock().await;
        let record = state
            .records
            .get_mut(&(user_id, mini_app_id.to_owned()))
            .and_then(|items| items.iter_mut().find(|item| item.id == record_id))
            .ok_or(StoreError::NotFound)?;
        record.values = input.values;
        record.updated_at = Utc::now();
        Ok(record.clone())
    }

    async fn delete_record(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_id: Uuid,
    ) -> Result<bool, StoreError> {
        let mut state = self.state.lock().await;
        let Some(items) = state.records.get_mut(&(user_id, mini_app_id.to_owned())) else {
            return Ok(false);
        };
        let before = items.len();
        items.retain(|item| item.id != record_id);
        Ok(items.len() != before)
    }

    async fn create_agent_run(
        &self,
        user_id: Uuid,
        input: AgentRunRequest,
        idempotency_key: Option<String>,
        credential_ciphertext: Option<String>,
    ) -> Result<AgentRunSnapshot, StoreError> {
        let fingerprint = request_fingerprint(&input);
        let stored_input = input.without_secret();
        let mut state = self.state.lock().await;
        if let Some(key) = idempotency_key.as_deref()
            && let Some((existing_fingerprint, run_id)) =
                state.idempotency.get(&(user_id, key.to_owned()))
        {
            if existing_fingerprint != &fingerprint {
                return Err(StoreError::Conflict);
            }
            return state.runs.get(run_id).cloned().ok_or(StoreError::NotFound);
        }
        let run = Self::new_run(&stored_input);
        if let Some(key) = idempotency_key {
            state
                .idempotency
                .insert((user_id, key), (fingerprint, run.id));
        }
        Self::append_event(&mut state, run.id, "run.queued", json!({"state": "queued"}));
        state.run_owners.insert(run.id, user_id);
        state.run_requests.insert(run.id, stored_input);
        state.run_credentials.insert(run.id, credential_ciphertext);
        state.run_attempts.insert(run.id, 0);
        state.run_next_attempt.insert(run.id, run.created_at);
        state.runs.insert(run.id, run.clone());
        Ok(run)
    }

    async fn get_agent_run(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<AgentRunSnapshot>, StoreError> {
        let state = self.state.lock().await;
        let Some(run) = state.runs.get(&run_id) else {
            return Ok(None);
        };
        if state.run_owners.get(&run_id) == Some(&user_id) {
            Ok(Some(run.clone()))
        } else {
            Ok(None)
        }
    }

    async fn cancel_agent_run(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<AgentRunSnapshot>, StoreError> {
        let mut state = self.state.lock().await;
        if !state.runs.contains_key(&run_id) {
            return Ok(None);
        }
        if state.run_owners.get(&run_id) != Some(&user_id) {
            return Ok(None);
        }
        let should_cancel = state.runs.get(&run_id).is_some_and(|run| {
            !matches!(
                run.state,
                RunState::Completed
                    | RunState::Failed
                    | RunState::Interrupted
                    | RunState::Cancelled
            )
        });
        if should_cancel {
            let run = state.runs.get_mut(&run_id).expect("run was checked above");
            run.state = RunState::Cancelled;
            run.phase = RunPhase::Cancelled;
            run.updated_at = Utc::now();
            state.leases.remove(&run_id);
            state.run_credentials.remove(&run_id);
        }
        if should_cancel {
            Self::append_event(
                &mut state,
                run_id,
                "run.cancelled",
                json!({"state": "cancelled"}),
            );
        }
        Ok(state.runs.get(&run_id).cloned())
    }

    async fn list_agent_events(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<Vec<AgentRunEvent>>, StoreError> {
        let state = self.state.lock().await;
        if !state.runs.contains_key(&run_id) {
            return Ok(None);
        }
        if state.run_owners.get(&run_id) != Some(&user_id) {
            return Ok(None);
        }
        Ok(Some(state.events.get(&run_id).cloned().unwrap_or_default()))
    }

    async fn claim_agent_run(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<Option<AgentLease>, StoreError> {
        let chrono_lease = chrono::Duration::from_std(lease_duration)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let mut state = self.state.lock().await;
        let candidate = state
            .runs
            .values()
            .filter(|run| {
                (matches!(run.state, RunState::Queued)
                    && state
                        .run_next_attempt
                        .get(&run.id)
                        .is_none_or(|available| *available <= now))
                    || matches!(run.state, RunState::Running)
                        && state
                            .leases
                            .get(&run.id)
                            .is_some_and(|lease| lease.expires_at <= now)
            })
            .min_by_key(|run| run.created_at)
            .map(|run| run.id);
        let Some(run_id) = candidate else {
            return Ok(None);
        };
        let was_expired = state
            .runs
            .get(&run_id)
            .is_some_and(|run| matches!(run.state, RunState::Running));
        if was_expired {
            Self::append_event(
                &mut state,
                run_id,
                "run.lease_expired",
                json!({"state": "queued"}),
            );
        }
        let attempt = {
            let attempt = state.run_attempts.entry(run_id).or_insert(0);
            *attempt += 1;
            *attempt
        };
        let expires_at = now + chrono_lease;
        let fence_token = Uuid::new_v4();
        state.leases.insert(
            run_id,
            LeaseRecord {
                worker_id: worker_id.to_owned(),
                fence_token,
                attempt,
                expires_at,
            },
        );
        let run = state.runs.get_mut(&run_id).ok_or(StoreError::NotFound)?;
        run.state = RunState::Running;
        run.phase = RunPhase::Planning;
        run.error = None;
        run.updated_at = now;
        Self::append_event(
            &mut state,
            run_id,
            "run.claimed",
            json!({"worker_id": worker_id, "attempt": attempt}),
        );
        let user_id = *state.run_owners.get(&run_id).ok_or(StoreError::NotFound)?;
        let request = state
            .run_requests
            .get(&run_id)
            .cloned()
            .ok_or(StoreError::NotFound)?;
        Ok(Some(AgentLease {
            run_id,
            user_id,
            worker_id: worker_id.to_owned(),
            fence_token,
            attempt,
            expires_at,
            request,
            credential_ciphertext: state.run_credentials.get(&run_id).cloned().flatten(),
        }))
    }

    async fn heartbeat_agent_run(
        &self,
        lease: &AgentLease,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<AgentLease, StoreError> {
        let chrono_lease = chrono::Duration::from_std(lease_duration)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let mut state = self.state.lock().await;
        let Some(run) = state.runs.get(&lease.run_id) else {
            return Err(StoreError::LateResult);
        };
        if matches!(run.state, RunState::Cancelled) {
            return Err(StoreError::Cancelled);
        }
        Self::ensure_lease(&state, lease, now)?;
        let expires_at = now + chrono_lease;
        let record = state
            .leases
            .get_mut(&lease.run_id)
            .ok_or(StoreError::StaleFence)?;
        record.expires_at = expires_at;
        Self::append_event(
            &mut state,
            lease.run_id,
            "run.heartbeat",
            json!({"worker_id": lease.worker_id, "attempt": lease.attempt}),
        );
        let mut refreshed = lease.clone();
        refreshed.expires_at = expires_at;
        Ok(refreshed)
    }

    async fn complete_agent_run(
        &self,
        lease: &AgentLease,
        output: AgentRunOutput,
        now: DateTime<Utc>,
    ) -> Result<AgentRunSnapshot, StoreError> {
        let mut state = self.state.lock().await;
        let Some(run) = state.runs.get(&lease.run_id) else {
            return Err(StoreError::LateResult);
        };
        if matches!(
            run.state,
            RunState::Cancelled | RunState::Completed | RunState::Failed | RunState::Interrupted
        ) {
            return Err(StoreError::LateResult);
        }
        Self::ensure_lease(&state, lease, now)?;
        state.leases.remove(&lease.run_id);
        state.run_credentials.remove(&lease.run_id);
        let run = state
            .runs
            .get_mut(&lease.run_id)
            .ok_or(StoreError::LateResult)?;
        run.state = RunState::Completed;
        run.phase = RunPhase::Completed;
        run.reply = Some(output.reply);
        run.emotion = output.emotion;
        run.created_emotion = output.created_emotion;
        run.actions = output.actions;
        run.error = None;
        run.updated_at = now;
        Self::append_event(
            &mut state,
            lease.run_id,
            "run.completed",
            json!({"state": "completed", "attempt": lease.attempt}),
        );
        Ok(state
            .runs
            .get(&lease.run_id)
            .cloned()
            .ok_or(StoreError::LateResult)?)
    }

    async fn fail_agent_run(
        &self,
        lease: &AgentLease,
        failure: AgentRunFailure,
        now: DateTime<Utc>,
    ) -> Result<AgentRunSnapshot, StoreError> {
        let mut state = self.state.lock().await;
        let Some(run) = state.runs.get(&lease.run_id) else {
            return Err(StoreError::LateResult);
        };
        if matches!(
            run.state,
            RunState::Cancelled | RunState::Completed | RunState::Failed | RunState::Interrupted
        ) {
            return Err(StoreError::LateResult);
        }
        Self::ensure_lease(&state, lease, now)?;
        state.leases.remove(&lease.run_id);
        let retry = failure.retryable && lease.attempt < 3;
        let safe_message = failure.message.chars().take(500).collect::<String>();
        let run = state
            .runs
            .get_mut(&lease.run_id)
            .ok_or(StoreError::LateResult)?;
        run.updated_at = now;
        run.error = Some(safe_message.clone());
        if retry {
            run.state = RunState::Queued;
            run.phase = RunPhase::Admitted;
            state.run_next_attempt.insert(
                lease.run_id,
                now + chrono::Duration::seconds(
                    2_i64.pow((lease.attempt.saturating_sub(1)) as u32).min(30),
                ),
            );
            Self::append_event(
                &mut state,
                lease.run_id,
                "run.retry_scheduled",
                json!({"code": failure.code, "attempt": lease.attempt, "error": safe_message}),
            );
        } else {
            run.state = RunState::Failed;
            run.phase = RunPhase::Failed;
            state.run_credentials.remove(&lease.run_id);
            Self::append_event(
                &mut state,
                lease.run_id,
                "run.failed",
                json!({"code": failure.code, "attempt": lease.attempt, "error": safe_message}),
            );
        }
        Ok(state
            .runs
            .get(&lease.run_id)
            .cloned()
            .ok_or(StoreError::LateResult)?)
    }
}

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .map_err(StoreError::Database)?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl BackendStore for PostgresStore {
    async fn get_profile(&self, user_id: Uuid) -> Result<Profile, StoreError> {
        query::<Postgres>(
            "INSERT INTO public.profiles (id) VALUES ($1) ON CONFLICT (id) DO NOTHING",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        let row = query::<Postgres>("SELECT id, display_name, avatar_url, created_at, updated_at FROM public.profiles WHERE id = $1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        profile_from_row(&row)
    }

    async fn update_profile(
        &self,
        user_id: Uuid,
        patch: ProfilePatch,
    ) -> Result<Profile, StoreError> {
        self.get_profile(user_id).await?;
        query::<Postgres>("UPDATE public.profiles SET display_name = COALESCE($2, display_name), avatar_url = COALESCE($3, avatar_url), updated_at = now() WHERE id = $1")
            .bind(user_id)
            .bind(patch.display_name)
            .bind(patch.avatar_url)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        self.get_profile(user_id).await
    }

    async fn list_memories(&self, user_id: Uuid) -> Result<Vec<Memory>, StoreError> {
        let rows = query::<Postgres>("SELECT id, title, content, created_at FROM public.memories WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1000")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(memory_from_row).collect()
    }

    async fn create_memory(
        &self,
        user_id: Uuid,
        input: MemoryCreate,
    ) -> Result<Memory, StoreError> {
        let id = Uuid::now_v7();
        let row = query::<Postgres>("INSERT INTO public.memories (id, user_id, title, content) VALUES ($1, $2, $3, $4) RETURNING id, title, content, created_at")
            .bind(id)
            .bind(user_id)
            .bind(input.title.trim())
            .bind(input.content.trim())
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        memory_from_row(&row)
    }

    async fn search_memories(
        &self,
        user_id: Uuid,
        input: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchResult>, StoreError> {
        let needle = format!("%{}%", input.query.trim());
        let rows = query::<Postgres>("SELECT id, title, content FROM public.memories WHERE user_id = $1 AND (title ILIKE $2 OR content ILIKE $2) ORDER BY created_at DESC LIMIT $3")
            .bind(user_id)
            .bind(needle)
            .bind(input.limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter()
            .map(|row| {
                Ok(MemorySearchResult {
                    memory_id: row.try_get("id").map_err(decode_error)?,
                    title: row.try_get("title").map_err(decode_error)?,
                    chunk_text: row.try_get("content").map_err(decode_error)?,
                    score: 1.0,
                    source_type: "memory",
                })
            })
            .collect()
    }

    async fn delete_memory(&self, user_id: Uuid, memory_id: Uuid) -> Result<bool, StoreError> {
        let result =
            query::<Postgres>("DELETE FROM public.memories WHERE id = $1 AND user_id = $2")
                .bind(memory_id)
                .bind(user_id)
                .execute(&self.pool)
                .await
                .map_err(StoreError::Database)?;
        Ok(result.rows_affected() == 1)
    }

    async fn list_todos(&self, user_id: Uuid) -> Result<Vec<Todo>, StoreError> {
        let rows = query::<Postgres>("SELECT id, title, done, created_at FROM public.todos WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1000")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(todo_from_row).collect()
    }

    async fn create_todo(&self, user_id: Uuid, input: TodoCreate) -> Result<Todo, StoreError> {
        let row = query::<Postgres>("INSERT INTO public.todos (id, user_id, title) VALUES ($1, $2, $3) RETURNING id, title, done, created_at")
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(input.title.trim())
            .fetch_one(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        todo_from_row(&row)
    }

    async fn update_todo(
        &self,
        user_id: Uuid,
        todo_id: Uuid,
        input: TodoUpdate,
    ) -> Result<Todo, StoreError> {
        let row = query::<Postgres>("UPDATE public.todos SET title = COALESCE($3, title), done = COALESCE($4, done), updated_at = now() WHERE id = $1 AND user_id = $2 RETURNING id, title, done, created_at")
            .bind(todo_id)
            .bind(user_id)
            .bind(input.title)
            .bind(input.done)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        todo_from_row(&row)
    }

    async fn delete_todo(&self, user_id: Uuid, todo_id: Uuid) -> Result<bool, StoreError> {
        let result = query::<Postgres>("DELETE FROM public.todos WHERE id = $1 AND user_id = $2")
            .bind(todo_id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        Ok(result.rows_affected() == 1)
    }

    async fn list_records(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_type: Option<&str>,
    ) -> Result<Vec<MiniAppRecord>, StoreError> {
        let rows = if let Some(record_type) = record_type {
            query::<Postgres>("SELECT id, mini_app_id, record_type, \"values\", created_at, updated_at FROM public.mini_app_records WHERE user_id = $1 AND mini_app_id = $2 AND record_type = $3 ORDER BY created_at DESC LIMIT 500")
                .bind(user_id).bind(mini_app_id).bind(record_type).fetch_all(&self.pool).await
        } else {
            query::<Postgres>("SELECT id, mini_app_id, record_type, \"values\", created_at, updated_at FROM public.mini_app_records WHERE user_id = $1 AND mini_app_id = $2 ORDER BY created_at DESC LIMIT 500")
                .bind(user_id).bind(mini_app_id).fetch_all(&self.pool).await
        }.map_err(StoreError::Database)?;
        rows.iter().map(record_from_row).collect()
    }

    async fn create_record(
        &self,
        user_id: Uuid,
        mini_app_id: String,
        input: MiniAppRecordCreate,
    ) -> Result<MiniAppRecord, StoreError> {
        let row = query::<Postgres>("INSERT INTO public.mini_app_records (id, user_id, mini_app_id, record_type, \"values\") VALUES ($1, $2, $3, $4, $5) RETURNING id, mini_app_id, record_type, \"values\", created_at, updated_at")
            .bind(Uuid::now_v7()).bind(user_id).bind(mini_app_id).bind(input.record_type.trim()).bind(Value::Object(input.values.into_iter().collect()))
            .fetch_one(&self.pool).await.map_err(StoreError::Database)?;
        record_from_row(&row)
    }

    async fn update_record(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_id: Uuid,
        input: MiniAppRecordUpdate,
    ) -> Result<MiniAppRecord, StoreError> {
        let row = query::<Postgres>("UPDATE public.mini_app_records SET \"values\" = $4, updated_at = now() WHERE id = $1 AND user_id = $2 AND mini_app_id = $3 RETURNING id, mini_app_id, record_type, \"values\", created_at, updated_at")
            .bind(record_id).bind(user_id).bind(mini_app_id).bind(Value::Object(input.values.into_iter().collect()))
            .fetch_optional(&self.pool).await.map_err(StoreError::Database)?.ok_or(StoreError::NotFound)?;
        record_from_row(&row)
    }

    async fn delete_record(
        &self,
        user_id: Uuid,
        mini_app_id: &str,
        record_id: Uuid,
    ) -> Result<bool, StoreError> {
        let result = query::<Postgres>("DELETE FROM public.mini_app_records WHERE id = $1 AND user_id = $2 AND mini_app_id = $3")
            .bind(record_id).bind(user_id).bind(mini_app_id).execute(&self.pool).await.map_err(StoreError::Database)?;
        Ok(result.rows_affected() == 1)
    }

    async fn create_agent_run(
        &self,
        user_id: Uuid,
        input: AgentRunRequest,
        idempotency_key: Option<String>,
        credential_ciphertext: Option<String>,
    ) -> Result<AgentRunSnapshot, StoreError> {
        let fingerprint = request_fingerprint(&input);
        if let Some(key) = idempotency_key.as_deref()
            && let Some(row) = query::<Postgres>("SELECT run_id, request_hash FROM public.idempotency_keys WHERE user_id = $1 AND key = $2")
                .bind(user_id).bind(key).fetch_optional(&self.pool).await.map_err(StoreError::Database)?
        {
            let existing_hash: String = row.try_get("request_hash").map_err(decode_error)?;
            if existing_hash != fingerprint { return Err(StoreError::Conflict); }
            let run_id: Uuid = row.try_get("run_id").map_err(decode_error)?;
            return self.get_agent_run(user_id, run_id).await?.ok_or(StoreError::NotFound);
        }
        let run = InMemoryStore::new_run(&input);
        let stored_request = serde_json::to_value(input.without_secret())
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        query::<Postgres>("INSERT INTO public.agent_runs (id, user_id, session_id, state, phase, request_payload, actions, children, emotion, attempt, max_attempts, next_attempt_at, credential_ciphertext, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 0, 3, $10, $11, $10, $10)")
            .bind(run.id).bind(user_id).bind(run.session_id).bind(run.state.as_str()).bind(run.phase.as_str()).bind(stored_request).bind(json!([])).bind(json!([])).bind(&run.emotion).bind(run.created_at).bind(credential_ciphertext)
            .execute(&mut *tx).await.map_err(StoreError::Database)?;
        if let Some(key) = idempotency_key {
            query::<Postgres>("INSERT INTO public.idempotency_keys (user_id, key, request_hash, run_id) VALUES ($1, $2, $3, $4)")
                .bind(user_id).bind(key).bind(fingerprint).bind(run.id).execute(&mut *tx).await.map_err(StoreError::Database)?;
        }
        insert_event(&mut tx, run.id, 0, "run.queued", json!({"state": "queued"})).await?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(run)
    }

    async fn get_agent_run(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<AgentRunSnapshot>, StoreError> {
        let row = query::<Postgres>("SELECT id, session_id, state, phase, reply, emotion, created_emotion, actions, children, error, created_at, updated_at FROM public.agent_runs WHERE id = $1 AND user_id = $2")
            .bind(run_id).bind(user_id).fetch_optional(&self.pool).await.map_err(StoreError::Database)?;
        row.as_ref().map(snapshot_from_row).transpose()
    }

    async fn cancel_agent_run(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<AgentRunSnapshot>, StoreError> {
        let Some(current) = self.get_agent_run(user_id, run_id).await? else {
            return Ok(None);
        };
        if matches!(
            current.state,
            RunState::Completed | RunState::Failed | RunState::Interrupted | RunState::Cancelled
        ) {
            return Ok(Some(current));
        }
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = query::<Postgres>("UPDATE public.agent_runs SET state = 'cancelled', phase = 'cancelled', updated_at = now() WHERE id = $1 AND user_id = $2 AND state IN ('queued', 'running') RETURNING id")
            .bind(run_id).bind(user_id).fetch_optional(&mut *tx).await.map_err(StoreError::Database)?;
        if row.is_some() {
            let next_sequence: i64 = query::<Postgres>("SELECT COALESCE(MAX(sequence), -1) + 1 AS next_sequence FROM public.agent_run_events WHERE run_id = $1")
                .bind(run_id).fetch_one(&mut *tx).await.map_err(StoreError::Database)?.try_get("next_sequence").map_err(decode_error)?;
            insert_event(
                &mut tx,
                run_id,
                next_sequence,
                "run.cancelled",
                json!({"state": "cancelled"}),
            )
            .await?;
        }
        tx.commit().await.map_err(StoreError::Database)?;
        self.get_agent_run(user_id, run_id).await
    }

    async fn list_agent_events(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<Vec<AgentRunEvent>>, StoreError> {
        if self.get_agent_run(user_id, run_id).await?.is_none() {
            return Ok(None);
        }
        let rows = query::<Postgres>("SELECT sequence, event_type, payload, created_at FROM public.agent_run_events WHERE run_id = $1 ORDER BY sequence ASC")
            .bind(run_id).fetch_all(&self.pool).await.map_err(StoreError::Database)?;
        rows.iter()
            .map(event_from_row)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    async fn claim_agent_run(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<Option<AgentLease>, StoreError> {
        let lease = chrono::Duration::from_std(lease_duration)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let expires_at = now + lease;
        let fence_token = Uuid::new_v4();
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let candidate = query::<Postgres>("SELECT id, state FROM public.agent_runs WHERE (state = 'queued' AND next_attempt_at <= $1) OR (state = 'running' AND lease_expires_at <= $1) ORDER BY created_at ASC FOR UPDATE SKIP LOCKED LIMIT 1")
            .bind(now)
            .fetch_optional(&mut *tx)
            .await
            .map_err(StoreError::Database)?;
        let Some(candidate) = candidate else {
            tx.commit().await.map_err(StoreError::Database)?;
            return Ok(None);
        };
        let run_id: Uuid = candidate.try_get("id").map_err(decode_error)?;
        let previous_state: String = candidate.try_get("state").map_err(decode_error)?;
        let row = query::<Postgres>("UPDATE public.agent_runs SET state = 'running', phase = 'planning', attempt = attempt + 1, lease_owner = $2, lease_token = $3, lease_expires_at = $4, error = NULL, updated_at = $1 WHERE id = $5 RETURNING id, user_id, attempt, request_payload, credential_ciphertext")
            .bind(now)
            .bind(worker_id)
            .bind(fence_token)
            .bind(expires_at)
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(StoreError::Database)?;
        let next_sequence = next_event_sequence(&mut tx, run_id).await?;
        let mut sequence = next_sequence;
        if previous_state == "running" {
            insert_event(
                &mut tx,
                run_id,
                sequence,
                "run.lease_expired",
                json!({"state": "queued"}),
            )
            .await?;
            sequence += 1;
        }
        insert_event(&mut tx, run_id, sequence, "run.claimed", json!({"worker_id": worker_id, "attempt": row.try_get::<i32, _>("attempt").map_err(decode_error)?})).await?;
        let lease = lease_from_row(&row, worker_id, fence_token, expires_at)?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(Some(lease))
    }

    async fn heartbeat_agent_run(
        &self,
        lease: &AgentLease,
        now: DateTime<Utc>,
        lease_duration: Duration,
    ) -> Result<AgentLease, StoreError> {
        let duration = chrono::Duration::from_std(lease_duration)
            .map_err(|error| StoreError::InvalidData(error.to_string()))?;
        let expires_at = now + duration;
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let updated = query::<Postgres>("UPDATE public.agent_runs SET lease_expires_at = $1, updated_at = $2 WHERE id = $3 AND user_id = $4 AND state = 'running' AND lease_owner = $5 AND lease_token = $6 AND lease_expires_at > $2 RETURNING id")
            .bind(expires_at).bind(now).bind(lease.run_id).bind(lease.user_id).bind(&lease.worker_id).bind(lease.fence_token)
            .fetch_optional(&mut *tx).await.map_err(StoreError::Database)?;
        if updated.is_none() {
            tx.rollback().await.map_err(StoreError::Database)?;
            return Err(self.classify_lease_loss(lease.run_id).await?);
        }
        let sequence = next_event_sequence(&mut tx, lease.run_id).await?;
        insert_event(
            &mut tx,
            lease.run_id,
            sequence,
            "run.heartbeat",
            json!({"worker_id": lease.worker_id, "attempt": lease.attempt}),
        )
        .await?;
        tx.commit().await.map_err(StoreError::Database)?;
        let mut refreshed = lease.clone();
        refreshed.expires_at = expires_at;
        Ok(refreshed)
    }

    async fn complete_agent_run(
        &self,
        lease: &AgentLease,
        output: AgentRunOutput,
        now: DateTime<Utc>,
    ) -> Result<AgentRunSnapshot, StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = query::<Postgres>("UPDATE public.agent_runs SET state = 'completed', phase = 'completed', reply = $1, emotion = $2, created_emotion = $3, actions = $4, error = NULL, lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, credential_ciphertext = NULL, updated_at = $5 WHERE id = $6 AND user_id = $7 AND state = 'running' AND lease_owner = $8 AND lease_token = $9 AND lease_expires_at > $5 RETURNING id, session_id, state, phase, reply, emotion, created_emotion, actions, children, error, created_at, updated_at")
            .bind(output.reply).bind(output.emotion).bind(output.created_emotion).bind(json!(output.actions)).bind(now).bind(lease.run_id).bind(lease.user_id).bind(&lease.worker_id).bind(lease.fence_token)
            .fetch_optional(&mut *tx).await.map_err(StoreError::Database)?;
        let Some(row) = row else {
            tx.rollback().await.map_err(StoreError::Database)?;
            return Err(self.classify_lease_loss(lease.run_id).await?);
        };
        let sequence = next_event_sequence(&mut tx, lease.run_id).await?;
        insert_event(
            &mut tx,
            lease.run_id,
            sequence,
            "run.completed",
            json!({"state": "completed", "attempt": lease.attempt}),
        )
        .await?;
        let snapshot = snapshot_from_row(&row)?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(snapshot)
    }

    async fn fail_agent_run(
        &self,
        lease: &AgentLease,
        failure: AgentRunFailure,
        now: DateTime<Utc>,
    ) -> Result<AgentRunSnapshot, StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let retry = failure.retryable && lease.attempt < 3;
        let retry_at = now
            + chrono::Duration::seconds(
                2_i64.pow((lease.attempt.saturating_sub(1)) as u32).min(30),
            );
        let (state, phase, clear_credential) = if retry {
            ("queued", "admitted", false)
        } else {
            ("failed", "failed", true)
        };
        let row = query::<Postgres>("UPDATE public.agent_runs SET state = $1, phase = $2, error = $3, next_attempt_at = $4, lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, credential_ciphertext = CASE WHEN $5 THEN NULL ELSE credential_ciphertext END, updated_at = $6 WHERE id = $7 AND user_id = $8 AND state = 'running' AND lease_owner = $9 AND lease_token = $10 AND lease_expires_at > $6 RETURNING id, session_id, state, phase, reply, emotion, created_emotion, actions, children, error, created_at, updated_at")
            .bind(state).bind(phase).bind(failure.message.chars().take(500).collect::<String>()).bind(retry_at).bind(clear_credential).bind(now).bind(lease.run_id).bind(lease.user_id).bind(&lease.worker_id).bind(lease.fence_token)
            .fetch_optional(&mut *tx).await.map_err(StoreError::Database)?;
        let Some(row) = row else {
            tx.rollback().await.map_err(StoreError::Database)?;
            return Err(self.classify_lease_loss(lease.run_id).await?);
        };
        let sequence = next_event_sequence(&mut tx, lease.run_id).await?;
        let event = if retry {
            "run.retry_scheduled"
        } else {
            "run.failed"
        };
        insert_event(&mut tx, lease.run_id, sequence, event, json!({"code": failure.code, "attempt": lease.attempt, "error": failure.message.chars().take(500).collect::<String>()})).await?;
        let snapshot = snapshot_from_row(&row)?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(snapshot)
    }
}

impl PostgresStore {
    async fn classify_lease_loss(&self, run_id: Uuid) -> Result<StoreError, StoreError> {
        let state = query::<Postgres>("SELECT state FROM public.agent_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .map(|row| row.try_get::<String, _>("state").map_err(decode_error))
            .transpose()?;
        Ok(match state.as_deref() {
            Some("cancelled" | "completed" | "failed" | "interrupted") => StoreError::LateResult,
            None => StoreError::LateResult,
            Some(_) => StoreError::StaleFence,
        })
    }
}

async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    sequence: i64,
    event_type: &str,
    payload: Value,
) -> Result<(), StoreError> {
    query::<Postgres>("INSERT INTO public.agent_run_events (run_id, sequence, event_type, payload) VALUES ($1, $2, $3, $4)")
        .bind(run_id).bind(sequence).bind(event_type).bind(payload).execute(&mut **tx).await.map_err(StoreError::Database)?;
    Ok(())
}

async fn next_event_sequence(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<i64, StoreError> {
    query::<Postgres>("SELECT COALESCE(MAX(sequence), -1) + 1 AS next_sequence FROM public.agent_run_events WHERE run_id = $1")
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(StoreError::Database)?
        .try_get("next_sequence")
        .map_err(decode_error)
}

fn lease_from_row(
    row: &PgRow,
    worker_id: &str,
    fence_token: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<AgentLease, StoreError> {
    let request_payload: Value = row.try_get("request_payload").map_err(decode_error)?;
    let request: AgentRunRequest = serde_json::from_value(request_payload)
        .map_err(|error| StoreError::InvalidData(format!("invalid run request: {error}")))?;
    Ok(AgentLease {
        run_id: row.try_get("id").map_err(decode_error)?,
        user_id: row.try_get("user_id").map_err(decode_error)?,
        worker_id: worker_id.to_owned(),
        fence_token,
        attempt: row.try_get("attempt").map_err(decode_error)?,
        expires_at,
        request,
        credential_ciphertext: row.try_get("credential_ciphertext").map_err(decode_error)?,
    })
}

fn profile_from_row(row: &PgRow) -> Result<Profile, StoreError> {
    Ok(Profile {
        id: row.try_get("id").map_err(decode_error)?,
        display_name: row.try_get("display_name").map_err(decode_error)?,
        avatar_url: row.try_get("avatar_url").map_err(decode_error)?,
        created_at: row.try_get("created_at").map_err(decode_error)?,
        updated_at: row.try_get("updated_at").map_err(decode_error)?,
    })
}

fn memory_from_row(row: &PgRow) -> Result<Memory, StoreError> {
    Ok(Memory {
        id: row.try_get("id").map_err(decode_error)?,
        title: row.try_get("title").map_err(decode_error)?,
        content: row.try_get("content").map_err(decode_error)?,
        created_at: row.try_get("created_at").map_err(decode_error)?,
    })
}

fn todo_from_row(row: &PgRow) -> Result<Todo, StoreError> {
    Ok(Todo {
        id: row.try_get("id").map_err(decode_error)?,
        title: row.try_get("title").map_err(decode_error)?,
        done: row.try_get("done").map_err(decode_error)?,
        created_at: row.try_get("created_at").map_err(decode_error)?,
    })
}

fn record_from_row(row: &PgRow) -> Result<MiniAppRecord, StoreError> {
    let values: Value = row.try_get("values").map_err(decode_error)?;
    let Value::Object(values) = values else {
        return Err(StoreError::InvalidData(
            "record values are not an object".to_owned(),
        ));
    };
    Ok(MiniAppRecord {
        id: row.try_get("id").map_err(decode_error)?,
        mini_app_id: row.try_get("mini_app_id").map_err(decode_error)?,
        record_type: row.try_get("record_type").map_err(decode_error)?,
        values: values.into_iter().collect(),
        created_at: row.try_get("created_at").map_err(decode_error)?,
        updated_at: row.try_get("updated_at").map_err(decode_error)?,
    })
}

fn snapshot_from_row(row: &PgRow) -> Result<AgentRunSnapshot, StoreError> {
    Ok(AgentRunSnapshot {
        id: row.try_get("id").map_err(decode_error)?,
        session_id: row.try_get("session_id").map_err(decode_error)?,
        state: parse_run_state(row.try_get("state").map_err(decode_error)?)?,
        phase: parse_run_phase(row.try_get("phase").map_err(decode_error)?)?,
        reply: row.try_get("reply").map_err(decode_error)?,
        emotion: row.try_get("emotion").map_err(decode_error)?,
        created_emotion: row.try_get("created_emotion").map_err(decode_error)?,
        actions: json_array(row.try_get("actions").map_err(decode_error)?)?,
        children: json_array(row.try_get("children").map_err(decode_error)?)?,
        error: row.try_get("error").map_err(decode_error)?,
        created_at: row.try_get("created_at").map_err(decode_error)?,
        updated_at: row.try_get("updated_at").map_err(decode_error)?,
    })
}

fn event_from_row(row: &PgRow) -> Result<AgentRunEvent, StoreError> {
    Ok(AgentRunEvent {
        sequence: row.try_get("sequence").map_err(decode_error)?,
        event_type: row.try_get("event_type").map_err(decode_error)?,
        payload: row.try_get("payload").map_err(decode_error)?,
        created_at: row.try_get("created_at").map_err(decode_error)?,
    })
}

fn json_array(value: Value) -> Result<Vec<Value>, StoreError> {
    match value {
        Value::Array(items) => Ok(items),
        _ => Err(StoreError::InvalidData(
            "stored run field is not an array".to_owned(),
        )),
    }
}

fn parse_run_state(value: String) -> Result<RunState, StoreError> {
    match value.as_str() {
        "queued" => Ok(RunState::Queued),
        "running" => Ok(RunState::Running),
        "completed" => Ok(RunState::Completed),
        "failed" => Ok(RunState::Failed),
        "interrupted" => Ok(RunState::Interrupted),
        "cancelled" => Ok(RunState::Cancelled),
        _ => Err(StoreError::InvalidData(format!(
            "unknown run state: {value}"
        ))),
    }
}

fn parse_run_phase(value: String) -> Result<RunPhase, StoreError> {
    match value.as_str() {
        "admitted" => Ok(RunPhase::Admitted),
        "planning" => Ok(RunPhase::Planning),
        "delegating" => Ok(RunPhase::Delegating),
        "synthesizing" => Ok(RunPhase::Synthesizing),
        "completed" => Ok(RunPhase::Completed),
        "failed" => Ok(RunPhase::Failed),
        "interrupted" => Ok(RunPhase::Interrupted),
        "cancelled" => Ok(RunPhase::Cancelled),
        _ => Err(StoreError::InvalidData(format!(
            "unknown run phase: {value}"
        ))),
    }
}

fn decode_error(error: sqlx_core::Error) -> StoreError {
    StoreError::InvalidData(error.to_string())
}
