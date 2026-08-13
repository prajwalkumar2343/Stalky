use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    credentials::{CredentialError, ProviderCredentialVault},
    protocol::{AgentRunFailure, AgentRunOutput},
    providers::{ProviderError, ProviderRegistry, ProviderRequest},
    store::{StoreError, StoreHandle},
};

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("worker storage operation failed")]
    Store(#[source] StoreError),
}

#[derive(Clone)]
pub struct AgentWorker {
    store: StoreHandle,
    providers: ProviderRegistry,
    vault: Arc<ProviderCredentialVault>,
    worker_id: String,
    lease_duration: Duration,
    poll_interval: Duration,
}

impl AgentWorker {
    pub fn new(
        store: StoreHandle,
        providers: ProviderRegistry,
        vault: Arc<ProviderCredentialVault>,
        worker_id: String,
    ) -> Self {
        Self {
            store,
            providers,
            vault,
            worker_id,
            lease_duration: Duration::from_secs(90),
            poll_interval: Duration::from_millis(500),
        }
    }

    pub fn with_timing(mut self, lease_duration: Duration, poll_interval: Duration) -> Self {
        self.lease_duration = lease_duration;
        self.poll_interval = poll_interval;
        self
    }

    pub async fn run_once(&self, now: chrono::DateTime<chrono::Utc>) -> Result<bool, WorkerError> {
        let Some(lease) = self
            .store
            .claim_agent_run(&self.worker_id, now, self.lease_duration)
            .await
            .map_err(WorkerError::Store)?
        else {
            return Ok(false);
        };
        let cancellation = CancellationToken::new();
        let heartbeat_cancellation = cancellation.clone();
        let store = self.store.clone();
        let heartbeat_lease = lease.clone();
        let heartbeat_duration = self.lease_duration;
        let heartbeat_every =
            Duration::from_millis((heartbeat_duration.as_millis() / 2).max(10) as u64);
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = heartbeat_cancellation.cancelled() => break,
                    _ = tokio::time::sleep(heartbeat_every) => {
                        match store.heartbeat_agent_run(&heartbeat_lease, chrono::Utc::now(), heartbeat_duration).await {
                            Ok(_) => {}
                            Err(StoreError::Cancelled | StoreError::StaleFence | StoreError::LateResult) => {
                                heartbeat_cancellation.cancel();
                                break;
                            }
                            Err(_) => {
                                heartbeat_cancellation.cancel();
                                break;
                            }
                        }
                    }
                }
            }
        });

        let result = self.execute(&lease, cancellation.clone()).await;
        cancellation.cancel();
        let _ = heartbeat.await;
        match result {
            Ok(output) => {
                match self
                    .store
                    .complete_agent_run(&lease, output, chrono::Utc::now())
                    .await
                {
                    Ok(_)
                    | Err(
                        StoreError::LateResult | StoreError::StaleFence | StoreError::Cancelled,
                    ) => {}
                    Err(error) => return Err(WorkerError::Store(error)),
                }
            }
            Err(error) => {
                let failure = AgentRunFailure {
                    code: error.code().to_owned(),
                    message: error.code().to_owned(),
                    retryable: error.retryable(),
                };
                match self
                    .store
                    .fail_agent_run(&lease, failure, chrono::Utc::now())
                    .await
                {
                    Ok(_)
                    | Err(
                        StoreError::LateResult | StoreError::StaleFence | StoreError::Cancelled,
                    ) => {}
                    Err(store_error) => return Err(WorkerError::Store(store_error)),
                }
            }
        }
        Ok(true)
    }

    pub async fn run_forever(&self, shutdown: CancellationToken) -> Result<(), WorkerError> {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                result = self.run_once(chrono::Utc::now()) => {
                    if !result? { tokio::time::sleep(self.poll_interval).await; }
                }
            }
        }
    }

    async fn execute(
        &self,
        lease: &crate::protocol::AgentLease,
        cancellation: CancellationToken,
    ) -> Result<AgentRunOutput, ProviderError> {
        let Some(ciphertext) = lease.credential_ciphertext.as_deref() else {
            return Err(ProviderError::MissingCredential);
        };
        let secret = self.vault.open(ciphertext).map_err(|error| match error {
            CredentialError::InvalidCiphertext
            | CredentialError::NotConfigured
            | CredentialError::InvalidKey => ProviderError::NotConfigured,
            CredentialError::Empty => ProviderError::MissingCredential,
        })?;
        let adapter = self
            .providers
            .get(&lease.request.provider)
            .ok_or(ProviderError::Unsupported)?;
        let response = adapter
            .complete(
                ProviderRequest::from_run(&lease.request),
                &secret,
                cancellation,
            )
            .await?;
        Ok(AgentRunOutput {
            reply: response.reply,
            emotion: response.emotion,
            created_emotion: response.created_emotion,
            actions: response.actions,
        })
    }
}

pub fn worker_id_from_env() -> String {
    std::env::var("STALKY_WORKER_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("worker-{}", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credentials::ProviderCredentialVault,
        providers::{FakeProvider, ProviderResponse},
    };

    fn provider_registry(response: Result<ProviderResponse, ProviderError>) -> ProviderRegistry {
        ProviderRegistry::from_adapters([Arc::new(FakeProvider::new("gemini", vec![response]))
            as Arc<dyn crate::providers::ProviderAdapter>])
    }

    fn request() -> crate::protocol::AgentRunRequest {
        crate::protocol::AgentRunRequest {
            message: "hello".to_owned(),
            session_id: None,
            provider: "gemini".to_owned(),
            api_key: "secret".to_owned(),
            model: "test".to_owned(),
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

    async fn queued_store() -> (StoreHandle, Uuid) {
        let store: StoreHandle = Arc::new(crate::store::InMemoryStore::new());
        let vault = ProviderCredentialVault::from_hex_key(&"55".repeat(32)).unwrap();
        let credential = vault.seal("secret").unwrap();
        let run = store
            .create_agent_run(Uuid::new_v4(), request(), None, Some(credential))
            .await
            .unwrap();
        (store, run.id)
    }

    #[tokio::test]
    async fn worker_completes_fake_provider_run() {
        let (store, run_id) = queued_store().await;
        let vault = Arc::new(ProviderCredentialVault::from_hex_key(&"55".repeat(32)).unwrap());
        let worker = AgentWorker::new(
            store.clone(),
            provider_registry(Ok(ProviderResponse {
                reply: "done".to_owned(),
                emotion: "focused".to_owned(),
                created_emotion: None,
                actions: Vec::new(),
            })),
            vault,
            "test-worker".to_owned(),
        );
        assert!(worker.run_once(chrono::Utc::now()).await.unwrap());
        let events = store
            .list_agent_events(Uuid::new_v4(), run_id)
            .await
            .unwrap();
        assert!(events.is_none());
    }
}
