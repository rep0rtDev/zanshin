//! Periodic maintenance tasks.

use std::time::Duration;

use tokio::sync::watch;

use crate::state::AppState;

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    let mut sweep = tokio::time::interval(Duration::from_secs(60));
    let mut changes = state.bus.subscribe();
    loop {
        tokio::select! {
            _ = sweep.tick() => {
                sweep_push(&state).await;
                sweep_scheduled(&state).await;
                close_idle_sessions(&state).await;
                for key in state.audiences.due_for_recompute().await {
                    if let Err(e) = state.audiences.recompute(&key).await {
                        tracing::warn!(audience = %key, error = %e, "scheduled recompute failed");
                    }
                }
                if let Err(e) = state.taxonomy.repo.sync_labels().await {
                    tracing::debug!(error = %e, "label sync failed");
                }
            }
            c = changes.recv() => {
                if let Ok(c) = c {
                    if c.kind == crate::services::bus::ChangeKind::Message && c.identity_id.is_none() {
                        sweep_push(&state).await;
                    }
                }
            }
            _ = tick.tick() => {
                match state.identity.purge_expired_sessions().await {
                    Ok(n) if n > 0 => tracing::info!(purged = n, "expired sessions purged"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "session purge failed"),
                }
                match state.audit.prune().await {
                    Ok(n) if n > 0 => tracing::info!(purged = n, "old audit entries purged"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "audit prune failed"),
                }
                match state.messages.purge_expired().await {
                    Ok(n) if n > 0 => tracing::info!(purged = n, "expired inbox messages purged"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "inbox purge failed"),
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}

async fn sweep_push(state: &AppState) {
    let identities = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone());
    if let Err(e) = state
        .messages
        .sweep_push_campaigns(&state.eval, &identities)
        .await
    {
        tracing::warn!(error = %e, "push campaign sweep failed");
    }
}

async fn sweep_scheduled(state: &AppState) {
    let identities = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone());
    if let Err(e) = state
        .messages
        .sweep_scheduled(&state.eval, &identities)
        .await
    {
        tracing::warn!(error = %e, "scheduled campaign sweep failed");
    }
}

async fn close_idle_sessions(state: &AppState) {
    let idle = state.cfg.taxonomy.session_inactivity.as_secs() as i64;
    match state.taxonomy.repo.close_idle_sessions(idle, 1000).await {
        Ok(closed) => {
            for ps in closed {
                if let Ok(identity) = state.identity.get(ps.identity_id).await {
                    state
                        .events
                        .system(
                            &identity,
                            zanshin_core::events::EV_SESSION_END,
                            ps.duration_sec.unwrap_or(0).to_string(),
                            None,
                            Some(ps.id),
                        )
                        .await;
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "idle session sweep failed"),
    }
}
