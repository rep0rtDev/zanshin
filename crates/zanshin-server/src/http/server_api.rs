//! Trusted-backend API, `/server/v1`. Requires a `server` or `admin` API key and addresses
//! identities by external id — this is what the Nakama module and game servers call.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use zanshin_core::Error;

use crate::state::AppState;

use super::{
    auth::ServerAuth,
    error::ApiResult,
    v1::{split_names, AudienceList, ExperimentList, FlagList, LiveEventList},
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
