//! Rate limiting middleware: classifies each API request into a scope + caller id and asks the
//! Redis token bucket for admission. Verified principals/sessions are stashed in request
//! extensions so the auth extractors do not verify them a second time.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use zanshin_core::apikey::ApiKeyKind;

use super::auth::{api_key_from_parts, bearer_from_parts, console_principal};
use crate::{
    services::{apikeys::Principal, identity::SessionContext, limits::Scope},
    state::AppState,
};

/// Best-effort client address: the proxy chain head when `server.trust_proxy` is set, else the peer.
pub fn client_ip(req: &Request, trust_proxy: bool) -> String {
    if trust_proxy {
        for name in ["x-forwarded-for", "x-real-ip"] {
            if let Some(v) = req.headers().get(name).and_then(|v| v.to_str().ok()) {
                if let Some(first) = v.split(',').next().map(str::trim) {
                    if !first.is_empty() {
                        return first.to_string();
                    }
                }
            }
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn is_api(path: &str) -> bool {
    path.starts_with("/v1/") || path.starts_with("/server/v1/") || path.starts_with("/admin/v1/")
}

pub async fn limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !state.limits.enabled() || !is_api(req.uri().path()) {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let (scope, id, override_pm) = match api_key_from_parts(&parts) {
        Some(key) => match state.api_keys.authenticate(&key).await {
            Ok(p) => {
                let scope = match p.kind {
                    ApiKeyKind::Client => Scope::Client,
                    ApiKeyKind::Server => Scope::Server,
                    ApiKeyKind::Admin => Scope::Admin,
                };
                let id = p
                    .key_id
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "bootstrap".into());
                let pm = p.rate_limit_per_minute;
                parts.extensions.insert::<Principal>(p);
                (scope, id, pm)
            }
            // Invalid keys are throttled by IP; the extractor will answer 401.
            Err(_) => {
                let req = Request::from_parts(parts, body);
                let ip = client_ip(&req, state.cfg.server.trust_proxy);
                return decide(state, Scope::Ip, ip, None, req, next).await;
            }
        },
        None if console_principal(&parts, &state).is_some() => {
            let p = console_principal(&parts, &state).expect("checked above");
            let id = format!("oidc:{}", p.name);
            parts.extensions.insert::<Principal>(p);
            (Scope::Admin, id, None)
        }
        None => match bearer_from_parts(&parts) {
            Some(token) => match state.identity.verify_access(&token).await {
                Ok(s) => {
                    let id = s.identity_id().to_string();
                    parts.extensions.insert::<SessionContext>(s);
                    (Scope::Session, id, None)
                }
                Err(_) => {
                    let req = Request::from_parts(parts, body);
                    let ip = client_ip(&req, state.cfg.server.trust_proxy);
                    return decide(state, Scope::Ip, ip, None, req, next).await;
                }
            },
            None => {
                let req = Request::from_parts(parts, body);
                let ip = client_ip(&req, state.cfg.server.trust_proxy);
                return decide(state, Scope::Ip, ip, None, req, next).await;
            }
        },
    };
    let req = Request::from_parts(parts, body);
    decide(state, scope, id, override_pm, req, next).await
}

async fn decide(
    state: AppState,
    scope: Scope,
    id: String,
    override_pm: Option<u32>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let d = state.limits.check(scope, &id, override_pm).await;
    let mut res = if d.allowed {
        next.run(req).await
    } else {
        let retry = d.retry_after.ceil().max(1.0) as u64;
        let mut r = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "code": "resource_exhausted",
                "message": format!("rate limit exceeded for {} scope, retry in {retry}s", scope.as_str()),
            })),
        )
            .into_response();
        r.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(retry));
        r
    };
    let h = res.headers_mut();
    h.insert("x-ratelimit-limit", HeaderValue::from(d.limit));
    h.insert("x-ratelimit-remaining", HeaderValue::from(d.remaining));
    h.insert(
        "x-ratelimit-scope",
        HeaderValue::from_static(scope.as_str()),
    );
    res
}
