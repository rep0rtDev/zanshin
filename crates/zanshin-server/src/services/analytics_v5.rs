//! Analytics v5: read-only SQL explorer (ClickHouse), saved queries, scheduled reports, metric
//! alerts, path analysis, cohorts → managed audiences and the unified player timeline.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{
    analytics_v5::{
        evaluate_alert, fold_paths, validate_readonly_sql, AlertAggregation, AlertCondition,
        MetricAlert, PathDirection, ReportEmail, ReportSchedule, SavedQuery, MAX_SQL_ROWS,
    },
    audiences::AudienceKind,
    notifications::Notice,
    Error, Result,
};
use zanshin_store::{
    analytics_v5::{AlertRepo, ReportRepo, SavedQueryRepo, TimelineRepo},
    clickhouse_sink::{ClickHouse, SqlResult},
};

use super::{
    analytics::{window, FunnelQuery},
    audiences::{AudiencePatch, AudienceService},
    events::EventService,
    integrations::IntegrationService,
    journeys::JourneyService,
    notifications::NotificationService,
};

const SQL_TIMEOUT_SECS: u64 = 30;
const PATH_ROW_CAP: i64 = 500_000;
const FUNNEL_ROW_CAP: i64 = 500_000;

#[derive(Clone)]
pub struct AnalyticsV5Service {
    events: EventService,
    ch: Option<ClickHouse>,
    pub queries: SavedQueryRepo,
    pub reports: ReportRepo,
    pub alerts: AlertRepo,
    timeline: TimelineRepo,
    audiences: AudienceService,
    journeys: JourneyService,
    notifications: NotificationService,
    integrations: IntegrationService,
}

#[derive(Debug, Default, Deserialize)]
pub struct SavedQueryPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sql: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ReportPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub query_key: Option<String>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub channels: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub email: Option<Option<ReportEmail>>,
    #[serde(default)]
    pub max_rows: Option<i64>,
}

fn double_option<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Option<ReportEmail>>, D::Error> {
    Option::<ReportEmail>::deserialize(d).map(Some)
}

#[derive(Debug, Default, Deserialize)]
pub struct AlertPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub aggregation: Option<AlertAggregation>,
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub condition: Option<AlertCondition>,
    #[serde(default)]
    pub window_secs: Option<i64>,
    #[serde(default)]
    pub cooldown_secs: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PathsQuery {
    pub event: String,
    #[serde(default)]
    pub direction: PathDirection,
    #[serde(default = "default_depth")]
    pub depth: usize,
    #[serde(default = "default_gap")]
    pub max_gap_secs: i64,
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    #[serde(default = "default_top")]
    pub top: usize,
}
fn default_depth() -> usize {
    3
}
fn default_gap() -> i64 {
    1800
}
fn default_top() -> usize {
    20
}

#[derive(Debug, Deserialize)]
pub struct FunnelCohortRequest {
    #[serde(flatten)]
    pub funnel: FunnelQuery,
    /// 1-based step index.
    pub step: usize,
    /// `true`: players who reached the step; `false`: players who reached the previous step but
    /// dropped before this one.
    #[serde(default = "default_true")]
    pub reached: bool,
    pub audience_key: String,
    #[serde(default)]
    pub name: Option<String>,
}
fn default_true() -> bool {
    true
}

impl AnalyticsV5Service {
    pub fn new(
        events: EventService,
        ch: Option<ClickHouse>,
        (queries, reports, alerts, timeline): (SavedQueryRepo, ReportRepo, AlertRepo, TimelineRepo),
        audiences: AudienceService,
        journeys: JourneyService,
        notifications: NotificationService,
        integrations: IntegrationService,
    ) -> Self {
        Self {
            events,
            ch,
            queries,
            reports,
            alerts,
            timeline,
            audiences,
            journeys,
            notifications,
            integrations,
        }
    }

    // --- SQL explorer -------------------------------------------------------------------------------

    pub fn sql_available(&self) -> bool {
        self.ch.is_some()
    }

    pub async fn run_sql(&self, sql: &str, limit: Option<i64>) -> Result<SqlResult> {
        let sql = validate_readonly_sql(sql)?;
        let ch = self.ch.as_ref().ok_or_else(|| {
            Error::invalid("the SQL explorer requires ClickHouse (clickhouse.enabled)")
        })?;
        let limit = limit.unwrap_or(1000).clamp(1, MAX_SQL_ROWS) as u64;
        let started = std::time::Instant::now();
        let r = ch.sql_readonly(&sql, limit, SQL_TIMEOUT_SECS).await;
        metrics::histogram!("zanshin_sql_explorer_seconds").record(started.elapsed().as_secs_f64());
        r
    }

    /// Materialises the `identity_id` column of a query result as a managed audience.
    pub async fn sql_cohort(
        &self,
        sql: &str,
        audience_key: &str,
        name: Option<String>,
    ) -> Result<Value> {
        let res = self.run_sql(sql, Some(MAX_SQL_ROWS)).await?;
        let col = res
            .columns
            .iter()
            .position(|c| c == "identity_id")
            .ok_or_else(|| Error::invalid("the query must return an `identity_id` column"))?;
        let ids: Vec<Uuid> = res
            .rows
            .iter()
            .filter_map(|r| {
                r.get(col)
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
            })
            .collect();
        self.materialise(audience_key, name, ids, "sql", res.truncated)
            .await
    }

    async fn materialise(
        &self,
        key: &str,
        name: Option<String>,
        mut ids: Vec<Uuid>,
        source: &str,
        truncated: bool,
    ) -> Result<Value> {
        ids.sort();
        ids.dedup();
        let audience = match self.audiences.repo.get(key).await {
            Ok(a) => {
                if a.kind != AudienceKind::Managed {
                    return Err(Error::invalid(format!(
                        "audience {key:?} exists and is not managed"
                    )));
                }
                a
            }
            Err(Error::NotFound(_)) => {
                let patch: AudiencePatch = serde_json::from_value(json!({
                    "name": name.unwrap_or_else(|| key.to_string()),
                    "description": format!("Cohort from {source}"),
                    "kind": "managed",
                    "tags": ["cohort", source],
                }))
                .map_err(|e| Error::invalid(e.to_string()))?;
                self.audiences.create(key, patch).await?
            }
            Err(e) => return Err(e),
        };
        let added = self
            .audiences
            .repo
            .add_members(audience.id, &ids, "managed")
            .await?;
        let total = self.audiences.repo.mark_computed(audience.id).await?;
        Ok(json!({
            "audience": audience.key, "matched": ids.len(), "added": added, "members": total,
            "truncated": truncated,
        }))
    }

    // --- saved queries ------------------------------------------------------------------------------

    pub async fn create_query(&self, key: &str, p: SavedQueryPatch) -> Result<SavedQuery> {
        let now = Utc::now();
        let q = SavedQuery {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            sql: p.sql.ok_or_else(|| Error::invalid("sql is required"))?,
            tags: p.tags.unwrap_or_default(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        q.validate()?;
        self.queries.create(&q).await
    }

    pub async fn update_query(&self, key: &str, p: SavedQueryPatch) -> Result<SavedQuery> {
        let mut q = self.queries.get(key).await?;
        if let Some(v) = p.name {
            q.name = v;
        }
        if let Some(v) = p.description {
            q.description = v;
        }
        if let Some(v) = p.sql {
            q.sql = v;
        }
        if let Some(v) = p.tags {
            q.tags = v;
        }
        q.validate()?;
        self.queries.update(&q).await
    }

    pub async fn run_query(&self, key: &str, limit: Option<i64>) -> Result<SqlResult> {
        let q = self.queries.get(key).await?;
        self.run_sql(&q.sql, limit).await
    }

    // --- reports ------------------------------------------------------------------------------------

    pub async fn create_report(&self, key: &str, p: ReportPatch) -> Result<ReportSchedule> {
        let now = Utc::now();
        let r = ReportSchedule {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            query_key: p
                .query_key
                .ok_or_else(|| Error::invalid("query_key is required"))?,
            cron: p.cron.ok_or_else(|| Error::invalid("cron is required"))?,
            timezone: p.timezone.unwrap_or_else(|| "UTC".into()),
            channels: p.channels.unwrap_or_default(),
            email: p.email.flatten(),
            max_rows: p.max_rows.unwrap_or(20),
            last_run_at: None,
            last_status: String::new(),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        r.validate()?;
        self.queries.get(&r.query_key).await?;
        self.reports.create(&r).await
    }

    pub async fn update_report(&self, key: &str, p: ReportPatch) -> Result<ReportSchedule> {
        let mut r = self.reports.get(key).await?;
        if let Some(v) = p.name {
            r.name = v;
        }
        if let Some(v) = p.enabled {
            r.enabled = v;
        }
        if let Some(v) = p.query_key {
            r.query_key = v;
        }
        if let Some(v) = p.cron {
            r.cron = v;
        }
        if let Some(v) = p.timezone {
            r.timezone = v;
        }
        if let Some(v) = p.channels {
            r.channels = v;
        }
        if let Some(v) = p.email {
            r.email = v;
        }
        if let Some(v) = p.max_rows {
            r.max_rows = v;
        }
        r.validate()?;
        self.queries.get(&r.query_key).await?;
        self.reports.update(&r).await
    }

    /// Runs the report now: query → text table → notification channels and/or email.
    pub async fn run_report(&self, key: &str) -> Result<Value> {
        let r = self.reports.get(key).await?;
        let now = Utc::now();
        let out = self.deliver_report(&r).await;
        let status = match &out {
            Ok(v) => v["status"].as_str().unwrap_or("ok").to_string(),
            Err(e) => format!("error: {e}"),
        };
        self.reports.mark_run(r.id, now, &status).await?;
        out
    }

    async fn deliver_report(&self, r: &ReportSchedule) -> Result<Value> {
        let q = self.queries.get(&r.query_key).await?;
        let res = self.run_sql(&q.sql, Some(r.max_rows)).await?;
        let title = if r.name.is_empty() {
            r.key.clone()
        } else {
            r.name.clone()
        };
        let (text, html) = render_table(&res, r.max_rows as usize);
        let mut n = Notice::new("report", title.clone(), text.clone());
        n.kind = Some(r.key.clone());
        n.fields = vec![
            ("query".into(), q.key.clone()),
            ("rows".into(), res.rows.len().to_string()),
        ];
        let channels = self.notifications.send_to(&r.channels, &n).await;
        let mut email = Vec::new();
        if let Some(e) = &r.email {
            email = self
                .integrations
                .email_raw(
                    &e.integration,
                    &e.to,
                    &format!("[Zanshin] {title}"),
                    &text,
                    &html,
                )
                .await?
                .into_iter()
                .map(|o| json!({ "to": o.channel, "status": o.status, "detail": o.detail }))
                .collect();
        }
        let failed =
            channels.iter().any(|(_, ok, _)| !ok) || email.iter().any(|e| e["status"] != "sent");
        Ok(json!({
            "status": if failed { "partial" } else { "ok" },
            "rows": res.rows.len(),
            "channels": channels.iter().map(|(k, ok, err)| json!({ "channel": k, "ok": ok, "error": err })).collect::<Vec<_>>(),
            "email": email,
        }))
    }

    /// Housekeeping: run every schedule whose cron slot passed since its last run.
    pub async fn run_due_reports(&self) {
        let now = Utc::now();
        let Ok(list) = self.reports.list().await else {
            return;
        };
        for r in list.into_iter().filter(|r| r.due(now)) {
            match self.run_report(&r.key).await {
                Ok(v) => tracing::info!(report = %r.key, status = %v["status"], "report delivered"),
                Err(e) => tracing::warn!(report = %r.key, error = %e, "report failed"),
            }
        }
    }

    // --- metric alerts ------------------------------------------------------------------------------

    pub async fn create_alert(&self, key: &str, p: AlertPatch) -> Result<MetricAlert> {
        let now = Utc::now();
        let a = MetricAlert {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            enabled: p.enabled.unwrap_or(true),
            event: p.event.ok_or_else(|| Error::invalid("event is required"))?,
            aggregation: p.aggregation.unwrap_or_default(),
            field: p.field.unwrap_or_default(),
            condition: p
                .condition
                .ok_or_else(|| Error::invalid("condition is required"))?,
            window_secs: p
                .window_secs
                .ok_or_else(|| Error::invalid("window_secs is required"))?,
            cooldown_secs: p.cooldown_secs.unwrap_or(3600),
            last_value: None,
            last_checked_at: None,
            last_fired_at: None,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        a.validate()?;
        self.alerts.create(&a).await
    }

    pub async fn update_alert(&self, key: &str, p: AlertPatch) -> Result<MetricAlert> {
        let mut a = self.alerts.get(key).await?;
        if let Some(v) = p.name {
            a.name = v;
        }
        if let Some(v) = p.enabled {
            a.enabled = v;
        }
        if let Some(v) = p.event {
            a.event = v;
        }
        if let Some(v) = p.aggregation {
            a.aggregation = v;
        }
        if let Some(v) = p.field {
            a.field = v;
        }
        if let Some(v) = p.condition {
            a.condition = v;
        }
        if let Some(v) = p.window_secs {
            a.window_secs = v;
        }
        if let Some(v) = p.cooldown_secs {
            a.cooldown_secs = v;
        }
        a.validate()?;
        self.alerts.update(&a).await
    }

    /// Evaluates one alert now; `force` ignores the cooldown. Returns the check result.
    pub async fn check_alert(&self, a: &MetricAlert, force: bool) -> Result<Value> {
        let now = Utc::now();
        let w = Duration::seconds(a.window_secs);
        let agg = match a.aggregation {
            AlertAggregation::Count => "count",
            AlertAggregation::Unique => "unique",
            AlertAggregation::Sum => "sum",
        };
        let store = self.events.store();
        let current = store
            .window_value(&a.event, agg, &a.field, now - w, now)
            .await?;
        let mut history = Vec::new();
        for k in (1..=a.baseline_windows() as i64).rev() {
            history.push(
                store
                    .window_value(
                        &a.event,
                        agg,
                        &a.field,
                        now - w * (k as i32 + 1),
                        now - w * (k as i32),
                    )
                    .await?,
            );
        }
        let fire = evaluate_alert(a, current, &history);
        let fired = fire.is_some() && (force || !a.in_cooldown(now));
        self.alerts.record_check(a.id, current, now, fired).await?;
        if let (true, Some(f)) = (fired, &fire) {
            self.alerts.record_fire(a.id, f, now).await?;
            let title = if a.name.is_empty() {
                a.key.clone()
            } else {
                a.name.clone()
            };
            let mut n = Notice::new("alert", format!("Metric alert: {title}"), f.text.clone());
            n.kind = Some("metric_alert".into());
            n.fields = vec![
                ("alert".into(), a.key.clone()),
                ("event".into(), a.event.clone()),
                ("window".into(), format!("{}s", a.window_secs)),
            ];
            self.notifications.notify(n);
            metrics::counter!("zanshin_metric_alerts_fired_total", "alert" => a.key.clone())
                .increment(1);
        }
        Ok(json!({
            "alert": a.key, "value": current, "history": history, "fired": fired,
            "suppressed": fire.is_some() && !fired, "fire": fire,
        }))
    }

    /// Housekeeping: evaluate every enabled alert.
    pub async fn check_alerts(&self) {
        let Ok(list) = self.alerts.list().await else {
            return;
        };
        for a in list.into_iter().filter(|a| a.enabled) {
            if let Err(e) = self.check_alert(&a, false).await {
                tracing::warn!(alert = %a.key, error = %e, "metric alert check failed");
            }
        }
    }

    // --- paths --------------------------------------------------------------------------------------

    pub async fn paths(&self, q: &PathsQuery) -> Result<Value> {
        if q.event.is_empty() {
            return Err(Error::invalid("event is required"));
        }
        let (from, to) = window(q.from, q.to, 7)?;
        let rows = self
            .events
            .store()
            .paths_rows(from, to, PATH_ROW_CAP)
            .await?;
        let truncated = rows.len() as i64 >= PATH_ROW_CAP;
        let r = fold_paths(
            rows,
            &q.event,
            q.direction,
            q.depth,
            q.max_gap_secs.clamp(1, 30 * 86_400),
            q.top.clamp(1, 100),
        );
        Ok(json!({
            "event": q.event, "direction": q.direction, "anchors": r.anchors, "truncated": truncated,
            "levels": r.levels.iter().enumerate().map(|(i, l)| json!({
                "depth": i + 1,
                "items": l.iter().map(|(n, c)| json!({ "name": n, "count": c, "share": if r.anchors > 0 { *c as f64 / r.anchors as f64 } else { 0.0 } })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "sequences": r.sequences.iter().map(|(s, c)| json!({ "steps": s, "count": c })).collect::<Vec<_>>(),
        }))
    }

    // --- funnel cohort ------------------------------------------------------------------------------

    pub async fn funnel_cohort(&self, req: &FunnelCohortRequest) -> Result<Value> {
        let q = &req.funnel;
        if q.steps.is_empty() || q.steps.len() > 20 {
            return Err(Error::invalid("steps must be 1..=20"));
        }
        if req.step == 0 || req.step > q.steps.len() {
            return Err(Error::invalid("step must be 1..=steps.len()"));
        }
        if !req.reached && req.step == 1 {
            return Err(Error::invalid("nobody can drop before the first step"));
        }
        let (from, to) = window(q.from, q.to, 30)?;
        let window_secs = q.window_secs.unwrap_or(7 * 86_400).clamp(1, 400 * 86_400);
        let mut names: Vec<String> = q.steps.iter().map(|s| s.name.clone()).collect();
        names.sort();
        names.dedup();
        q.filter.validate().map_err(Error::invalid)?;
        let rows = self
            .events
            .store()
            .funnel_rows(&names, from, to, FUNNEL_ROW_CAP, &q.filter)
            .await?;
        let truncated = rows.len() as i64 >= FUNNEL_ROW_CAP;
        let mut by_id: std::collections::HashMap<Uuid, Vec<(usize, DateTime<Utc>)>> =
            Default::default();
        for (id, name, at, meta, _) in &rows {
            for (i, step) in q.steps.iter().enumerate() {
                if step.name == *name && step.matches(meta) {
                    by_id.entry(*id).or_default().push((i, *at));
                }
            }
        }
        let mut ids = Vec::new();
        for (id, mut evs) in by_id {
            evs.sort_by_key(|(_, at)| *at);
            let mut reached = 0usize;
            let mut start: Option<DateTime<Utc>> = None;
            let mut last: Option<DateTime<Utc>> = None;
            for (i, at) in evs {
                if i == reached
                    && last.is_none_or(|l| at >= l)
                    && start.is_none_or(|s| (at - s).num_seconds() <= window_secs)
                {
                    if reached == 0 {
                        start = Some(at);
                    }
                    reached += 1;
                    last = Some(at);
                    if reached == q.steps.len() {
                        break;
                    }
                }
            }
            let hit = if req.reached {
                reached >= req.step
            } else {
                reached == req.step - 1
            };
            if hit {
                ids.push(id);
            }
        }
        self.materialise(
            &req.audience_key,
            req.name.clone(),
            ids,
            "funnel",
            truncated,
        )
        .await
    }

    // --- timeline -----------------------------------------------------------------------------------

    pub async fn timeline(&self, identity_id: Uuid, limit: i64) -> Result<Value> {
        let limit = limit.clamp(1, 1000);
        let mut items: Vec<(DateTime<Utc>, Value)> = Vec::new();
        for e in self.events.recent_for_identity(identity_id, limit).await? {
            items.push((
                e.event_time,
                json!({ "at": e.event_time, "kind": "event", "title": e.name, "detail": { "value": e.value, "metadata": e.metadata, "experiments": e.experiments } }),
            ));
        }
        for s in self
            .timeline
            .sessions_for_identity(identity_id, limit)
            .await?
        {
            items.push((
                s.started_at,
                json!({ "at": s.started_at, "kind": "session", "title": "session", "detail": { "ended_at": s.ended_at, "duration_sec": s.duration_sec, "platform": s.platform, "app_version": s.app_version } }),
            ));
        }
        for d in self
            .timeline
            .deliveries_for_identity(identity_id, limit)
            .await?
        {
            items.push((
                d.at,
                json!({ "at": d.at, "kind": "message", "title": d.message_key, "detail": { "channel": d.channel, "status": d.status, "detail": d.detail, "variant": d.variant } }),
            ));
        }
        for (key, run, log) in self.journeys.runs_for_identity(identity_id).await? {
            items.push((
                run.entered_at,
                json!({ "at": run.entered_at, "kind": "journey", "title": key, "detail": { "status": run.status, "step": run.step, "branch": run.branch, "converted_at": run.converted_at, "steps": log.len() } }),
            ));
            if let Some(c) = run.converted_at {
                items.push((c, json!({ "at": c, "kind": "journey", "title": format!("{key} · converted"), "detail": { "run": run.id } })));
            }
        }
        items.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        items.truncate(limit as usize);
        Ok(json!({ "items": items.into_iter().map(|(_, v)| v).collect::<Vec<_>>() }))
    }
}

/// Plain-text (for Slack / Discord / email body) and HTML renderings of the first `max_rows` rows.
fn render_table(res: &SqlResult, max_rows: usize) -> (String, String) {
    let rows: Vec<Vec<String>> = res
        .rows
        .iter()
        .take(max_rows)
        .map(|r| r.iter().map(cell).collect())
        .collect();
    let widths: Vec<usize> = res
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            rows.iter()
                .map(|r| r.get(i).map(|s| s.chars().count()).unwrap_or(0))
                .max()
                .unwrap_or(0)
                .max(c.chars().count())
                .min(40)
        })
        .collect();
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "{:<w$}",
                    c.chars().take(40).collect::<String>(),
                    w = widths[i]
                )
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    let mut text = String::from("```\n");
    text.push_str(&line(&res.columns));
    text.push('\n');
    for r in &rows {
        text.push_str(&line(r));
        text.push('\n');
    }
    if res.truncated || res.rows.len() > rows.len() {
        text.push_str("…\n");
    }
    text.push_str("```");
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut html = String::from("<table border=\"1\" cellpadding=\"4\" style=\"border-collapse:collapse;font-family:monospace\"><tr>");
    for c in &res.columns {
        html.push_str(&format!("<th>{}</th>", esc(c)));
    }
    html.push_str("</tr>");
    for r in &rows {
        html.push_str("<tr>");
        for c in r {
            html.push_str(&format!("<td>{}</td>", esc(c)));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    (text, html)
}

fn cell(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
