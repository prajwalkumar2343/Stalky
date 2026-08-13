use std::sync::Arc;

use stalky_backend::{
    config::Config,
    credentials::ProviderCredentialVault,
    providers::ProviderRegistry,
    store::PostgresStore,
    worker::{AgentWorker, worker_id_from_env},
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stalky_backend=info".into()),
        )
        .json()
        .init();
    let config = Config::from_env()?;
    let database_url = config
        .database_url
        .as_deref()
        .ok_or("DATABASE_URL is required for the agent worker")?;
    let store = Arc::new(PostgresStore::connect(database_url).await?);
    let vault = Arc::new(ProviderCredentialVault::from_env()?);
    let worker = AgentWorker::new(
        store,
        ProviderRegistry::default(),
        vault,
        worker_id_from_env(),
    );
    let shutdown = CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_shutdown.cancel();
    });
    worker.run_forever(shutdown).await?;
    Ok(())
}
