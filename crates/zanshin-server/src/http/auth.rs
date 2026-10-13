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

pub struct ApiKeyAuth(pub Principal);

impl FromRequestParts<AppState> for ApiKeyAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(key) = api_key_from_parts(parts) else {
            return Err(Error::unauthenticated("api key required").into());
        };
        Ok(ApiKeyAuth(state.api_keys.authenticate(&key).await?))
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
        match api_key_from_parts(parts) {
            Some(key) => Ok(ClientAuth(Some(state.api_keys.authenticate(&key).await?))),
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
        let Some(token) = bearer_from_parts(parts) else {
            return Err(Error::unauthenticated("session token required").into());
        };
        Ok(SessionAuth(state.identity.verify_access(&token).await?))
    }
}

pub struct AdminAuth(pub Principal);

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(key) = api_key_from_parts(parts) else {
            return Err(Error::unauthenticated("admin api key required").into());
        };
        let p = state.api_keys.authenticate(&key).await?;
        if !p.is_admin() {
            return Err(Error::denied("admin api key required").into());
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
        let Some(key) = api_key_from_parts(parts) else {
            return Err(Error::unauthenticated("server api key required").into());
        };
        let p = state.api_keys.authenticate(&key).await?;
        if !p.is_server() {
            return Err(Error::denied("server api key required").into());
        }
        Ok(ServerAuth(p))
    }
}
