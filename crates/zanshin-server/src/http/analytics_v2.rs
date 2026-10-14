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
use crate::{
    services::analytics::{window, DashboardPatch, FunnelQuery},
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
}

#[derive(Debug, Default, Deserialize)]
struct Range {
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    days: Option<String>,
}

async fn overview(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    Ok(Json(state.analytics.overview(from, to).await?))
}

async fn retention(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Range>,
) -> ApiResult<Json<Value>> {
    let (from, to) = window(q.from, q.to, 30)?;
    let days: Vec<i64> = q
        .days
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    Ok(Json(state.analytics.retention(from, to, &days).await?))
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
