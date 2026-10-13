//! WebSocket change stream, `GET /v1/stream`. Authenticate with `Authorization: Bearer` or
//! `?token=` (browsers cannot set headers on WebSocket upgrades).
//!
//! Server → client messages:
//! `{"type":"hello","identity_id":"…"}`, `{"type":"flag","flag":{…}}`, `{"type":"flag_deleted","key":"…"}`,
//! `{"type":"flags","flags":[…]}` and `{"type":"experiments","experiments":[…]}` after an audience or
//! experiment change, `{"type":"error","message":"…"}`. Client may send `{"type":"ping"}` → `{"type":"pong"}`.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::request::Parts,
    response::Response,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use zanshin_core::Error;

use crate::{
    services::{bus::ChangeKind, identity::SessionContext},
    state::AppState,
};

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

async fn send(socket: &mut WebSocket, msg: serde_json::Value) -> bool {
    socket
        .send(Message::Text(msg.to_string().into()))
        .await
        .is_ok()
}

async fn run(mut socket: WebSocket, state: AppState, ctx: SessionContext, names: Vec<String>) {
    let identity_id = ctx.identity_id();
    let expires_at = ctx.claims.expires_at();
    let mut rx = state.bus.subscribe();
    metrics::gauge!("zanshin_ws_connections").increment(1.0);
    send(
        &mut socket,
        json!({"type": "hello", "identity_id": identity_id}),
    )
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
                    send(&mut socket, json!({"type": "error", "message": "session expired"})).await;
                    break;
                }
                if change.identity_id.is_some_and(|id| id != identity_id) {
                    continue;
                }
                let Ok(identity) = state.identity.get(identity_id).await else { break };
                let ok = match change.kind {
                    ChangeKind::Flag => {
                        if !names.is_empty() && !names.contains(&change.key) {
                            continue;
                        }
                        if change.deleted {
                            send(&mut socket, json!({"type": "flag_deleted", "key": change.key})).await
                        } else {
                            match state.eval.flag(&identity, &change.key).await {
                                Ok(ev) => send(&mut socket, json!({"type": "flag", "flag": ev})).await,
                                Err(_) => continue,
                            }
                        }
                    }
                    ChangeKind::LiveEvent => {
                        let events = state.eval.live_events(&identity, &[], false).await;
                        send(&mut socket, json!({"type": "live_events", "live_events": events})).await
                    }
                    ChangeKind::Message => {
                        let inbox = state.eval.inbox(&identity, 50).await.unwrap_or_default();
                        send(&mut socket, json!({"type": "inbox", "messages": inbox})).await
                    }
                    ChangeKind::Audience | ChangeKind::Experiment => {
                        let flags = state.eval.flags(&identity, &names).await.unwrap_or_default();
                        let experiments = state.eval.assignments(&identity, &[]).await.unwrap_or_default();
                        send(&mut socket, json!({"type": "flags", "flags": flags})).await
                            && send(&mut socket, json!({"type": "experiments", "experiments": experiments})).await
                    }
                };
                if !ok {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        if t.contains("\"ping\"") {
                            send(&mut socket, json!({"type": "pong"})).await;
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
