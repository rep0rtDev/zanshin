//! Admin API: outbound webhook subscriptions, test sends, delivery log and retries.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::webhooks::{Webhook, EVENT_TYPES};

use super::{auth::AdminAuth, error::ApiResult};
use crate::{services::webhooks::WebhookPatch, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list).post(create))
        .route("/webhooks/event-types", get(event_types))
        .route("/webhooks/{key}", get(get_one).put(update).delete(delete))
        .route("/webhooks/{key}/test", post(test))
        .route("/webhooks/{key}/deliveries", get(deliveries))
        .route("/webhooks/{key}/deliveries/{id}/retry", post(retry))
}

async fn list(State(state): State<AppState>, AdminAuth(_): AdminAuth) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.webhooks.list().await? })))
}

async fn event_types(AdminAuth(_): AdminAuth) -> Json<Value> {
    Json(json!({ "items": EVENT_TYPES }))
}

#[derive(Debug, Deserialize)]
struct Create {
    key: String,
    #[serde(flatten)]
    patch: WebhookPatch,
}

async fn create(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Create>,
) -> ApiResult<(axum::http::StatusCode, Json<Webhook>)> {
    let w = state.webhooks.create(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(w)))
}

async fn get_one(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Webhook>> {
    Ok(Json(state.webhooks.get(&key).await?))
}

async fn update(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<WebhookPatch>,
) -> ApiResult<Json<Webhook>> {
    Ok(Json(state.webhooks.update(&key, p).await?))
}

async fn delete(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    state.webhooks.delete(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

async fn test(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.webhooks.test(&key).await?)))
}

#[derive(Debug, Default, Deserialize)]
struct DeliveriesQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    after: Option<Uuid>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn deliveries(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<DeliveriesQuery>,
) -> ApiResult<Json<Value>> {
    let (items, stats) = state
        .webhooks
        .deliveries(&key, q.status.as_deref(), q.after, q.limit.unwrap_or(100))
        .await?;
    let next = items.last().map(|d| d.id);
    let stats: Vec<Value> = stats
        .into_iter()
        .map(|(status, count)| json!({ "status": status, "count": count }))
        .collect();
    Ok(Json(
        json!({ "items": items, "next": next, "stats": stats }),
    ))
}

async fn retry(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.webhooks.retry(&key, id).await?)))
}
