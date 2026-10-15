//! Admin API: SQL explorer, saved queries, scheduled reports, metric alerts, path analysis,
//! funnel / SQL cohorts and the player timeline.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{auth::AdminAuth, error::ApiResult};
use crate::{
    services::analytics_v5::{
        AlertPatch, FunnelCohortRequest, PathsQuery, ReportPatch, SavedQueryPatch,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sql", get(sql_status).post(sql))
        .route("/sql/cohort", post(sql_cohort))
        .route("/analytics/queries", get(list_queries).post(create_query))
        .route(
            "/analytics/queries/{key}",
            get(get_query).put(update_query).delete(delete_query),
        )
        .route("/analytics/queries/{key}/run", post(run_query))
        .route("/analytics/reports", get(list_reports).post(create_report))
        .route(
            "/analytics/reports/{key}",
            get(get_report).put(update_report).delete(delete_report),
        )
        .route("/analytics/reports/{key}/run", post(run_report))
        .route("/analytics/alerts", get(list_alerts).post(create_alert))
        .route(
            "/analytics/alerts/{key}",
            get(get_alert).put(update_alert).delete(delete_alert),
        )
        .route("/analytics/alerts/{key}/check", post(check_alert))
        .route("/analytics/alerts/{key}/fires", get(alert_fires))
        .route("/analytics/paths", post(paths))
        .route("/analytics/funnel/cohort", post(funnel_cohort))
        .route("/identities/{id}/timeline", get(timeline))
}

#[derive(Deserialize)]
struct SqlBody {
    sql: String,
    #[serde(default)]
    limit: Option<i64>,
}

async fn sql_status(State(state): State<AppState>, AdminAuth(_): AdminAuth) -> Json<Value> {
    Json(
        json!({ "available": state.analytics_v5.sql_available(), "max_rows": zanshin_core::analytics_v5::MAX_SQL_ROWS }),
    )
}

async fn sql(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<SqlBody>,
) -> ApiResult<Json<Value>> {
    let r = state.analytics_v5.run_sql(&b.sql, b.limit).await?;
    Ok(Json(serde_json::to_value(r).unwrap_or(Value::Null)))
}

#[derive(Deserialize)]
struct SqlCohortBody {
    sql: String,
    audience_key: String,
    #[serde(default)]
    name: Option<String>,
}

async fn sql_cohort(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<SqlCohortBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        state
            .analytics_v5
            .sql_cohort(&b.sql, &b.audience_key, b.name)
            .await?,
    ))
}

#[derive(Deserialize)]
struct Keyed<T> {
    key: String,
    #[serde(flatten)]
    rest: T,
}

#[derive(Deserialize)]
struct Limit {
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_queries(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "items": state.analytics_v5.queries.list().await? }),
    ))
}
async fn create_query(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Keyed<SavedQueryPatch>>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let q = state.analytics_v5.create_query(&b.key, b.rest).await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!(q))))
}
async fn get_query(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.analytics_v5.queries.get(&key).await?)))
}
async fn update_query(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<SavedQueryPatch>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.analytics_v5.update_query(&key, p).await?)))
}
async fn delete_query(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    state.analytics_v5.queries.delete(&key).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
async fn run_query(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(l): Query<Limit>,
) -> ApiResult<Json<Value>> {
    let r = state.analytics_v5.run_query(&key, l.limit).await?;
    Ok(Json(serde_json::to_value(r).unwrap_or(Value::Null)))
}

async fn list_reports(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "items": state.analytics_v5.reports.list().await? }),
    ))
}
async fn create_report(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Keyed<ReportPatch>>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let r = state.analytics_v5.create_report(&b.key, b.rest).await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!(r))))
}
async fn get_report(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.analytics_v5.reports.get(&key).await?)))
}
async fn update_report(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<ReportPatch>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(
        state.analytics_v5.update_report(&key, p).await?
    )))
}
async fn delete_report(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    state.analytics_v5.reports.delete(&key).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
async fn run_report(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.analytics_v5.run_report(&key).await?))
}

async fn list_alerts(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "items": state.analytics_v5.alerts.list().await? }),
    ))
}
async fn create_alert(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Keyed<AlertPatch>>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let a = state.analytics_v5.create_alert(&b.key, b.rest).await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!(a))))
}
async fn get_alert(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.analytics_v5.alerts.get(&key).await?)))
}
async fn update_alert(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<AlertPatch>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.analytics_v5.update_alert(&key, p).await?)))
}
async fn delete_alert(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    state.analytics_v5.alerts.delete(&key).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Force {
    #[serde(default)]
    force: bool,
}
async fn check_alert(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(f): Query<Force>,
) -> ApiResult<Json<Value>> {
    let a = state.analytics_v5.alerts.get(&key).await?;
    Ok(Json(state.analytics_v5.check_alert(&a, f.force).await?))
}
async fn alert_fires(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(l): Query<Limit>,
) -> ApiResult<Json<Value>> {
    let a = state.analytics_v5.alerts.get(&key).await?;
    Ok(Json(
        json!({ "items": state.analytics_v5.alerts.fires(a.id, l.limit.unwrap_or(50)).await? }),
    ))
}

async fn paths(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(q): Json<PathsQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.analytics_v5.paths(&q).await?))
}
async fn funnel_cohort(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(q): Json<FunnelCohortRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.analytics_v5.funnel_cohort(&q).await?))
}
async fn timeline(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
    Query(l): Query<Limit>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        state
            .analytics_v5
            .timeline(id, l.limit.unwrap_or(200))
            .await?,
    ))
}
