//! Audit middleware: every mutating call under `/admin/v1` is recorded with its actor, outcome
//! and a redacted copy of the JSON body (including rejected and unauthenticated attempts).

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::Response,
};
use zanshin_store::audit::NewAuditEntry;

use super::{auth::api_key_from_parts, limits::client_ip};
use crate::{
    services::{apikeys::Principal, audit::redact},
    state::AppState,
};

const BODY_CAP: usize = 256 * 1024;

pub async fn audit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mutating = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !state.audit.enabled() || !mutating || !req.uri().path().starts_with("/admin/v1/") {
        return next.run(req).await;
    }
    let ip = client_ip(&req, state.cfg.server.trust_proxy);
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let (mut parts, body) = req.into_parts();

    let actor = match parts.extensions.get::<Principal>() {
        Some(p) => Some(p.clone()),
        None => match api_key_from_parts(&parts) {
            Some(k) => state.api_keys.authenticate(&k).await.ok(),
            None => None,
        },
    };
    if let Some(p) = &actor {
        parts.extensions.insert(p.clone());
    }

    let is_json = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);
    let (body, captured) = if state.audit.store_bodies() && is_json {
        match to_bytes(body, BODY_CAP).await {
            Ok(bytes) => {
                let parsed = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .map(|mut v| {
                        redact(&mut v);
                        v
                    });
                (Body::from(bytes), parsed)
            }
            Err(_) => (
                Body::empty(),
                Some(serde_json::json!({ "_truncated": true })),
            ),
        }
    } else {
        (body, None)
    };

    let res = next.run(Request::from_parts(parts, body)).await;

    state.audit.record(NewAuditEntry {
        actor_key_id: actor.as_ref().and_then(|p| p.key_id),
        actor_name: actor
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "unauthenticated".into()),
        actor_kind: actor
            .as_ref()
            .map(|p| p.kind.as_str().to_string())
            .unwrap_or_else(|| "none".into()),
        method,
        path,
        status: res.status().as_u16() as i32,
        request_id,
        ip: Some(ip),
        body: captured,
    });
    res
}
