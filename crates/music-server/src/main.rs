#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::process::ExitCode;

use music_server::{AppConfig, AppRuntime, RuntimeError, initialize_tracing};
use tokio::net::TcpListener;

const LISTEN_ADDRESS: &str = "0.0.0.0:8000";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("music-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), RuntimeError> {
    let config = AppConfig::load()?;
    initialize_tracing(&config)?;
    let listener = TcpListener::bind(LISTEN_ADDRESS)
        .await
        .map_err(|source| RuntimeError::io("bind the HTTP listener", source))?;
    let runtime = AppRuntime::start(config).await?;
    tracing::info!(listen_address = LISTEN_ADDRESS, "music-server listening");
    runtime.run(listener, shutdown_signal()).await
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<(), RuntimeError> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())
        .map_err(|source| RuntimeError::io("install the SIGTERM handler", source))?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.map_err(|source| RuntimeError::io("wait for Ctrl-C", source))?;
        }
        _ = terminate.recv() => {}
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<(), RuntimeError> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|source| RuntimeError::io("wait for Ctrl-C", source))
}
