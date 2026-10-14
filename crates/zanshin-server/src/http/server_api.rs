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
        .route(
            "/audiences/{key}/members",
            post(add_audience_members).delete(remove_audience_members),
        )
        .route("/experiments", get(experiments))
        .route("/experiments/{key}/expose", post(expose))
        .route("/live-events", get(live_events))
        .route("/messages", post(send_message))
        .route("/events", post(send_events))
        .route("/stream", get(stream))
        .route("/identities", post(upsert_identity))
        .route(
            "/identities/{external_id}/identify",
            post(identify_identity),
        )
        .route(
            "/identities/{external_id}/properties",
            axum::routing::put(update_identity_properties),
        )
        .route("/messages/inbox", get(inbox))
        .route(
            "/messages/inbox/{id}",
            axum::routing::put(inbox_read).delete(inbox_delete),
        )
        .route("/messages/inbox/{id}/consume", post(inbox_consume))
        .route("/flags/{key}", get(flag))
        .route("/flags/{key}/explain", get(explain_flag))
        .route("/live-events/{key}/join", post(join_live_event))
        .route("/live-events/{key}/leave", post(leave_live_event))
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

#[derive(Debug, Deserialize)]
pub struct AudienceMembersRequest {
    pub external_ids: Vec<String>,
}

/// Push membership of a managed audience from a trusted backend.
async fn add_audience_members(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Json(b): Json<AudienceMembersRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let (n, missing) = state
        .audiences
        .set_managed(&key, &b.external_ids, true)
        .await?;
    Ok(Json(serde_json::json!({ "added": n, "missing": missing })))
}

async fn remove_audience_members(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Json(b): Json<AudienceMembersRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let (n, missing) = state
        .audiences
        .set_managed(&key, &b.external_ids, false)
        .await?;
    Ok(Json(
        serde_json::json!({ "removed": n, "missing": missing }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct IdentifyIdentityRequest {
    pub id: String,
}

/// Link an identity to a new external id (merging into an existing identity if the id is taken).
async fn identify_identity(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(external_id): Path<String>,
    Json(req): Json<IdentifyIdentityRequest>,
) -> ApiResult<Json<IdentityWithProperties>> {
    let from = state.identity.get_by_external_id(&external_id).await?;
    let (s, merged) = state.identity.identify(from.id, &req.id).await?;
    if let Some(src) = &merged {
        state
            .taxonomy
            .repo
            .record_merge(&zanshin_store::taxonomy::IdentityMerge {
                id: uuid::Uuid::now_v7(),
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
            None,
        )
        .await;
    let i = s.identity;
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

async fn flag(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<zanshin_core::flags::Evaluation>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(state.eval.flag(&identity, &key).await?))
}

async fn explain_flag(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(state.eval.explain_flag(&identity, &key).await?))
}

async fn join_live_event(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<zanshin_core::live_events::ActiveEvent>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    let view = state.eval.join_live_event(&identity, &key).await?;
    state
        .events
        .system(
            &identity,
            zanshin_core::events::EV_LIVE_EVENT_JOIN,
            &key,
            None,
            None,
        )
        .await;
    Ok(Json(view))
}

async fn leave_live_event(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(key): Path<String>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    let left = state.live_events.leave(identity.id, &key).await?;
    Ok(Json(serde_json::json!({ "left": left })))
}

async fn inbox_read(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<zanshin_core::messages::InboxMessage>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    Ok(Json(state.messages.mark_read(&identity, id).await?))
}

async fn inbox_consume(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    state.messages.consume(&identity, id).await?;
    Ok(Json(serde_json::json!({})))
}

async fn inbox_delete(
    State(state): State<AppState>,
    ServerAuth(_): ServerAuth,
    Path(id): Path<uuid::Uuid>,
    Query(q): Query<IdentityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    q.check()?;
    let identity = state.eval.identity_by_external_id(&q.external_id).await?;
    state.messages.delete_inbox(&identity, id).await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}
