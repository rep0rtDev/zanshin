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
    /// Apply database migrations and exit (for init containers / release steps).
    #[arg(long)]
    migrate: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref()).context("loading configuration")?;
    if cli.check_config {
        println!("{}", cfg.redacted_yaml()?);
        return Ok(());
    }
    telemetry::init_with(&cfg.log, &cfg.telemetry)?;
    telemetry::prometheus();
    if cli.migrate {
        zanshin_store::Stores::migrate_only(&cfg.database).await?;
        tracing::info!("migrations applied");
        return Ok(());
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        production = cfg.server.production,
        "starting zanshin"
    );

    let state = AppState::build(Arc::new(cfg)).await?;
    let http_addr = state.cfg.server.http_addr;
    let grpc_addr = state.cfg.server.grpc_addr;
    let grace = state.cfg.server.shutdown_grace;
    let deadline = state.cfg.server.shutdown_timeout;

    // Two-phase shutdown: signal → drain (readyz 503, listeners still open) → stop listeners.
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    {
        let state = state.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            tracing::info!(grace = ?grace, "shutdown signal received, draining");
            state.begin_drain();
            tokio::time::sleep(grace).await;
            let _ = stop_tx.send(true);
            let _ = state.shutdown.send(true);
        });
    }

    let http_task = tokio::spawn(http::serve(state.clone(), http_addr, stop_rx.clone()));
    let grpc_task = tokio::spawn(grpc::serve(state.clone(), grpc_addr, stop_rx.clone()));
    let housekeeping = tokio::spawn(zanshin_server::housekeeping::run(state.clone(), stop_rx));

    let mut stop = state.shutdown.subscribe();
    let joined = async { tokio::try_join!(http_task, grpc_task, housekeeping) };
    tokio::pin!(joined);

    // Either every task ends on its own (startup failure) or the drain finishes and we give the
    // listeners `shutdown_timeout` to close their connections.
    let outcome = tokio::select! {
        r = &mut joined => r,
        _ = stop.wait_for(|v| *v) => match tokio::time::timeout(deadline, joined).await {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(?deadline, "shutdown deadline hit, exiting with open connections");
                telemetry::shutdown();
                return Ok(());
            }
        },
    };
    let (h, g, _) = outcome?;
    h?;
    g?;
    telemetry::shutdown();
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
