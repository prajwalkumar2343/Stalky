use std::{sync::Arc, time::Duration};

use chrono::{Duration as ChronoDuration, Utc};
use stalky_backend::{
    credentials::ProviderCredentialVault,
    protocol::{AgentRunFailure, AgentRunOutput, AgentRunRequest, RunState},
    providers::{FakeProvider, ProviderAdapter, ProviderError, ProviderRegistry},
    store::{InMemoryStore, StoreError, StoreHandle},
    worker::AgentWorker,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const KEY: &str = "66";

fn request() -> AgentRunRequest {
    AgentRunRequest {
        message: "hello".to_owned(),
        session_id: None,
        provider: "gemini".to_owned(),
        api_key: "provider-secret-do-not-log".to_owned(),
        model: "test-model".to_owned(),
        memories: Vec::new(),
        todos: Vec::new(),
        apps: Vec::new(),
        mini_apps: Vec::new(),
        automations: Vec::new(),
        context_files: Vec::new(),
        image_base64: None,
        image_mime_type: None,
    }
}

async fn queued_run() -> (StoreHandle, Uuid, Uuid, Arc<ProviderCredentialVault>) {
    let store: StoreHandle = Arc::new(InMemoryStore::new());
    let user_id = Uuid::new_v4();
    let vault = Arc::new(ProviderCredentialVault::from_hex_key(&KEY.repeat(32)).unwrap());
    let credential = vault.seal(request().api_key.as_str()).unwrap();
    let run = store
        .create_agent_run(user_id, request(), None, Some(credential))
        .await
        .unwrap();
    (store, user_id, run.id, vault)
}

fn success() -> AgentRunOutput {
    AgentRunOutput {
        reply: "done".to_owned(),
        emotion: "focused".to_owned(),
        created_emotion: None,
        actions: Vec::new(),
    }
}

#[tokio::test]
async fn duplicate_workers_cannot_claim_the_same_run() {
    let (store, _, _, _) = queued_run().await;
    let now = Utc::now();
    let first = store
        .claim_agent_run("worker-a", now, Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert!(
        store
            .claim_agent_run("worker-b", now, Duration::from_secs(30))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(first.worker_id, "worker-a");
}

#[tokio::test]
async fn lease_expiry_fences_stale_heartbeat_and_terminal_result() {
    let (store, user_id, run_id, _) = queued_run().await;
    let now = Utc::now();
    let old = store
        .claim_agent_run("worker-a", now, Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    let recovered_at = now + ChronoDuration::seconds(2);
    let current = store
        .claim_agent_run("worker-b", recovered_at, Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(old.fence_token, current.fence_token);
    assert!(matches!(
        store
            .heartbeat_agent_run(&old, recovered_at, Duration::from_secs(30))
            .await,
        Err(StoreError::StaleFence)
    ));
    assert!(matches!(
        store
            .complete_agent_run(&old, success(), recovered_at)
            .await,
        Err(StoreError::StaleFence)
    ));
    let finished = store
        .complete_agent_run(&current, success(), recovered_at)
        .await
        .unwrap();
    assert_eq!(finished.state, RunState::Completed);
    let events = store
        .list_agent_events(user_id, run_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[1].sequence == pair[0].sequence + 1)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "run.lease_expired")
    );
}

#[tokio::test]
async fn cancellation_fences_late_results_and_heartbeats() {
    let (store, user_id, run_id, _) = queued_run().await;
    let now = Utc::now();
    let lease = store
        .claim_agent_run("worker-a", now, Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    let cancelled = store
        .cancel_agent_run(user_id, run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.state, RunState::Cancelled);
    assert!(matches!(
        store
            .heartbeat_agent_run(&lease, now, Duration::from_secs(30))
            .await,
        Err(StoreError::Cancelled)
    ));
    assert!(matches!(
        store.complete_agent_run(&lease, success(), now).await,
        Err(StoreError::LateResult)
    ));
    let events = store
        .list_agent_events(user_id, run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(events.last().unwrap().event_type, "run.cancelled");
    assert!(
        events
            .iter()
            .all(|event| !event.payload.to_string().contains("provider-secret"))
    );
}

#[tokio::test]
async fn retryable_timeout_is_delayed_and_becomes_terminal_after_three_attempts() {
    let (store, _, _, _) = queued_run().await;
    let now = Utc::now();
    let failure = || AgentRunFailure {
        code: "provider.timeout".to_owned(),
        message: "provider.timeout".to_owned(),
        retryable: true,
    };
    let first = store
        .claim_agent_run("worker", now, Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .fail_agent_run(&first, failure(), now)
            .await
            .unwrap()
            .state,
        RunState::Queued
    );
    assert!(
        store
            .claim_agent_run("worker", now, Duration::from_secs(30))
            .await
            .unwrap()
            .is_none()
    );
    let second = store
        .claim_agent_run(
            "worker",
            now + ChronoDuration::seconds(2),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .fail_agent_run(&second, failure(), now + ChronoDuration::seconds(2))
            .await
            .unwrap()
            .state,
        RunState::Queued
    );
    let third = store
        .claim_agent_run(
            "worker",
            now + ChronoDuration::seconds(6),
            Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .fail_agent_run(&third, failure(), now + ChronoDuration::seconds(6))
            .await
            .unwrap()
            .state,
        RunState::Failed
    );
}

#[tokio::test]
async fn worker_persists_provider_failure_and_stops_on_cancellation() {
    let (store, user_id, run_id, vault) = queued_run().await;
    let failed_provider = FakeProvider::new("gemini", vec![Err(ProviderError::Timeout)]);
    let registry =
        ProviderRegistry::from_adapters([Arc::new(failed_provider) as Arc<dyn ProviderAdapter>]);
    let worker = AgentWorker::new(
        store.clone(),
        registry,
        vault.clone(),
        "worker-failure".to_owned(),
    );
    assert!(worker.run_once(Utc::now()).await.unwrap());
    assert_eq!(
        store
            .get_agent_run(user_id, run_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RunState::Queued
    );

    let (store, user_id, run_id, vault) = queued_run().await;
    let delayed = FakeProvider::delayed("gemini", Duration::from_secs(5));
    let registry = ProviderRegistry::from_adapters([Arc::new(delayed) as Arc<dyn ProviderAdapter>]);
    let worker = AgentWorker::new(store.clone(), registry, vault, "worker-cancel".to_owned())
        .with_timing(Duration::from_millis(100), Duration::from_millis(1));
    let task = tokio::spawn(async move { worker.run_once(Utc::now()).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(20)).await;
    store.cancel_agent_run(user_id, run_id).await.unwrap();
    assert!(task.await.unwrap());
    assert_eq!(
        store
            .get_agent_run(user_id, run_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RunState::Cancelled
    );
}

#[tokio::test]
async fn provider_timeout_and_cancellation_do_not_make_live_calls() {
    let vault = ProviderCredentialVault::from_hex_key(&KEY.repeat(32)).unwrap();
    let secret = vault.open(&vault.seal("secret").unwrap()).unwrap();
    let fake = FakeProvider::delayed("gemini", Duration::from_secs(5));
    let token = CancellationToken::new();
    token.cancel();
    let result = fake
        .complete(
            stalky_backend::providers::ProviderRequest {
                model: "test".to_owned(),
                message: "hello".to_owned(),
                image_base64: None,
                image_mime_type: None,
            },
            &secret,
            token,
        )
        .await;
    assert!(matches!(result, Err(ProviderError::Cancelled)));
}
