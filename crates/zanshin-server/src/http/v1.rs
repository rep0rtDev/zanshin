//! Client API, `/v1`.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
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
        .route("/identify", post(identify))
        .route("/properties", get(get_properties).put(update_properties))
        .route("/flags", get(get_flags))
        .route("/flags/{key}", get(get_flag))
        .route("/audiences", get(get_audiences))
        .route("/experiments", get(get_experiments))
        .route("/experiments/{key}/expose", post(expose_experiment))
        .route("/events", post(send_events))
        .route("/live-events", get(get_live_events))
        .route("/messages", get(get_messages))
        .route(
            "/messages/{id}",
            axum::routing::put(update_message).delete(delete_message),
        )
        .route(
            "/push-tokens",
            get(list_push_tokens).put(register_push_token),
        )
        .route(
            "/push-tokens/{token}",
            axum::routing::delete(unregister_push_token),
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
    headers: axum::http::HeaderMap,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Json(mut req): Json<AuthenticateRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let ip = client_ip(&headers, Some(peer.ip()), state.cfg.server.trust_proxy);
    state
        .taxonomy
        .enrich_defaults(&headers, ip, &mut req.default);
    let s = state
        .identity
        .authenticate(&req.id, req.default, req.custom)
        .await?;
    begin_play_session(&state, &s).await;
    Ok(Json(session_response(s)))
}

fn client_ip(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::IpAddr>,
    trust_proxy: bool,
) -> Option<std::net::IpAddr> {
    if trust_proxy {
        for name in ["x-forwarded-for", "x-real-ip"] {
            if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
                if let Some(ip) = v.split(',').next().and_then(|s| s.trim().parse().ok()) {
                    return Some(ip);
                }
            }
        }
    }
    peer
}

/// Record a play session and the synthetic `$identity_create` / `$session_start` events.
async fn begin_play_session(state: &AppState, s: &crate::services::identity::SessionTokens) {
    let d = &s.identity.properties.default;
    let str_of = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    state
        .taxonomy
        .repo
        .open_session(
            s.session_id,
            s.identity.id,
            chrono::Utc::now(),
            &str_of("platform"),
            &str_of("app_version"),
        )
        .await
        .ok();
    if s.created {
        state
            .events
            .system(
                &s.identity,
                zanshin_core::events::EV_IDENTITY_CREATE,
                "",
                None,
                Some(s.session_id),
            )
            .await;
    }
    state
        .events
        .system(
            &s.identity,
            zanshin_core::events::EV_AUTHENTICATE,
            "",
            None,
            Some(s.session_id),
        )
        .await;
    state
        .events
        .system(
            &s.identity,
            zanshin_core::events::EV_SESSION_START,
            "",
            None,
            Some(s.session_id),
        )
        .await;
}

/// Close a play session and emit `$session_end` with the duration in seconds.
pub async fn end_play_session(state: &AppState, sid: Uuid) {
    if let Ok(Some((identity_id, duration))) = state
        .taxonomy
        .repo
        .close_session(sid, chrono::Utc::now())
        .await
    {
        if let Ok(identity) = state.identity.get(identity_id).await {
            state
                .events
                .system(
                    &identity,
                    zanshin_core::events::EV_SESSION_END,
                    duration.to_string(),
                    None,
                    Some(sid),
                )
                .await;
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct IdentifyRequest {
    pub id: String,
}

async fn identify(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Json(req): Json<IdentifyRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let from = state.identity.get(ctx.identity_id()).await?;
    let (s, merged) = state.identity.identify(from.id, &req.id).await?;
    state
        .taxonomy
        .repo
        .close_session(ctx.claims.sid, chrono::Utc::now())
        .await
        .ok();
    if let Some(src) = &merged {
        state
            .taxonomy
            .repo
            .record_merge(&zanshin_store::taxonomy::IdentityMerge {
                id: Uuid::now_v7(),
                from_identity: src.id,
                from_external: src.external_id.clone(),
                into_identity: s.identity.id,
                into_external: s.identity.external_id.clone(),
                merged_at: chrono::Utc::now(),
            })
            .await
            .ok();
    }
    let mut meta = serde_json::Map::new();
    meta.insert(
        "from".into(),
        serde_json::Value::String(from.external_id.clone()),
    );
    meta.insert(
        "to".into(),
        serde_json::Value::String(s.identity.external_id.clone()),
    );
    meta.insert("merged".into(), serde_json::Value::Bool(merged.is_some()));
    state
        .events
        .system(
            &s.identity,
            zanshin_core::events::EV_IDENTIFY,
            "",
            Some(meta),
            Some(s.session_id),
        )
        .await;
    begin_play_session(&state, &s).await;
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
    state
        .taxonomy
        .repo
        .touch_session(s.session_id, chrono::Utc::now())
        .await
        .ok();
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
    let sid = state
        .identity
        .logout(req.token.as_deref(), req.refresh_token.as_deref())
        .await?;
    end_play_session(&state, sid).await;
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
    let _ = state.events.delete_identity(ctx.identity_id()).await;
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
    state
        .taxonomy
        .check_client_custom_write(&req.custom)
        .await?;
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

#[derive(Debug, Deserialize)]
pub struct EventBatch {
    pub events: Vec<zanshin_core::events::EventInput>,
}

async fn send_events(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Json(req): Json<EventBatch>,
) -> ApiResult<Json<serde_json::Value>> {
    let identity = state.identity.get(ctx.identity_id()).await?;
    let events = state
        .taxonomy
        .check_events(identity.id, &identity.external_id, req.events)
        .await?;
    let n = state
        .events
        .track(&identity, events, false, Some(ctx.claims.sid))
        .await?;
    let repo = state.taxonomy.repo.clone();
    let sid = ctx.claims.sid;
    tokio::spawn(async move {
        repo.touch_session(sid, chrono::Utc::now()).await.ok();
    });
    Ok(Json(serde_json::json!({ "accepted": n })))
}

#[derive(Debug, Deserialize)]
pub struct RegisterPushTokenRequest {
    pub platform: zanshin_core::push::Platform,
    pub token: String,
    #[serde(default)]
    pub device_id: String,
}

async fn register_push_token(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Json(req): Json<RegisterPushTokenRequest>,
) -> ApiResult<Json<zanshin_core::push::PushToken>> {
    Ok(Json(
        state
            .push
            .register(ctx.identity_id(), req.platform, &req.token, &req.device_id)
            .await?,
    ))
}

async fn list_push_tokens(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::json!({ "tokens": state.push.tokens(ctx.identity_id()).await? }),
    ))
}

async fn unregister_push_token(
    State(state): State<AppState>,
    SessionAuth(ctx): SessionAuth,
    Path(token): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.push.unregister(ctx.identity_id(), &token).await?;
    Ok(Json(serde_json::json!({})))
}

async fn healthcheck() -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}
