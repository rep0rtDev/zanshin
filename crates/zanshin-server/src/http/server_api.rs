//! Trusted-backend API, `/server/v1`. Requires a `server` or `admin` API key and addresses
//! identities by external id — this is what the Nakama module and game servers call.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::request::Parts,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use zanshin_core::Error;

use crate::state::AppState;

use super::{
    auth::ServerAuth,
    error::ApiResult,
    v1::{
        split_names, AudienceList, ExperimentList, FlagList, IdentityResponse, LiveEventList,
        MessageList,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/flags", get(flags))
        .route("/audiences", get(audiences))
        .route("/experiments", get(experiments))
        .route("/experiments/{key}/expose", post(expose))
        .route("/live-events", get(live_events))
        .route("/messages", post(send_message))
        .route("/events", post(send_events))
        .route("/stream", get(stream))
        .route("/identities", post(upsert_identity))
        .route(
            "/identities/{external_id}/properties",
            axum::routing::put(update_identity_properties),
        )
        .route("/messages/inbox", get(inbox))
}

#[derive(Debug, Deserialize)]
pub struct IdentityQuery {
    pub external_id: String,
    #[serde(default)]
    pub names: Option<String>,
    #[serde(default)]
    pub upcoming: bool,
}

impl IdentityQuery {
    fn check(&self) -> ApiResult<()> {
        if self.external_id.is_empty() {
            return Err(Error::invalid("external_id is required").into());
        }
        Ok(())
    }
}

async fn flags(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<FlagList>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(FlagList {
        flags: state.eval.flags(&identity, &split_names(q.names)).await?,
    }))
}

async fn audiences(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<AudienceList>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(AudienceList {
        audiences: state.eval.audience_keys(&identity).await,
    }))
}

async fn experiments(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<ExperimentList>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(ExperimentList {
        experiments: state
            .eval
            .assignments(&identity, &split_names(q.names))
            .await?,
    }))
}

async fn expose(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    state.experiments.expose(&identity, &key).await?;
    Ok(Json(serde_json::json!({})))
}

async fn live_events(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<LiveEventList>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(LiveEventList {
        live_events: state
            .eval
            .live_events(&identity, &split_names(q.names), !q.upcoming)
            .await,
    }))
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub external_id: String,
    #[serde(flatten)]
    pub message: crate::services::messages::DirectMessage,
}

async fn send_message(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    if req.external_id.is_empty() {
        return Err(Error::invalid("external_id is required").into());
    }
    let identity = state.eval.identity_by_external_id(&req.external_id).await?;
    let id = state.messages.send_direct(&identity, req.message).await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({ "id": id })),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ServerEventBatch {
    pub external_id: String,
    pub events: Vec<zanshin_core::events::EventInput>,
}

async fn send_events(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Json(req): Json<ServerEventBatch>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.external_id.is_empty() {
        return Err(Error::invalid("external_id is required").into());
    }
    let identity = state.eval.identity_by_external_id(&req.external_id).await?;
    let n = state
        .events
        .track(&identity, req.events, true, None)
        .await?;
    Ok(Json(serde_json::json!({ "accepted": n })))
}

/// Change feed for trusted backends: every configuration change (`flag` / `audience` /
/// `experiment` / `live_event` / `message`) as `{"type":"change","kind","key","version","deleted","identity_id"}`.
/// Backends use it to invalidate local caches of per-identity evaluations. Authenticate with
/// `x-api-key` (or `?api_key=`) of kind server / admin.
#[derive(Debug, Default, Deserialize)]
pub struct StreamQuery {
    #[serde(default)]
    pub api_key: Option<String>,
}

async fn stream(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
    parts: Parts,
    ws: WebSocketUpgrade,
) -> Result<Response, super::error::ApiError> {
    let key = super::auth::api_key_from_parts(&parts)
        .or(q.api_key)
        .ok_or_else(|| Error::unauthenticated("server API key required"))?;
    let principal = state.api_keys.authenticate(&key).await?;
    if !principal.is_server() {
        return Err(Error::denied("server or admin API key required").into());
    }
    Ok(ws.on_upgrade(move |socket| run_stream(socket, state)))
}

async fn run_stream(mut socket: WebSocket, state: AppState) {
    let mut rx = state.bus.subscribe();
    metrics::gauge!("zanshin_server_stream_connections").increment(1.0);
    let hello = serde_json::json!({"type": "hello", "node": state.bus.node_id()});
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            change = rx.recv() => {
                let change = match change {
                    Ok(c) => c,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = socket.send(Message::Text(serde_json::json!({"type": "resync"}).to_string().into())).await;
                        continue;
                    }
                    Err(_) => break,
                };
                let msg = serde_json::json!({
                    "type": "change", "kind": change.kind, "key": change.key, "version": change.version,
                    "deleted": change.deleted, "identity_id": change.identity_id,
                });
                if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) if t.contains("\"ping\"") => {
                    let _ = socket.send(Message::Text(serde_json::json!({"type": "pong"}).to_string().into())).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
    metrics::gauge!("zanshin_server_stream_connections").decrement(1.0);
}

#[derive(Debug, Deserialize)]
pub struct UpsertIdentityRequest {
    pub external_id: String,
    #[serde(default)]
    pub default: zanshin_core::properties::PropertyMap,
    #[serde(default)]
    pub custom: zanshin_core::properties::PropertyMap,
}

#[derive(Debug, serde::Serialize)]
pub struct IdentityWithProperties {
    #[serde(flatten)]
    pub identity: IdentityResponse,
    pub properties: zanshin_core::PropertySet,
}

/// Create or refresh an identity from a trusted backend (e.g. on Nakama login). No session is issued.
async fn upsert_identity(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Json(req): Json<UpsertIdentityRequest>,
) -> ApiResult<Json<IdentityWithProperties>> {
    let i = state
        .identity
        .upsert(&req.external_id, req.default, req.custom)
        .await?;
    state
        .events
        .system(&i, zanshin_core::events::EV_AUTHENTICATE, "", None, None)
        .await;
    Ok(Json(IdentityWithProperties {
        identity: IdentityResponse::from(&i),
        properties: i.properties,
    }))
}

#[derive(Debug, Default, Deserialize)]
pub struct PropertiesPatch {
    #[serde(default)]
    pub default: zanshin_core::properties::PropertyMap,
    #[serde(default)]
    pub custom: zanshin_core::properties::PropertyMap,
}

async fn update_identity_properties(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(external_id): Path<String>,
    Json(req): Json<PropertiesPatch>,
) -> ApiResult<Json<zanshin_core::PropertySet>> {
    let identity = state.eval.identity_by_external_id(&external_id).await?;
    let i = state
        .identity
        .update_properties(identity.id, req.default, req.custom)
        .await?;
    Ok(Json(i.properties))
}

#[derive(Debug, Deserialize)]
pub struct InboxQuery {
    pub external_id: String,
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn inbox(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Query(q): Query<InboxQuery>,
) -> ApiResult<Json<MessageList>> {
    if q.external_id.is_empty() {
        return Err(Error::invalid("external_id is required").into());
    }
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(MessageList {
        messages: state.eval.inbox(&identity, q.limit.unwrap_or(50)).await?,
    }))
}
