//! Analytics v2: game-metrics overview, retention cohorts, funnels, custom dashboards, annotations.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{
    analytics::{fold_funnel, Annotation, Dashboard, FunnelStep, Widget, WidgetKind},
    flags::validate_key,
    Error, Result,
};
use zanshin_store::analytics::{AnnotationRepo, DashboardRepo, PlayerMetricsRepo};

use super::{
    bus::ChangeBus, events::EventService, flags::normalize_tags, taxonomy::TaxonomyService,
};

const FUNNEL_ROW_CAP: i64 = 200_000;
const COHORT_CAP: i64 = 50_000;
const DEFAULT_REVENUE_EVENTS: &[&str] = &["purchase", "revenue", "iap"];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DashboardPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub widgets: Option<Vec<Widget>>,
    #[serde(default)]
    pub filters: Option<Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FunnelQuery {
    pub steps: Vec<FunnelStep>,
    #[serde(default)]
    pub window_secs: Option<i64>,
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct AnalyticsService {
    events: EventService,
    players: PlayerMetricsRepo,
    dashboards: DashboardRepo,
    annotations: AnnotationRepo,
    taxonomy: TaxonomyService,
}

pub fn window(
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    default_days: i64,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let to = to.unwrap_or_else(Utc::now);
    let from = from.unwrap_or(to - Duration::days(default_days));
    if from >= to {
        return Err(Error::invalid("from must be before to"));
    }
    if to - from > Duration::days(400) {
        return Err(Error::invalid("range must be at most 400 days"));
    }
    Ok((from, to))
}

fn day_of(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d").to_string()
}

impl AnalyticsService {
    pub fn new(
        events: EventService,
        players: PlayerMetricsRepo,
        dashboards: DashboardRepo,
        annotations: AnnotationRepo,
        taxonomy: TaxonomyService,
        bus: ChangeBus,
    ) -> Self {
        let svc = Self {
            events,
            players,
            dashboards,
            annotations,
            taxonomy,
        };
        let repo = svc.annotations.clone();
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(c) if c.identity_id.is_none() => {
                        let kind = format!("{:?}", c.kind).to_lowercase();
                        let kind = if kind == "liveevent" {
                            "live_event".to_string()
                        } else {
                            kind
                        };
                        let title = format!(
                            "{} {} {}",
                            kind.replace('_', " "),
                            c.key,
                            if c.deleted { "deleted" } else { "updated" }
                        );
                        let _ = repo.insert(Utc::now(), &kind, &c.key, &title, "").await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        svc
    }

    /// Event names that carry revenue: taxonomy definitions typed `revenue` plus the conventional names.
    pub async fn revenue_names(&self) -> Vec<String> {
        let mut names: Vec<String> = DEFAULT_REVENUE_EVENTS
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Ok(defs) = self.taxonomy.definitions().await {
            for d in defs.values().filter(|d| d.value_type == "revenue") {
                if !names.contains(&d.name) {
                    names.push(d.name.clone());
                }
            }
        }
        names
    }

    pub async fn overview(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Value> {
        let revenue_names = self.revenue_names().await;
        let (active, new_users, sessions, revenue, wau, mau, total) = tokio::try_join!(
            self.events.active_per_day(from, to),
            self.players.new_identities(from, to),
            self.players.sessions(from, to),
            self.events.revenue_per_day(&revenue_names, from, to),
            self.events.active_in_range(to - Duration::days(7), to),
            self.events.active_in_range(to - Duration::days(30), to),
            self.players.total_identities(),
        )?;
        let mut days: BTreeMap<String, Value> = BTreeMap::new();
        let mut d = from.date_naive();
        while d <= to.date_naive() {
            let day = d.format("%Y-%m-%d").to_string();
            days.insert(day.clone(), json!({"day": day, "dau": 0, "new_users": 0, "sessions": 0, "avg_session_secs": 0.0, "revenue": 0.0, "payers": 0, "purchases": 0}));
            d += Duration::days(1);
        }
        for (day, n) in &active {
            if let Some(v) = days.get_mut(day) {
                v["dau"] = json!(n);
            }
        }
        for r in &new_users {
            if let Some(v) = days.get_mut(&r.day) {
                v["new_users"] = json!(r.count);
            }
        }
        for r in &sessions {
            if let Some(v) = days.get_mut(&r.day) {
                v["sessions"] = json!(r.sessions);
                v["avg_session_secs"] = json!(r.avg_secs);
            }
        }
        for (day, amount, payers, purchases) in &revenue {
            if let Some(v) = days.get_mut(day) {
                v["revenue"] = json!(amount);
                v["payers"] = json!(payers);
                v["purchases"] = json!(purchases);
            }
        }
        let series: Vec<Value> = days.into_values().collect();
        let active_days: i64 = active.iter().map(|(_, n)| n).sum();
        let dau_latest = active.last().map(|(_, n)| *n).unwrap_or(0);
        let dau_avg = if active.is_empty() {
            0.0
        } else {
            active_days as f64 / active.len() as f64
        };
        let new_total: i64 = new_users.iter().map(|r| r.count).sum();
        let sessions_total: i64 = sessions.iter().map(|r| r.sessions).sum();
        let avg_session = if sessions_total > 0 {
            sessions
                .iter()
                .map(|r| r.avg_secs * r.sessions as f64)
                .sum::<f64>()
                / sessions_total as f64
        } else {
            0.0
        };
        let revenue_total: f64 = revenue.iter().map(|r| r.1).sum();
        let payer_days: i64 = revenue.iter().map(|r| r.2).sum();
        let purchases: i64 = revenue.iter().map(|r| r.3).sum();
        let ratio = |a: f64, b: f64| if b > 0.0 { a / b } else { 0.0 };
        Ok(json!({
            "from": from, "to": to, "backend": self.events.backend(),
            "revenue_events": revenue_names,
            "totals": {
                "dau": dau_latest, "dau_avg": dau_avg, "wau": wau, "mau": mau, "total_players": total,
                "new_users": new_total, "sessions": sessions_total, "avg_session_secs": avg_session,
                "revenue": revenue_total, "purchases": purchases, "paying_player_days": payer_days,
                "arpdau": ratio(revenue_total, active_days as f64),
                "arppu": ratio(revenue_total, payer_days as f64),
                "paying_share": ratio(payer_days as f64, active_days as f64),
                "stickiness": ratio(dau_avg, mau as f64),
            },
            "series": series,
        }))
    }

    pub async fn retention(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        days: &[i64],
    ) -> Result<Value> {
        let days: Vec<i64> = {
            let mut d: Vec<i64> = days
                .iter()
                .copied()
                .filter(|d| (1..=90).contains(d))
                .collect();
            d.sort_unstable();
            d.dedup();
            if d.is_empty() {
                vec![1, 3, 7, 14, 30]
            } else {
                d
            }
        };
        let cohorts = self.players.cohorts(from, to, COHORT_CAP).await?;
        if cohorts.is_empty() {
            return Ok(json!({ "from": from, "to": to, "days": days, "cohorts": [] }));
        }
        let ids: Vec<Uuid> = cohorts.iter().map(|(id, _)| *id).collect();
        let horizon = to + Duration::days(*days.last().unwrap_or(&30) + 1);
        let active = self.events.active_days(&ids, from, horizon).await?;
        let mut active_set: HashSet<(Uuid, String)> = HashSet::with_capacity(active.len());
        for (id, day) in active {
            active_set.insert((id, day));
        }
        let mut by_cohort: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
        for (id, day) in cohorts {
            by_cohort.entry(day).or_default().push(id);
        }
        let now_day = day_of(Utc::now());
        let out: Vec<Value> = by_cohort
            .into_iter()
            .map(|(cohort_day, ids)| {
                let base = chrono::NaiveDate::parse_from_str(&cohort_day, "%Y-%m-%d").ok();
                let mut retained = serde_json::Map::new();
                for d in &days {
                    let Some(base) = base else { continue };
                    let target = (base + Duration::days(*d)).format("%Y-%m-%d").to_string();
                    if target > now_day {
                        retained.insert(format!("d{d}"), Value::Null);
                        continue;
                    }
                    let n = ids
                        .iter()
                        .filter(|id| active_set.contains(&(**id, target.clone())))
                        .count();
                    retained.insert(
                        format!("d{d}"),
                        json!({ "count": n, "rate": n as f64 / ids.len() as f64 }),
                    );
                }
                json!({ "cohort": cohort_day, "size": ids.len(), "retained": retained })
            })
            .collect();
        Ok(json!({ "from": from, "to": to, "days": days, "cohorts": out }))
    }

    pub async fn funnel(&self, q: &FunnelQuery) -> Result<Value> {
        if q.steps.is_empty() || q.steps.len() > 20 {
            return Err(Error::invalid("steps must be 1..=20"));
        }
        let (from, to) = window(q.from, q.to, 30)?;
        let window_secs = q.window_secs.unwrap_or(7 * 86_400).clamp(1, 400 * 86_400);
        let mut names: Vec<String> = q.steps.iter().map(|s| s.name.clone()).collect();
        names.sort();
        names.dedup();
        let rows = self
            .events
            .funnel_rows(&names, from, to, FUNNEL_ROW_CAP)
            .await?;
        let truncated = rows.len() as i64 >= FUNNEL_ROW_CAP;
        let mut folded = Vec::with_capacity(rows.len());
        for (id, name, at, meta) in &rows {
            for (i, step) in q.steps.iter().enumerate() {
                if step.name == *name && step.matches(meta) {
                    folded.push((*id, i, *at));
                }
            }
        }
        let counts = fold_funnel(folded, q.steps.len(), window_secs);
        let first = counts.first().copied().unwrap_or(0);
        let steps: Vec<Value> = q
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let n = counts[i];
                let prev = if i == 0 { n } else { counts[i - 1] };
                json!({
                    "name": s.name, "field": s.field, "op": s.op, "value": s.value, "count": n,
                    "conversion": if first > 0 { n as f64 / first as f64 } else { 0.0 },
                    "step_conversion": if prev > 0 { n as f64 / prev as f64 } else { 0.0 },
                    "drop": prev - n,
                })
            })
            .collect();
        Ok(
            json!({ "from": from, "to": to, "window_secs": window_secs, "entered": first, "steps": steps, "truncated": truncated }),
        )
    }

    // ---- annotations

    pub async fn annotations(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<Annotation>> {
        self.annotations.list(from, to, 500).await
    }

    pub async fn annotate(
        &self,
        at: Option<DateTime<Utc>>,
        title: &str,
        body: &str,
    ) -> Result<Annotation> {
        if title.trim().is_empty() || title.len() > 200 {
            return Err(Error::invalid("title must be 1..200 chars"));
        }
        self.annotations
            .insert(at.unwrap_or_else(Utc::now), "note", "", title, body)
            .await
    }

    pub async fn delete_annotation(&self, id: Uuid) -> Result<()> {
        self.annotations.delete(id).await
    }

    pub async fn purge_annotations(&self, keep: Duration) -> Result<u64> {
        self.annotations.purge(Utc::now() - keep).await
    }

    // ---- dashboards

    pub async fn list_dashboards(&self) -> Result<Vec<Dashboard>> {
        self.dashboards.list().await
    }

    pub async fn get_dashboard(&self, key: &str) -> Result<Dashboard> {
        self.dashboards.get(key).await
    }

    pub async fn create_dashboard(&self, key: &str, p: DashboardPatch) -> Result<Dashboard> {
        validate_key(key)?;
        let now = Utc::now();
        let d = Dashboard {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            widgets: p.widgets.unwrap_or_default(),
            filters: p.filters.unwrap_or_else(|| json!({})),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        d.validate()?;
        self.dashboards.create(&d).await
    }

    pub async fn update_dashboard(&self, key: &str, p: DashboardPatch) -> Result<Dashboard> {
        let mut d = self.dashboards.get(key).await?;
        if let Some(v) = p.name {
            d.name = v;
        }
        if let Some(v) = p.description {
            d.description = v;
        }
        if let Some(v) = p.widgets {
            d.widgets = v;
        }
        if let Some(v) = p.filters {
            d.filters = v;
        }
        if let Some(v) = p.tags {
            d.tags = normalize_tags(v)?;
        }
        d.validate()?;
        self.dashboards.update(&d).await
    }

    pub async fn delete_dashboard(&self, key: &str) -> Result<()> {
        self.dashboards.delete(key).await
    }

    /// Evaluate every widget of a dashboard for the window.
    pub async fn dashboard_data(
        &self,
        key: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Value> {
        let d = self.dashboards.get(key).await?;
        let mut overview: Option<Value> = None;
        let mut out: Vec<Value> = Vec::with_capacity(d.widgets.len());
        for w in &d.widgets {
            let data = match w.kind {
                WidgetKind::Timeseries => {
                    let event = w.config["event"].as_str().unwrap_or_default();
                    self.events
                        .timeseries(event, from, to, zanshin_store::events::Granularity::Day)
                        .await
                        .map(|b| json!(b))
                }
                WidgetKind::Number => {
                    let metric = w.config["metric"].as_str().unwrap_or_default();
                    if metric == "event_count" {
                        let event = w.config["event"].as_str().unwrap_or_default();
                        self.events.timeseries(event, from, to, zanshin_store::events::Granularity::Day).await.map(|b| {
                            json!({ "value": b.iter().map(|x| x.count).sum::<i64>(), "identities": b.iter().map(|x| x.identities).sum::<i64>() })
                        })
                    } else {
                        if overview.is_none() {
                            overview = Some(self.overview(from, to).await?);
                        }
                        let totals = &overview.as_ref().unwrap()["totals"];
                        Ok(json!({ "value": totals.get(metric).cloned().unwrap_or(Value::Null) }))
                    }
                }
                WidgetKind::Funnel => {
                    let steps: Vec<FunnelStep> =
                        serde_json::from_value(w.config["steps"].clone()).unwrap_or_default();
                    let q = FunnelQuery {
                        steps,
                        window_secs: w.config["window_secs"].as_i64(),
                        from: Some(from),
                        to: Some(to),
                    };
                    self.funnel(&q).await
                }
                WidgetKind::Retention => {
                    let days: Vec<i64> = w.config["days"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                        .unwrap_or_default();
                    self.retention(from, to, &days).await
                }
                WidgetKind::TopEvents => {
                    let limit = w.config["limit"].as_i64().unwrap_or(10);
                    self.events
                        .top_names(from, to, limit)
                        .await
                        .map(|n| json!(n))
                }
            };
            out.push(match data {
                Ok(v) => json!({ "id": w.id, "kind": w.kind, "title": w.title, "width": w.width, "data": v }),
                Err(e) => json!({ "id": w.id, "kind": w.kind, "title": w.title, "width": w.width, "error": e.to_string() }),
            });
        }
        Ok(json!({ "dashboard": d.key, "from": from, "to": to, "widgets": out }))
    }
}
