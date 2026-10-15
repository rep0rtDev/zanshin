//! Platform → Health: one endpoint that tells an on-call person whether this deployment is doing
//! its background work. Combines the readiness probe (PostgreSQL / Redis / ClickHouse), the
//! ingest queue, stream connections, per-replica housekeeping job history, and the last-known
//! state of every integration, webhook queue, export sink, scheduled report and metric alert.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Default)]
pub struct JobRun {
    pub name: String,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_finished_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_ms: u64,
    pub runs: u64,
    pub skipped: u64,
    pub last_error: Option<String>,
}

/// In-memory history of housekeeping jobs on this replica; updated by `housekeeping::locked`.
#[derive(Clone, Default)]
pub struct JobRegistry(Arc<Mutex<HashMap<String, JobRun>>>);

impl JobRegistry {
    fn with(&self, name: &str, f: impl FnOnce(&mut JobRun)) {
        if let Ok(mut g) = self.0.lock() {
            let e = g.entry(name.to_string()).or_insert_with(|| JobRun {
                name: name.to_string(),
                ..Default::default()
            });
            f(e);
        }
    }

    pub fn started(&self, name: &str) {
        self.with(name, |j| j.last_started_at = Some(Utc::now()));
    }

    pub fn finished(&self, name: &str, took: Duration) {
        self.with(name, |j| {
            let now = Utc::now();
            j.last_finished_at = Some(now);
            j.last_seen_at = Some(now);
            j.last_ms = took.as_millis() as u64;
            j.runs += 1;
            j.last_error = None;
        });
    }

    pub fn skipped(&self, name: &str) {
        self.with(name, |j| {
            j.last_seen_at = Some(Utc::now());
            j.skipped += 1;
        });
    }

    pub fn failed(&self, name: &str, err: String) {
        self.with(name, |j| {
            j.last_seen_at = Some(Utc::now());
            j.last_error = Some(err);
        });
    }

    pub fn snapshot(&self) -> Vec<JobRun> {
        let mut v: Vec<JobRun> = self
            .0
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

/// How often each housekeeping job is scheduled (see `housekeeping::run`).
pub fn job_interval_secs(name: &str) -> i64 {
    match name {
        "purge" => 600,
        _ => 60,
    }
}

/// Sum of all samples of a Prometheus metric in the process-wide recorder (labels collapsed).
fn metric_sum(rendered: &str, name: &str) -> f64 {
    rendered
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (head, val) = l.rsplit_once(' ')?;
            let metric = head.split('{').next()?;
            (metric == name).then(|| val.parse::<f64>().ok()).flatten()
        })
        .sum()
}

fn short(s: &str) -> String {
    let s = s.lines().next().unwrap_or("").trim();
    if s.chars().count() > 160 {
        format!("{}…", s.chars().take(160).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Lists up to three offenders by name, then "and N more" — keeps `issues[]` readable when a
/// whole class of channels is broken rather than one.
fn grouped(issues: &mut Vec<String>, what: &str, failed: Vec<(String, String)>) {
    match failed.len() {
        0 => {}
        1..=3 => {
            for (k, e) in failed {
                issues.push(format!("{what} {k} failed: {e}"));
            }
        }
        n => {
            let names: Vec<&str> = failed.iter().take(3).map(|(k, _)| k.as_str()).collect();
            issues.push(format!(
                "{n} {what}s failed their last delivery ({}, and {} more)",
                names.join(", "),
                n - 3
            ));
        }
    }
}

pub async fn overview(state: &AppState) -> zanshin_core::Result<Value> {
    let now = Utc::now();
    let mut issues: Vec<String> = Vec::new();

    let deps = state.stores.ping_timeout().await;
    if !deps.postgres.ok {
        issues.push("PostgreSQL is unreachable".into());
    }
    if !deps.redis.ok {
        issues.push("Redis is unreachable — rate limits and change fan-out are degraded".into());
    }
    if deps.clickhouse.as_ref().is_some_and(|c| !c.ok) {
        issues.push("ClickHouse is unreachable — analytics fall back to PostgreSQL".into());
    }

    let rendered = crate::telemetry::prometheus().render();
    let queue_depth = state.events.queue_depth();
    let queue_capacity = state.events.queue_capacity();
    if queue_depth * 5 > queue_capacity * 4 {
        issues.push(format!(
            "event queue is {}% full — the store is not keeping up",
            queue_depth * 100 / queue_capacity.max(1)
        ));
    }
    let (events_5m, _) = state
        .events
        .count_since(now - chrono::Duration::minutes(5))
        .await
        .unwrap_or((0, 0));
    let (events_1h, identities_1h) = state
        .events
        .count_since(now - chrono::Duration::hours(1))
        .await
        .unwrap_or((0, 0));

    let uptime = (now - state.started_at).num_seconds();
    let jobs: Vec<Value> = state
        .jobs
        .snapshot()
        .into_iter()
        .map(|j| {
            let interval = job_interval_secs(&j.name);
            let age = j.last_seen_at.map(|t| (now - t).num_seconds());
            let stale = age.is_some_and(|a| a > interval * 3);
            if stale {
                issues.push(format!(
                    "job {} has not run for {}s (expected every {interval}s)",
                    j.name,
                    age.unwrap_or(0)
                ));
            }
            if let Some(e) = &j.last_error {
                issues.push(format!("job {} failed: {e}", j.name));
            }
            json!({
                "name": j.name, "interval_secs": interval, "stale": stale,
                "last_started_at": j.last_started_at, "last_finished_at": j.last_finished_at,
                "last_seen_at": j.last_seen_at, "last_ms": j.last_ms, "runs": j.runs,
                "skipped": j.skipped, "last_error": j.last_error,
            })
        })
        .collect();
    if jobs.is_empty() && uptime > 180 {
        issues.push("housekeeping has not reported any job on this replica".into());
    }

    let mut failed_integrations = Vec::new();
    let integrations: Vec<Value> = state
        .integrations
        .list()
        .await?
        .into_iter()
        .map(|i| {
            if i.enabled && i.health.last_status.as_deref() == Some("failed") {
                failed_integrations.push((
                    i.key.clone(),
                    short(i.health.last_error.as_deref().unwrap_or_default()),
                ));
            }
            json!({
                "key": i.key, "name": i.name, "kind": i.kind, "enabled": i.enabled,
                "last_status": i.health.last_status, "last_at": i.health.last_at,
                "last_error": i.health.last_error, "sent": i.health.sent_count, "failed": i.health.failed_count,
            })
        })
        .collect();
    grouped(&mut issues, "integration", failed_integrations);

    let hooks = state.webhooks.list().await?;
    let deliveries = state.webhooks.delivery_overview().await?;
    if deliveries.dead_24h > 0 {
        issues.push(format!(
            "{} webhook deliveries gave up in the last 24h",
            deliveries.dead_24h
        ));
    }
    if deliveries.oldest_pending_secs > 600 {
        issues.push(format!(
            "oldest undelivered webhook is {}s old",
            deliveries.oldest_pending_secs
        ));
    }

    let mut exports: Vec<Value> = Vec::new();
    let mut failed_exports = Vec::new();
    for s in state.export.list().await? {
        let last = state.export.runs(&s.key, 1).await?.into_iter().next();
        if s.enabled && last.as_ref().is_some_and(|r| r.status == "failed") {
            failed_exports.push((
                s.key.clone(),
                short(last.as_ref().map(|r| r.error.as_str()).unwrap_or_default()),
            ));
        }
        exports.push(json!({
            "key": s.key, "name": s.name, "kind": s.kind, "enabled": s.enabled,
            "last_run_at": s.last_run_at, "next_run_at": s.next_run_at, "cursor_at": s.cursor_at,
            "last_status": last.as_ref().map(|r| r.status.clone()),
            "last_error": last.as_ref().map(|r| r.error.clone()),
            "last_rows": last.as_ref().map(|r| r.rows),
        }));
    }
    grouped(&mut issues, "export sink", failed_exports);

    let mut failed_reports = Vec::new();
    let reports: Vec<Value> = state
        .analytics_v5
        .reports
        .list()
        .await?
        .into_iter()
        .map(|r| {
            if r.enabled && r.last_status.starts_with("failed") {
                failed_reports.push((r.key.clone(), short(&r.last_status)));
            }
            json!({ "key": r.key, "name": r.name, "enabled": r.enabled, "cron": r.cron,
                    "last_run_at": r.last_run_at, "last_status": r.last_status })
        })
        .collect();
    grouped(&mut issues, "report", failed_reports);

    let alerts: Vec<Value> = state
        .analytics_v5
        .alerts
        .list()
        .await?
        .into_iter()
        .map(|a| {
            json!({ "key": a.key, "name": a.name, "enabled": a.enabled, "event": a.event,
                    "last_checked_at": a.last_checked_at, "last_fired_at": a.last_fired_at,
                    "last_value": a.last_value,
                    "firing": a.last_fired_at.is_some_and(|t| (now - t).num_seconds() < a.cooldown_secs) })
        })
        .collect();

    let status = if !deps.ok {
        "fail"
    } else if issues.is_empty() {
        "ok"
    } else {
        "warn"
    };
    Ok(json!({
        "status": status,
        "issues": issues,
        "checked_at": now,
        "process": {
            "version": env!("CARGO_PKG_VERSION"),
            "project": state.project,
            "node_id": state.bus.node_id(),
            "started_at": state.started_at,
            "uptime_seconds": uptime,
            "draining": state.is_draining(),
            "production": state.cfg.server.production,
        },
        "dependencies": deps,
        "ingest": {
            "sink": state.events.backend(),
            "queue_depth": queue_depth,
            "queue_capacity": queue_capacity,
            "events_5m": events_5m,
            "events_1h": events_1h,
            "identities_1h": identities_1h,
            "dropped_total": metric_sum(&rendered, "zanshin_events_dropped_total") as u64,
            "rate_limited_total": metric_sum(&rendered, "zanshin_rate_limited_total") as u64,
        },
        "streams": {
            "websocket": metric_sum(&rendered, "zanshin_ws_connections").max(0.0) as u64,
            "server_sdk": metric_sum(&rendered, "zanshin_server_stream_connections").max(0.0) as u64,
        },
        "jobs": jobs,
        "integrations": integrations,
        "webhooks": {
            "configured": hooks.len(),
            "enabled": hooks.iter().filter(|h| h.enabled).count(),
            "deliveries": deliveries,
        },
        "exports": exports,
        "reports": reports,
        "alerts": alerts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_sum_collapses_labels() {
        let r = "# TYPE x counter\nzanshin_events_dropped_total{reason=\"a\"} 2\nzanshin_events_dropped_total{reason=\"b\"} 3\nzanshin_ws_connections 4\n";
        assert_eq!(metric_sum(r, "zanshin_events_dropped_total"), 5.0);
        assert_eq!(metric_sum(r, "zanshin_ws_connections"), 4.0);
        assert_eq!(metric_sum(r, "missing"), 0.0);
    }

    #[test]
    fn grouped_issues_collapse_after_three() {
        let mut v = Vec::new();
        grouped(&mut v, "integration", vec![("a".into(), "x".into())]);
        assert_eq!(v, vec!["integration a failed: x"]);
        v.clear();
        let many = (0..5).map(|i| (format!("k{i}"), "e".to_string())).collect();
        grouped(&mut v, "integration", many);
        assert_eq!(
            v,
            vec!["5 integrations failed their last delivery (k0, k1, k2, and 2 more)"]
        );
        assert_eq!(short(&"x".repeat(200)).chars().count(), 161);
    }

    #[test]
    fn registry_tracks_runs() {
        let reg = JobRegistry::default();
        reg.started("purge");
        reg.finished("purge", Duration::from_millis(12));
        reg.skipped("purge");
        reg.failed("sweep", "lock".into());
        let s = reg.snapshot();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "purge");
        assert_eq!((s[0].runs, s[0].skipped, s[0].last_ms), (1, 1, 12));
        assert_eq!(s[1].last_error.as_deref(), Some("lock"));
    }
}
