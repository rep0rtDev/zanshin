//! Client API, `/v1`.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use zanshin_core::{
    experiments::Assignment, flags::Evaluation, live_events::ActiveEvent, messages::InboxMessage,
    properties::PropertyMap, Identity, PropertySet,
};

use crate::state::AppState;

use super::{
    auth::{ClientAuth, SessionAuth},
    error::ApiResult,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/authenticate", post(authenticate))
        .route("/authenticate/refresh", post(refresh))
        .route("/authenticate/logout", post(logout))
        .route("/identity", get(get_identity).delete(delete_identity))
        .route("/properties", get(get_properties).put(update_properties))
        .route("/flags", get(get_flags))
        .route("/flags/{key}", get(get_flag))
        .route("/audiences", get(get_audiences))
        .route("/experiments", get(get_experiments))
        .route("/experiments/{key}/expose", post(expose_experiment))
        .route("/live-events", get(get_live_events))
        .route("/messages", get(get_messages))
        .route(
            "/messages/{id}",
            axum::routing::put(update_message).delete(delete_message),
        )
        .route("/stream", get(super::ws::stream))
        .route("/healthcheck", get(healthcheck))
}

#[derive(Debug, Deserialize)]
pub struct AuthenticateRequest {
    pub id: String,
    #[serde(default)]
    pub default: PropertyMap,
    #[serde(default)]
    pub custom: PropertyMap,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub token: String,
    pub refresh_token: String,
    pub identity: IdentityResponse,
    pub properties: PropertySet,
}

#[derive(Debug, Serialize)]
pub struct IdentityResponse {
    pub id: uuid::Uuid,
    pub external_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

impl From<&Identity> for IdentityResponse {
    fn from(i: &Identity) -> Self {
        Self {
            id: i.id,
            external_id: i.external_id.clone(),
            created_at: i.created_at,
            updated_at: i.updated_at,
            last_seen_at: i.last_seen_at,
        }
    }
}

fn session_response(s: crate::services::identity::SessionTokens) -> SessionResponse {
    SessionResponse {
        token: s.token,
        refresh_token: s.refresh_token,
        identity: IdentityResponse::from(&s.identity),
        properties: s.identity.properties,
    }
}

async fn authenticate(
    State(state): State<AppState>,
    ClientAuth(_principal): ClientAuth,
    Json(req): Json<AuthenticateRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let s = state
        .identity
        .authenticate(&req.id, req.default, req.custom)
        .await?;
    Ok(Json(session_response(s)))
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

async fn refresh(
    State(state): State<AppState>,
    ClientAuth(_): ClientAuth,
    Json(req): Json<RefreshRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let s = state.identity.refresh(&req.refresh_token).await?;
    Ok(Json(session_response(s)))
}

#[derive(Debug, Default, Deserialize)]
pub struct LogoutRequest {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

async fn logout(
    State(state): State<AppState>,
    ClientAuth(_): ClientAuth,
    body: Option<Json<LogoutRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    let req = body.map(|b| b.0).unwrap_or_default();
    state
        .identity
        .logout(req.token.as_deref(), req.refresh_token.as_deref())
        .await?;
    Ok(Json(serde_json::json!({})))
}

async fn get_identity(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
) -> ApiResult<Json<IdentityResponse>> {
    let i = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(IdentityResponse::from(&i)))
}

async fn delete_identity(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
) -> ApiResult<Json<serde_json::Value>> {
    state.identity.delete(ctx.identity_id()).await?;
    Ok(Json(serde_json::json!({})))
}

async fn get_properties(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
) -> ApiResult<Json<PropertySet>> {
    let i = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(i.properties))
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdatePropertiesRequest {
    #[serde(default)]
    pub default: PropertyMap,
    #[serde(default)]
    pub custom: PropertyMap,
    #[serde(default)]
    pub recompute: bool,
}

async fn update_properties(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Json(req): Json<UpdatePropertiesRequest>,
) -> ApiResult<Json<PropertySet>> {
    let i = state
        .identity
        .update_properties(ctx.identity_id(), req.default, req.custom)
        .await?;
    Ok(Json(i.properties))
}

#[derive(Debug, Default, Deserialize)]
pub struct NamesQuery {
    /// Comma-separated keys; omitted = all.
    #[serde(default)]
    pub names: Option<String>,
}

pub fn split_names(names: Option<String>) -> Vec<String> {
    names
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[derive(Debug, Serialize)]
pub struct FlagList {
    pub flags: Vec<Evaluation>,
}

#[derive(Debug, Serialize)]
pub struct AudienceList {
    pub audiences: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ExperimentList {
    pub experiments: Vec<Assignment>,
}

async fn get_flags(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Json<FlagList>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(FlagList {
        flags: state.eval.flags(&identity, &split_names(q.names)).await?,
    }))
}

async fn get_flag(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Evaluation>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(state.eval.flag(&identity, &key).await?))
}

async fn get_audiences(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
) -> ApiResult<Json<AudienceList>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(AudienceList {
        audiences: state.eval.audience_keys(&identity).await,
    }))
}

async fn get_experiments(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Json<ExperimentList>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(ExperimentList {
        experiments: state
            .eval
            .assignments(&identity, &split_names(q.names))
            .await?,
    }))
}

async fn expose_experiment(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    state.experiments.expose(&identity, &key).await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Default, Deserialize)]
pub struct LiveEventsQuery {
    #[serde(default)]
    pub names: Option<String>,
    /// Also return targeted events that are not active right now, with their next occurrence.
    #[serde(default)]
    pub upcoming: bool,
}

#[derive(Debug, Serialize)]
pub struct LiveEventList {
    pub live_events: Vec<ActiveEvent>,
}

#[derive(Debug, Serialize)]
pub struct MessageList {
    pub messages: Vec<InboxMessage>,
}

#[derive(Debug, Default, Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateMessageRequest {
    #[serde(default)]
    pub read: bool,
}

async fn get_live_events(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Query(q): Query<LiveEventsQuery>,
) -> ApiResult<Json<LiveEventList>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(LiveEventList {
        live_events: state
            .eval
            .live_events(&identity, &split_names(q.names), !q.upcoming)
            .await,
    }))
}

async fn get_messages(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Query(q): Query<InboxQuery>,
) -> ApiResult<Json<MessageList>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    Ok(Json(MessageList {
        messages: state.eval.inbox(&identity, q.limit.unwrap_or(50)).await?,
    }))
}

async fn update_message(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Path(id): Path<uuid::Uuid>,
    body: Option<Json<UpdateMessageRequest>>,
) -> ApiResult<Json<InboxMessage>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    let _ = body;
    Ok(Json(state.messages.mark_read(&identity, id).await?))
}

async fn delete_message(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    state.messages.consume(&identity, id).await?;
    Ok(Json(serde_json::json!({})))
}

async fn healthcheck() -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}
