//! Client API, `/v1`.

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use zanshin_core::{properties::PropertyMap, Identity, PropertySet};

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

async fn healthcheck() -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}
