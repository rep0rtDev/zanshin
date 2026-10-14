//! Admin API: notification channels (Slack / Discord / webhook).

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use zanshin_core::notifications::{NotificationChannel, TOPICS};

use super::{auth::AdminAuth, error::ApiResult};
use crate::{services::notifications::ChannelPatch, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notification-channels", get(list).post(create))
        .route("/notification-channels/topics", get(topics))
        .route(
            "/notification-channels/{key}",
            get(get_one).put(update).delete(delete),
        )
        .route("/notification-channels/{key}/test", post(test))
}

async fn list(State(state): State<AppState>, AdminAuth(_): AdminAuth) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.notifications.list().await? })))
}

async fn topics(AdminAuth(_): AdminAuth) -> Json<Value> {
    Json(json!({ "items": TOPICS }))
}

#[derive(Debug, Deserialize)]
struct Create {
    key: String,
    #[serde(flatten)]
    patch: ChannelPatch,
}

async fn create(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Create>,
) -> ApiResult<(axum::http::StatusCode, Json<NotificationChannel>)> {
    let c = state.notifications.create(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(c)))
}

async fn get_one(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<NotificationChannel>> {
    Ok(Json(state.notifications.get(&key).await?))
}

async fn update(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<ChannelPatch>,
) -> ApiResult<Json<NotificationChannel>> {
    Ok(Json(state.notifications.update(&key, p).await?))
}

async fn delete(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    state.notifications.delete(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

async fn test(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.notifications.test(&key).await?))
}
