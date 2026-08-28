#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use music_output::client::run_websocket_client;
use music_output::config::{OutputArgs, OutputConfig};
use music_output::control::{bind_is_loopback, control_router};
use music_output::runtime::OutputRuntime;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;

type MainError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), MainError> {
    initialize_tracing();
    let config = OutputConfig::resolve(OutputArgs::parse())?;
    let runtime = OutputRuntime::start(config).await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut tasks: JoinSet<Result<(), MainError>> = JoinSet::new();

    let websocket_runtime = Arc::clone(&runtime);
    let websocket_shutdown = shutdown_rx.clone();
    tasks.spawn(async move {
        run_websocket_client(websocket_runtime, websocket_shutdown)
            .await
            .map_err(|error| Box::new(error) as MainError)
    });

    if let Some(port) = runtime.config().control_port {
        let bind = runtime.config().control_bind.clone();
        let exposed = !bind_is_loopback(&bind);
        if exposed && runtime.config().control_token.is_none() {
            tracing::warn!(
                bind = %bind,
                port,
                "control endpoint is network-accessible without a token"
            );
        }
        let listener = tokio::net::TcpListener::bind((bind.as_str(), port)).await?;
        let address = listener.local_addr()?;
        let router = control_router(
            Arc::clone(&runtime),
            runtime.config().control_token.clone(),
            exposed,
        );
        let control_shutdown = shutdown_rx.clone();
        tasks.spawn(async move {
            tracing::info!(%address, exposed, "control endpoint ready");
            axum::serve(listener, router)
                .with_graceful_shutdown(wait_for_shutdown(control_shutdown))
                .await
                .map_err(|error| Box::new(error) as MainError)
        });
    }

    tracing::info!(
        name = %runtime.config().name,
        server = %runtime.config().server_url,
        client_id = %runtime.config().client_id,
        console_gate = runtime.config().respect_console,
        sfx = runtime.config().play_sfx,
        "headless output started"
    );

    let mut failure = None;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            if let Err(error) = signal {
                failure = Some(Box::new(error) as MainError);
            }
        }
        completed = tasks.join_next() => {
            failure = match completed {
                Some(Ok(Ok(()))) => Some("output task stopped unexpectedly".into()),
                Some(Ok(Err(error))) => Some(error),
                Some(Err(error)) => Some(Box::new(error)),
                None => Some("output runtime had no active tasks".into()),
            };
        }
    }

    let _ = shutdown_tx.send(true);
    let drain = async {
        while let Some(result) = tasks.join_next().await {
            if let Ok(Err(error)) = result {
                tracing::warn!(error = %error, "output task failed during shutdown");
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), drain).await;
    tasks.abort_all();
    runtime.shutdown().await;
    if let Some(error) = failure {
        Err(error)
    } else {
        Ok(())
    }
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

fn initialize_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
