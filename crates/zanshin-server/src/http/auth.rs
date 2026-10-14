//! Axum extractors for the three credential types.
//!
//! * `ApiKeyAuth`   — `x-api-key: <key>` or `Authorization: Basic base64(<key>:)`; any key kind.
//! * `SessionAuth`  — `Authorization: Bearer <access token>`.
//! * `AdminAuth`    — API key of kind `admin` (also accepts `Authorization: Bearer <admin key>`).

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use zanshin_core::Error;

use crate::{
    services::{apikeys::Principal, identity::SessionContext},
    state::AppState,
};

use super::error::ApiError;

fn header_str<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
}

/// Extract an API key from `x-api-key`, Basic auth or Bearer auth (when it looks like a key).
pub fn api_key_from_parts(parts: &Parts) -> Option<String> {
    if let Some(k) = header_str(parts, "x-api-key") {
        if !k.is_empty() {
            return Some(k.to_string());
        }
    }
    let auth = header_str(parts, header::AUTHORIZATION.as_str())?;
    if let Some(b64) = auth
        .strip_prefix("Basic ")
        .or_else(|| auth.strip_prefix("basic "))
    {
        let decoded = STANDARD.decode(b64.trim()).ok()?;
        let s = String::from_utf8(decoded).ok()?;
        let user = s.split(':').next()?.trim();
        if !user.is_empty() {
            return Some(user.to_string());
        }
    }
    if let Some(tok) = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
    {
        if zanshin_core::apikey::looks_like_key(tok.trim()) || !tok.contains('.') {
            return Some(tok.trim().to_string());
        }
    }
    None
}

pub fn bearer_from_parts(parts: &Parts) -> Option<String> {
    let auth = header_str(parts, header::AUTHORIZATION.as_str())?;
    auth.strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .map(|t| t.trim().to_string())
}

/// Resolve the API key principal, reusing the one the rate limiter already verified for this request.
pub async fn principal(parts: &mut Parts, state: &AppState) -> Result<Option<Principal>, Error> {
    if let Some(p) = parts.extensions.get::<Principal>() {
        return Ok(Some(p.clone()));
    }
    let Some(key) = api_key_from_parts(parts) else {
        if let Some(p) = console_principal(parts, state).await {
            parts.extensions.insert(p.clone());
            return Ok(Some(p));
        }
        return Ok(None);
    };
    let p = state.api_keys.authenticate(&key).await?;
    parts.extensions.insert(p.clone());
    Ok(Some(p))
}

pub async fn session(parts: &mut Parts, state: &AppState) -> Result<Option<SessionContext>, Error> {
    if let Some(s) = parts.extensions.get::<SessionContext>() {
        return Ok(Some(s.clone()));
    }
    let Some(token) = bearer_from_parts(parts) else {
        return Ok(None);
    };
    let s = state.identity.verify_access(&token).await?;
    parts.extensions.insert(s.clone());
    Ok(Some(s))
}

pub struct ApiKeyAuth(pub Principal);

impl FromRequestParts<AppState> for ApiKeyAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match principal(parts, state).await? {
            Some(p) => Ok(ApiKeyAuth(p)),
            None => Err(Error::unauthenticated("api key required").into()),
        }
    }
}

/// Like `ApiKeyAuth`, but honours `auth.allow_anonymous_authenticate` for development.
pub struct ClientAuth(pub Option<Principal>);

impl FromRequestParts<AppState> for ClientAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match principal(parts, state).await? {
            Some(p) => Ok(ClientAuth(Some(p))),
            None if state.cfg.auth.allow_anonymous_authenticate => Ok(ClientAuth(None)),
            None => Err(Error::unauthenticated("api key required").into()),
        }
    }
}

pub struct SessionAuth(pub SessionContext);

impl FromRequestParts<AppState> for SessionAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match session(parts, state).await? {
            Some(s) => Ok(SessionAuth(s)),
            None => Err(Error::unauthenticated("session token required").into()),
        }
    }
}

pub struct AdminAuth(pub Principal);

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(p) = principal(parts, state).await? else {
            return Err(Error::unauthenticated("admin api key required").into());
        };
        if !p.is_admin() {
            return Err(Error::denied("admin api key required").into());
        }
        // Inside the nested `/admin/v1` router the request URI is already stripped.
        let path = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|u| u.path().to_string())
            .unwrap_or_else(|| parts.uri.path().to_string());
        if let Some(resource) = zanshin_core::permissions::resource_for_path(&path) {
            let action = zanshin_core::permissions::Action::for_method(parts.method.as_str());
            if !p.allows(resource, action) {
                return Err(Error::denied(format!(
                    "permission required: {resource}:{}",
                    action.as_str()
                ))
                .into());
            }
        }
        Ok(AdminAuth(p))
    }
}

/// Server-or-admin key: trusted backends acting on behalf of identities.
pub struct ServerAuth(pub Principal);

impl FromRequestParts<AppState> for ServerAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(p) = principal(parts, state).await? else {
            return Err(Error::unauthenticated("server api key required").into());
        };
        if !p.is_server() {
            return Err(Error::denied("server api key required").into());
        }
        Ok(ServerAuth(p))
    }
}

/// Admin principal carried by a console cookie: an OIDC session (stateless JWT) or a console
/// user session (server-side, password / TOTP login).
pub async fn console_principal(parts: &Parts, state: &AppState) -> Option<Principal> {
    let cookie = header_str(parts, header::COOKIE.as_str())?;
    if state.oidc.enabled() {
        if let Some(token) =
            crate::services::oidc::cookie_value(cookie, crate::services::oidc::SESSION_COOKIE)
        {
            if let Some(u) = state.oidc.verify_session(token) {
                return Some(u.principal());
            }
        }
    }
    let token = crate::services::oidc::cookie_value(
        cookie,
        crate::services::console_users::SESSION_COOKIE,
    )?;
    let (session, user) = state.console_users.verify_session(token).await.ok()??;
    let mut p = crate::services::console_users::ConsoleUserService::principal_for(&user);
    p.name = user.email.clone();
    let _ = session;
    Some(p)
}
