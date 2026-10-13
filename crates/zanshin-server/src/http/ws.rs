//! WebSocket change stream, `GET /v1/stream`. Authenticate with `Authorization: Bearer` or
//! `?token=` (browsers cannot set headers on WebSocket upgrades).
//!
//! Server → client messages:
//! `{"type":"hello","identity_id":"…"}`, `{"type":"flag","flag":{…}}`, `{"type":"flag_deleted","key":"…"}`,
//! `{"type":"error","message":"…"}`. Client may send `{"type":"ping"}` and receives `{"type":"pong"}`.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::request::Parts,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use zanshin_core::Error;

use crate::{services::identity::SessionContext, state::AppState};

use super::{auth::bearer_from_parts, error::ApiError};

#[derive(Debug, Default, Deserialize)]
pub struct StreamQuery {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub names: Option<String>,
}

pub async fn stream(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
    parts: Parts,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let token = bearer_from_parts(&parts)
        .or(q.token)
        .ok_or_else(|| Error::unauthenticated("session token required"))?;
    let ctx = state.identity.verify_access(&token).await?;
    let names = super::v1::split_names(q.names);
    Ok(ws.on_upgrade(move |socket| run(socket, state, ctx, names)))
}

async fn run(mut socket: WebSocket, state: AppState, ctx: SessionContext, names: Vec<String>) {
    let identity_id = ctx.identity_id();
    let expires_at = ctx.claims.expires_at();
    let mut rx = state.flags.subscribe();
    metrics::gauge!("zanshin_ws_connections").increment(1.0);
    let _ = socket
        .send(Message::Text(
            json!({"type": "hello", "identity_id": identity_id})
                .to_string()
                .into(),
        ))
        .await;

    loop {
        tokio::select! {
            change = rx.recv() => {
                let change = match change {
                    Ok(c) => c,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                if chrono::Utc::now() >= expires_at {
                    let _ = socket.send(Message::Text(json!({"type": "error", "message": "session expired"}).to_string().into())).await;
                    break;
                }
                if change.identity_id.is_some_and(|id| id != identity_id) || (!names.is_empty() && !names.contains(&change.key)) {
                    continue;
                }
                let msg = if change.deleted {
                    json!({"type": "flag_deleted", "key": change.key})
                } else {
                    let Ok(identity) = state.identity.get(identity_id).await else { break };
                    match state.flags.evaluate_one(&identity, &change.key).await {
                        Ok(ev) => json!({"type": "flag", "flag": ev}),
                        Err(_) => continue,
                    }
                };
                if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        if t.contains("\"ping\"") {
                            let _ = socket.send(Message::Text(json!({"type": "pong"}).to_string().into())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    metrics::gauge!("zanshin_ws_connections").decrement(1.0);
}

impl IntoResponse for Box<ApiError> {
    fn into_response(self) -> Response {
        (*self).into_response()
    }
}
