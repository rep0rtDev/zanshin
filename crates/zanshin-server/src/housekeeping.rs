//! Periodic maintenance. Every replica runs this loop; each job takes a cluster-wide advisory
//! lock first (see `zanshin_store::locks`), so with N replicas a job still runs once per tick and
//! fails over automatically when the replica holding the lock dies.

use std::time::Duration;

use tokio::sync::watch;
use zanshin_store::locks::Lock;

use crate::state::AppState;

pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    let mut sweep = tokio::time::interval(Duration::from_secs(60));
    let mut gauges = tokio::time::interval(Duration::from_secs(10));
    let mut changes = state.bus.subscribe();

    loop {
        tokio::select! {
            _ = gauges.tick() => report_gauges(&state),
            _ = sweep.tick() => {
                if interruptible(&mut shutdown, sweeps(&state)).await { break; }
            }
            c = changes.recv() => {
                let broadcast = c.as_ref().is_ok_and(|c| {
                    c.kind == crate::services::bus::ChangeKind::Message && c.identity_id.is_none()
                });
                if broadcast && interruptible(&mut shutdown, locked(&state, "push_sweep", || sweep_push(&state))).await {
                    break;
                }
            }
            _ = tick.tick() => {
                if interruptible(&mut shutdown, locked(&state, "purge", || purge(&state))).await { break; }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
    tracing::info!("housekeeping stopped");
}

/// Runs `job` unless shutdown is requested first; returns true when shutdown won. Jobs are
/// idempotent sweeps and their cluster lock is transaction-scoped, so dropping them mid-way is safe.
async fn interruptible<F: std::future::Future<Output = ()>>(
    shutdown: &mut watch::Receiver<bool>,
    job: F,
) -> bool {
    tokio::select! {
        _ = job => false,
        r = shutdown.wait_for(|v| *v) => r.is_ok(),
    }
}

async fn sweeps(state: &AppState) {
    locked(state, "push_sweep", || sweep_push(state)).await;
    locked(state, "scheduled_sweep", || sweep_scheduled(state)).await;
    locked(state, "idle_sessions", || close_idle_sessions(state)).await;
    locked(state, "export_runs", || state.export.run_due()).await;
    locked(state, "audience_recompute", || async {
        for key in state.audiences.due_for_recompute().await {
            if let Err(e) = state.audiences.recompute(&key).await {
                tracing::warn!(audience = %key, error = %e, "scheduled recompute failed");
            }
        }
    })
    .await;
    locked(state, "label_sync", || async {
        if let Err(e) = state.taxonomy.repo.sync_labels().await {
            tracing::debug!(error = %e, "label sync failed");
        }
    })
    .await;
}

/// Run `job` under the cluster lock `name`; lock errors are logged and the job is skipped.
async fn locked<F, Fut>(state: &AppState, name: &str, job: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let started = std::time::Instant::now();
    tracing::debug!(job = name, "job started");
    match state.locks.try_run(name, job).await {
        Ok(Lock::Ran(())) => {
            tracing::debug!(job = name, elapsed = ?started.elapsed(), "job finished")
        }
        Ok(Lock::Skipped) => tracing::trace!(job = name, "held by another replica"),
        Err(e) => tracing::warn!(job = name, error = %e, "job lock failed"),
    }
}

fn report_gauges(state: &AppState) {
    let pg = &state.stores.pg;
    metrics::gauge!("zanshin_db_pool_connections", "state" => "total").set(pg.size() as f64);
    metrics::gauge!("zanshin_db_pool_connections", "state" => "idle").set(pg.num_idle() as f64);
    metrics::gauge!("zanshin_db_pool_connections", "state" => "max")
        .set(state.cfg.database.max_connections as f64);
    metrics::gauge!("zanshin_event_queue_depth").set(state.events.queue_depth() as f64);
    metrics::gauge!("zanshin_uptime_seconds")
        .set((chrono::Utc::now() - state.started_at).num_seconds() as f64);
}

async fn purge(state: &AppState) {
    let r = &state.cfg.retention;
    let dur = |d: Duration| chrono::Duration::from_std(d).unwrap_or(chrono::Duration::MAX);
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
    if !r.export_runs.is_zero() {
        if let Err(e) = state.export.purge_runs(dur(r.export_runs)).await {
            tracing::warn!(error = %e, "export run purge failed");
        }
    }
    if !r.config_history.is_zero() {
        if let Err(e) = state.config_history.purge(dur(r.config_history)).await {
            tracing::warn!(error = %e, "config history purge failed");
        }
    }
    if !r.annotations.is_zero() {
        if let Err(e) = state.analytics.purge_annotations(dur(r.annotations)).await {
            tracing::warn!(error = %e, "annotation purge failed");
        }
    }
    if !r.journey_runs.is_zero() {
        match state.journeys.purge(dur(r.journey_runs)).await {
            Ok(n) if n > 0 => tracing::info!(purged = n, "old journey runs purged"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "journey run purge failed"),
        }
    }
    if !r.webhook_deliveries.is_zero() {
        match state.webhooks.purge(dur(r.webhook_deliveries)).await {
            Ok(n) if n > 0 => tracing::info!(purged = n, "old webhook deliveries purged"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "webhook delivery purge failed"),
        }
    }
    match state.messages.purge_expired().await {
        Ok(n) if n > 0 => tracing::info!(purged = n, "expired inbox messages purged"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "inbox purge failed"),
    }
    if !r.events.is_zero() {
        let before = chrono::Utc::now() - dur(r.events);
        match state.events.purge_before(before, r.purge_batch).await {
            Ok(n) if n > 0 => tracing::info!(purged = n, "expired events purged"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "event purge failed"),
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
