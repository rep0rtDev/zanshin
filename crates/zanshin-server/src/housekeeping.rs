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
    locked(state, "message_deferrals", || sweep_deferrals(state)).await;
    locked(state, "idle_sessions", || close_idle_sessions(state)).await;
    locked(state, "export_runs", || state.export.run_due()).await;
    locked(state, "currency_rates", || async {
        if let Err(e) = state.currency.refresh_remote(false).await {
            tracing::warn!(error = %e, "currency rate refresh failed");
        }
    })
    .await;
    locked(state, "audience_recompute", || async {
        for key in state.audiences.due_for_recompute().await {
            if let Err(e) = state.audiences.recompute(&key).await {
                tracing::warn!(audience = %key, error = %e, "scheduled recompute failed");
            }
        }
    })
    .await;
    locked(state, "flag_schedules", || flag_schedules(state)).await;
    flag_eval_flush(state).await;
    locked(state, "experiment_guardrails", || guardrails(state)).await;
    locked(state, "experiment_bandit", || bandit(state)).await;
    locked(state, "metric_alerts", || state.analytics_v5.check_alerts()).await;
    locked(state, "report_schedules", || {
        state.analytics_v5.run_due_reports()
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
    state.jobs.started(name);
    match state.locks.try_run(name, job).await {
        Ok(Lock::Ran(())) => {
            tracing::debug!(job = name, elapsed = ?started.elapsed(), "job finished");
            state.jobs.finished(name, started.elapsed());
        }
        Ok(Lock::Skipped) => {
            tracing::trace!(job = name, "held by another replica");
            state.jobs.skipped(name);
        }
        Err(e) => {
            tracing::warn!(job = name, error = %e, "job lock failed");
            state.jobs.failed(name, e.to_string());
        }
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
    match state.console_users.repo().purge_sessions().await {
        Ok(n) if n > 0 => tracing::info!(purged = n, "stale console sessions purged"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "console session purge failed"),
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

/// Guardrail metrics: pause a running experiment whose variant regresses a protected metric by
/// more than the tolerated relative drop against control.
async fn guardrails(state: &AppState) {
    let now = chrono::Utc::now();
    for exp in state.experiments.list().await {
        if exp.status != zanshin_core::experiments::Status::Running || exp.guardrails.is_empty() {
            continue;
        }
        let Some(control) = exp.control.as_deref() else {
            continue;
        };
        let Ok((_, stats)) = state.experiments.results(&exp.key).await else {
            continue;
        };
        let mut scoped = (*exp).clone();
        scoped.metrics = exp.guardrails.iter().map(|g| g.metric.clone()).collect();
        let rows = state
            .metrics
            .for_experiment(&scoped, &stats, exp.created_at, now)
            .await;
        for g in &exp.guardrails {
            let Some(row) = rows.iter().find(|r| r["metric"] == g.metric) else {
                continue;
            };
            let Some(variants) = row["variants"].as_array() else {
                continue;
            };
            let value = |k: &str| {
                variants
                    .iter()
                    .find(|v| v["variant"] == k)
                    .and_then(|v| v["value"].as_f64())
            };
            let identities = |k: &str| {
                variants
                    .iter()
                    .find(|v| v["variant"] == k)
                    .and_then(|v| {
                        v["stats"]["trials"]
                            .as_i64()
                            .or_else(|| stats.iter().find(|s| s.variant == k).map(|s| s.assigned))
                    })
                    .unwrap_or(0)
            };
            let Some(c) = value(control) else { continue };
            if c <= 0.0 || identities(control) < g.min_identities {
                continue;
            }
            for v in &exp.variants {
                if v.key == control || identities(&v.key) < g.min_identities {
                    continue;
                }
                let Some(x) = value(&v.key) else { continue };
                let drop = (c - x) / c;
                if drop > g.max_drop {
                    let reason = format!(
                        "guardrail {} tripped: variant {} is {:.1}% below control ({} vs {}), limit {:.1}%",
                        g.metric, v.key, drop * 100.0, x, c, g.max_drop * 100.0
                    );
                    match state.experiments.interrupt(&exp.key, &reason, None).await {
                        Ok(_) => {
                            tracing::warn!(experiment = %exp.key, %reason, "experiment paused by guardrail");
                            state
                                .notifications
                                .notify(zanshin_core::notifications::Notice::new(
                                    "alert",
                                    format!("Experiment `{}` paused by guardrail", exp.key),
                                    reason.clone(),
                                ));
                            state.webhooks.emit(
                                "experiment.guardrail_tripped",
                                serde_json::json!({ "experiment": exp.key, "metric": g.metric, "variant": v.key, "drop": drop, "reason": reason }),
                            );
                        }
                        Err(e) => {
                            tracing::warn!(experiment = %exp.key, error = %e, "guardrail interrupt failed")
                        }
                    }
                    break;
                }
            }
        }
    }
}

/// Bandit allocation: re-derive variant weights from Thompson sampling on the configured
/// conversion metric. New players follow the new weights; assigned players stay where they are.
async fn bandit(state: &AppState) {
    let now = chrono::Utc::now();
    for exp in state.experiments.list().await {
        let Some(b) = exp.bandit.clone() else {
            continue;
        };
        if exp.status != zanshin_core::experiments::Status::Running {
            continue;
        }
        if b.last_run_at
            .is_some_and(|t| (now - t).num_seconds() < b.every_secs as i64)
        {
            continue;
        }
        let Ok((_, stats)) = state.experiments.results(&exp.key).await else {
            continue;
        };
        let total: i64 = stats.iter().map(|s| s.assigned).sum();
        if total < b.min_assigned {
            continue;
        }
        let (event_name, field) = match state.metrics.get(&b.metric).await {
            Ok(m) => (m.event_name.clone(), m.metadata_field().map(str::to_string)),
            Err(_) => (b.metric.clone(), None),
        };
        let Ok(totals) = state
            .events
            .experiment_metric_field(&exp.key, &event_name, field.as_deref(), exp.created_at, now)
            .await
        else {
            continue;
        };
        let arms: Vec<(i64, i64)> = exp
            .variants
            .iter()
            .map(|v| {
                let succ = totals
                    .iter()
                    .find(|t| t.variant == v.key)
                    .map(|t| t.identities)
                    .unwrap_or(0);
                let trials = stats
                    .iter()
                    .find(|s| s.variant == v.key)
                    .map(|s| s.assigned)
                    .unwrap_or(0);
                (succ.min(trials.max(succ)), trials.max(succ))
            })
            .collect();
        let weights = zanshin_core::stats::thompson_weights(&arms, b.min_weight);
        let probs = zanshin_core::stats::best_arm_probabilities(&arms);
        let mut variants = exp.variants.clone();
        for (v, w) in variants.iter_mut().zip(&weights) {
            v.weight = (w * 10.0).round() / 10.0;
        }
        let patch = crate::services::experiments::ExperimentPatch {
            variants: Some(variants),
            bandit: Some(Some(zanshin_core::experiments::BanditConfig {
                last_run_at: Some(now),
                last_probabilities: probs,
                ..b
            })),
            ..Default::default()
        };
        match state.experiments.update(&exp.key, patch).await {
            Ok(_) => tracing::info!(experiment = %exp.key, ?weights, "bandit weights updated"),
            Err(e) => tracing::warn!(experiment = %exp.key, error = %e, "bandit update failed"),
        }
    }
}

/// Apply flag patches whose scheduled time has come.
async fn flag_schedules(state: &AppState) {
    let now = chrono::Utc::now();
    let due = match state.flags_v3.due_schedules(now).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "flag schedules: query failed");
            return;
        }
    };
    for s in due {
        let res = crate::services::config::apply(state, "flags", &s.flag_key, &s.patch).await;
        let (status, err) = match &res {
            Ok(_) => ("applied", String::new()),
            Err(e) => ("failed", e.to_string()),
        };
        if let Err(e) = state.flags_v3.finish_schedule(s.id, status, &err).await {
            tracing::warn!(error = %e, "flag schedules: finish failed");
        }
        state.webhooks.emit(
            "flag.schedule_applied",
            serde_json::json!({ "flag": s.flag_key, "schedule_id": s.id, "at": s.at, "status": status, "error": err, "patch": s.patch }),
        );
    }
}

/// Flush in-memory flag evaluation counters (every node, no lock needed: the upsert adds).
async fn flag_eval_flush(state: &AppState) {
    let counts = state.eval.drain_eval_counts();
    if counts.is_empty() {
        return;
    }
    let rows: Vec<(String, String, i64)> = counts
        .into_iter()
        .map(|((f, v), n)| (f, v, n as i64))
        .collect();
    if let Err(e) = state.flags_v3.add_eval_counts(&rows).await {
        tracing::warn!(error = %e, "flag eval counters: flush failed");
    }
}

/// Run the flag jobs once (tests / `zanshinctl`), without the cluster lock.
pub async fn sweep_once(state: &AppState) {
    flag_schedules(state).await;
    flag_eval_flush(state).await;
}

async fn sweep_deferrals(state: &AppState) {
    let identities = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone());
    match state
        .messages
        .sweep_deferrals(&state.eval, &identities)
        .await
    {
        Ok(n) if n > 0 => tracing::info!(delivered = n, "deferred messages delivered"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "deferred message sweep failed"),
    }
}
