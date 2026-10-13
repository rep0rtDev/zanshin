//! Periodic maintenance tasks.

use std::time::Duration;

use tokio::sync::watch;

use crate::state::AppState;

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                match state.identity.purge_expired_sessions().await {
                    Ok(n) if n > 0 => tracing::info!(purged = n, "expired sessions purged"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "session purge failed"),
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}
