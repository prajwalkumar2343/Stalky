use std::io;

use std::sync::Arc;

use stalky_backend::{
    app_with_store,
    config::Config,
    store::{InMemoryStore, PostgresStore, StoreHandle},
};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;
    let bind_address = config.bind_address;
    let listener = TcpListener::bind(bind_address).await?;
    let store: StoreHandle = if let Some(database_url) = config.database_url.as_deref() {
        Arc::new(PostgresStore::connect(database_url).await?)
    } else {
        tracing::warn!("DATABASE_URL is not configured; using process-local in-memory persistence");
        Arc::new(InMemoryStore::new())
    };
    tracing::info!(%bind_address, "stalky backend listening");
    axum::serve(listener, app_with_store(config, store))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install termination handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!(
        kind = io::ErrorKind::Interrupted.to_string(),
        "shutdown requested"
    );
}
