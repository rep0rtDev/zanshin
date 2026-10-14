//! Admin API: game-metrics overview, retention, funnels, annotations, custom dashboards.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::analytics::Dashboard;

use super::{auth::AdminAuth, error::ApiResult};
use zanshin_core::analytics::EventFilter;
use zanshin_store::analytics::AdSpendRow;

use crate::{
    services::analytics::{window, AnalyticsService, DashboardPatch, ExploreQuery, FunnelQuery},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/analytics/overview", get(overview))
        .route("/analytics/retention", get(retention))
        .route("/analytics/funnel", post(funnel))
        .route("/analytics/annotations", get(annotations).post(annotate))
        .route(
            "/analytics/annotations/{id}",
            axum::routing::delete(delete_annotation),
        )
        .route("/dashboards", get(list_dashboards).post(create_dashboard))
        .route(
            "/dashboards/{key}",
            get(get_dashboard)
                .put(update_dashboard)
                .delete(delete_dashboard),
        )
        .route("/dashboards/{key}/data", get(dashboard_data))
        .route("/analytics/explore", post(explore))
        .route("/analytics/map", get(map))
        .route("/analytics/playtime", get(playtime))
        .route("/analytics/ltv", get(ltv))
        .route(
            "/analytics/ad-spend",
            get(ad_spend).post(import_ad_spend).delete(delete_ad_spend),
        )
        .route("/analytics/errors", get(errors))
        .route("/analytics/status", get(status))
        .route("/analytics/calendar", get(calendar))
}

#[derive(Debug, Default, Deserialize)]
struct Range {
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    days: Option<String>,
    /// Retention: count players returning on or before day N instead of exactly on day N.
    #[serde(default)]
    cumulative: Option<bool>,
    #[serde(flatten)]
    filter: EventFilter,
}

fn parse_days(days: &Option<String>) -> Vec<i64> {
    days.as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

async fn overview(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    Ok(Json(state.analytics.overview(from, to, &q.filter).await?))
}

async fn retention(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    let days = parse_days(&q.days);
    Ok(Json(
        state
            .analytics
            .retention(from, to, &days, &q.filter, q.cumulative.unwrap_or(false))
            .await?,
    ))
}

async fn explore(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(q): Json<ExploreQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.analytics.explore(&q).await?))
}

async fn map(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    Ok(Json(state.analytics.map(from, to, &q.filter).await?))
}

async fn playtime(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    Ok(Json(state.analytics.playtime(from, to, &q.filter).await?))
}

async fn ltv(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    let days = parse_days(&q.days);
    Ok(Json(state.analytics.ltv(from, to, &days, &q.filter).await?))
}

async fn ad_spend(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 90)?;
    Ok(Json(
        json!({ "from": from, "to": to, "items": state.analytics.ad_spend(from, to).await? }),
    ))
}

/// JSON `{"rows": [...]}` or `{"csv": "day,platform,country,campaign,spend,installs"}`.
#[derive(Debug, Deserialize)]
struct AdSpendImport {
    #[serde(default)]
    rows: Vec<AdSpendRow>,
    #[serde(default)]
    csv: Option<String>,
}

async fn import_ad_spend(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(body): Json<AdSpendImport>,
) -> ApiResult<Json<Value>> {
    let mut rows = body.rows;
    if let Some(csv) = body.csv.as_deref() {
        rows.extend(AnalyticsService::parse_ad_spend_csv(csv)?);
    }
    let n = state.analytics.import_ad_spend(rows).await?;
    Ok(Json(json!({ "upserted": n })))
}

async fn delete_ad_spend(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 90)?;
    Ok(Json(
        json!({ "deleted": state.analytics.delete_ad_spend(from, to).await? }),
    ))
}

async fn errors(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let since = q
        .from
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::hours(24));
    Ok(Json(state.analytics.errors(since).await?))
}

/// LiveOps status board: who is online, what is running, what is next, what is failing.
async fn status(State(state): State<AppState>, AdminAuth(_): AdminAuth) -> ApiResult<Json<Value>> {
    let now = chrono::Utc::now();
    let today = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|d| d.and_utc())
        .unwrap_or(now);
    let filter = EventFilter::default();
    let (online, (events_hour, active_hour), dau, failures) = tokio::try_join!(
        state.analytics.players().online_now(300),
        state
            .analytics
            .event_service()
            .count_since(now - chrono::Duration::hours(1)),
        state
            .analytics
            .event_service()
            .active_in_range(today, now, &filter),
        state
            .analytics
            .players()
            .failures_since(now - chrono::Duration::hours(24)),
    )?;
    let rejected = state
        .taxonomy
        .rejected(500)
        .await
        .map(|v| v.len())
        .unwrap_or(0);

    let experiments: Vec<Value> = state
        .experiments
        .list()
        .await
        .iter()
        .filter(|e| matches!(e.status, zanshin_core::experiments::Status::Running | zanshin_core::experiments::Status::Paused))
        .map(|e| json!({ "key": e.key, "name": e.name, "status": e.status, "starts_at": e.starts_at, "ends_at": e.ends_at, "enrol_until": e.enrol_until }))
        .collect();
    let mut live_now: Vec<Value> = Vec::new();
    let mut live_next: Vec<Value> = Vec::new();
    for ev in state.live_events.list().await.iter().filter(|e| e.enabled) {
        if let Some(o) = ev.current(now) {
            live_now
                .push(json!({ "key": ev.key, "name": ev.name, "start": o.start, "end": o.end }));
        } else if let Some(o) = ev.next(now) {
            if o.start < now + chrono::Duration::days(7) {
                live_next.push(
                    json!({ "key": ev.key, "name": ev.name, "start": o.start, "end": o.end }),
                );
            }
        }
    }
    live_next.sort_by(|a, b| a["start"].as_str().cmp(&b["start"].as_str()));
    let campaigns: Vec<Value> = state
        .messages
        .list()
        .await
        .iter()
        .filter(|m| m.enabled)
        .map(|m| json!({ "key": m.key, "name": m.name, "starts_at": m.starts_at, "ends_at": m.ends_at, "schedule": m.schedule }))
        .collect();
    let journeys: Vec<Value> = state
        .journeys
        .list()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|j| j.enabled)
        .map(|j| json!({ "key": j.key, "name": j.name }))
        .collect();
    Ok(Json(json!({
        "now": now,
        "online_now": online,
        "dau_today": dau,
        "events_last_hour": events_hour,
        "active_last_hour": active_hour,
        "running": { "experiments": experiments, "live_events": live_now, "campaigns": campaigns, "journeys": journeys },
        "upcoming": { "live_events": live_next },
        "errors_24h": { "rejected_events": rejected, "failures": failures.iter().map(|(k, n)| json!({ "source": k, "count": n })).collect::<Vec<_>>() },
    })))
}

/// LiveOps calendar: everything with a time extent in `[from, to)` — live-event occurrences,
/// experiments, campaigns (incl. cron fire times), annotations — plus daily DAU / revenue to
/// overlay. Default range: this week ± 1 week.
async fn calendar(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let now = chrono::Utc::now();
    let (from, to) = window(
        q.from.or(Some(now - chrono::Duration::days(7))),
        q.to.or(Some(now + chrono::Duration::days(14))),
        21,
    )?;
    let mut items: Vec<Value> = Vec::new();
    for ev in state.live_events.list().await.iter() {
        let mut cursor = from;
        if let Some(o) = ev.current(from) {
            items.push(json!({ "kind": "live_event", "key": ev.key, "name": ev.name, "enabled": ev.enabled, "start": o.start, "end": o.end }));
            cursor = o.start;
        }
        let mut n = 0;
        while let Some(o) = ev.next(cursor) {
            if o.start >= to || n >= 200 {
                break;
            }
            items.push(json!({ "kind": "live_event", "key": ev.key, "name": ev.name, "enabled": ev.enabled, "start": o.start, "end": o.end }));
            cursor = o.start;
            n += 1;
        }
    }
    for e in state.experiments.list().await.iter() {
        if matches!(e.status, zanshin_core::experiments::Status::Draft) {
            continue;
        }
        let start = e.starts_at.unwrap_or(e.created_at);
        let end = e.ends_at.or(e.enrol_until);
        let shown_end = end.unwrap_or(to);
        if start < to && shown_end >= from {
            items.push(json!({ "kind": "experiment", "key": e.key, "name": e.name, "status": e.status, "start": start, "end": end, "open_ended": end.is_none() }));
        }
    }
    for m in state.messages.list().await.iter() {
        match &m.schedule {
            zanshin_core::messages::Schedule::Cron { cron, window_secs } => {
                if let Ok(sched) = cron.parse::<cron::Schedule>() {
                    let w = window_secs.unwrap_or(zanshin_core::messages::DEFAULT_WINDOW_SECS);
                    for t in sched.after(&from).take_while(|t| *t < to).take(200) {
                        items.push(json!({ "kind": "campaign", "key": m.key, "name": m.name, "enabled": m.enabled, "start": t, "end": t + chrono::Duration::seconds(w) }));
                    }
                }
            }
            zanshin_core::messages::Schedule::Once => {
                let start = m.starts_at.unwrap_or(m.created_at);
                let end = m.ends_at;
                if start < to && end.unwrap_or(to) >= from {
                    items.push(json!({ "kind": "campaign", "key": m.key, "name": m.name, "enabled": m.enabled, "start": start, "end": end, "open_ended": end.is_none() }));
                }
            }
            zanshin_core::messages::Schedule::LiveEvent { .. } => {}
        }
    }
    for a in state.analytics.annotations(from, to).await? {
        items.push(json!({ "kind": "annotation", "key": a.kind, "name": a.title, "start": a.at, "end": a.at, "annotation": a }));
    }
    items.sort_by(|a, b| a["start"].as_str().cmp(&b["start"].as_str()));
    let overview = state
        .analytics
        .overview(from, to.min(now), &q.filter)
        .await
        .ok();
    Ok(Json(json!({
        "from": from, "to": to, "items": items,
        "metrics": overview.as_ref().map(|o| o["series"].clone()).unwrap_or(Value::Array(vec![])),
    })))
}

async fn funnel(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(q): Json<FunnelQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.analytics.funnel(&q).await?))
}

async fn annotations(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    Ok(Json(
        json!({ "items": state.analytics.annotations(from, to).await? }),
    ))
}

#[derive(Debug, Deserialize)]
struct Annotate {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn annotate(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Annotate>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let a = state.analytics.annotate(b.at, &b.title, &b.body).await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!(a))))
}

async fn delete_annotation(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    state.analytics.delete_annotation(id).await?;
    Ok(Json(json!({ "deleted": id })))
}

async fn list_dashboards(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "items": state.analytics.list_dashboards().await? }),
    ))
}

#[derive(Debug, Deserialize)]
struct CreateDashboard {
    key: String,
    #[serde(flatten)]
    patch: DashboardPatch,
}

async fn create_dashboard(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<CreateDashboard>,
) -> ApiResult<(axum::http::StatusCode, Json<Dashboard>)> {
    let d = state.analytics.create_dashboard(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(d)))
}

async fn get_dashboard(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Dashboard>> {
    Ok(Json(state.analytics.get_dashboard(&key).await?))
}

async fn update_dashboard(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<DashboardPatch>,
) -> ApiResult<Json<Dashboard>> {
    Ok(Json(state.analytics.update_dashboard(&key, p).await?))
}

async fn delete_dashboard(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    state.analytics.delete_dashboard(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

async fn dashboard_data(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    Ok(Json(state.analytics.dashboard_data(&key, from, to).await?))
}
