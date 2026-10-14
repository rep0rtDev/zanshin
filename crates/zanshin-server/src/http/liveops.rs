//! Admin API for flags v2 / live events v2: explain chain, participants, timeline, QA time travel.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{auth::AdminAuth, error::ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/flags/{key}/explain/{identity_id}", get(explain_flag))
        .route("/live-events/{key}/participants", get(participants))
        .route("/live-events/{key}/timeline", get(timeline))
        .route(
            "/identities/{identity_id}/qa-time",
            get(get_qa_time).put(set_qa_time).delete(clear_qa_time),
        )
}

async fn explain_flag(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    let identity = state.identity.get(identity_id).await?;
    Ok(Json(state.eval.explain_flag(&identity, &key).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    pub after: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn participants(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = state
        .live_events
        .participants(&key, q.after, q.limit.unwrap_or(100))
        .await?;
    let next = items.last().map(|p| p.identity_id);
    Ok(Json(serde_json::json!({ "items": items, "next": next })))
}

#[derive(Debug, Default, Deserialize)]
pub struct RangeQuery {
    #[serde(default)]
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

async fn timeline(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = chrono::Utc::now();
    let from = q.from.unwrap_or(now - chrono::Duration::days(30));
    let to = q.to.unwrap_or(now + chrono::Duration::days(60));
    Ok(Json(state.live_events.timeline(&key, from, to).await?))
}

async fn get_qa_time(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::json!({ "qa_time": state.eval.qa_time(identity_id).await }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct QaTimeBody {
    pub at: chrono::DateTime<chrono::Utc>,
}

/// Pin the evaluation clock for an identity so QA can see future live events / schedules.
async fn set_qa_time(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
    Json(b): Json<QaTimeBody>,
) -> ApiResult<Json<serde_json::Value>> {
    state.identity.get(identity_id).await?;
    state.eval.set_qa_time(identity_id, Some(b.at)).await?;
    Ok(Json(serde_json::json!({ "qa_time": b.at })))
}

async fn clear_qa_time(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    state.eval.set_qa_time(identity_id, None).await?;
    Ok(Json(serde_json::json!({ "qa_time": null })))
}
