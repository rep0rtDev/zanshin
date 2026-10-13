use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio::signal;
use zanshin_server::{config::Config, grpc, http, state::AppState, telemetry};

#[derive(Parser, Debug)]
#[command(name = "zanshin", version, about = "Zanshin LiveOps server")]
struct Cli {
    /// Path to a YAML config file. Every value can also be set with `ZANSHIN__SECTION__KEY` env vars.
    #[arg(short, long, env = "ZANSHIN_CONFIG")]
    config: Option<std::path::PathBuf>,
    /// Print the effective configuration (secrets redacted) and exit.
    #[arg(long)]
    check_config: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref()).context("loading configuration")?;
    if cli.check_config {
        println!("{}", cfg.redacted_yaml()?);
        return Ok(());
    }
    telemetry::init(&cfg.log)?;
    telemetry::prometheus();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting zanshin");

    let state = AppState::build(Arc::new(cfg)).await?;

    let http_addr = state.cfg.server.http_addr;
    let grpc_addr = state.cfg.server.grpc_addr;

    let shutdown = shutdown_signal();
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown.await;
        tracing::info!("shutdown signal received");
        let _ = tx.send(true);
    });

    let http_task = tokio::spawn(http::serve(state.clone(), http_addr, rx.clone()));
    let grpc_task = tokio::spawn(grpc::serve(state.clone(), grpc_addr, rx.clone()));
    let housekeeping = tokio::spawn(zanshin_server::housekeeping::run(state.clone(), rx));

    let (h, g, _) = tokio::try_join!(http_task, grpc_task, housekeeping)?;
    h?;
    g?;
    tracing::info!("bye");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install sigterm handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
