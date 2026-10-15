//! Admin console: the compiled React app from `console/dist` embedded into the binary
//! and served under `/console` with SPA fallback (deep links render `index.html`).

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};

use crate::{
    services::oidc::{cookie_value, SESSION_COOKIE, STATE_COOKIE},
    state::AppState,
};

use super::error::ApiError;

#[derive(RustEmbed)]
#[folder = "../../console/dist/"]
#[exclude = ".gitkeep"]
struct Assets;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/console",
            get(|| async { Redirect::permanent("/console/") }),
        )
        .route("/console/", get(index))
        .route("/console/auth/config", get(auth_config))
        .route("/console/auth/login", get(auth_login))
        .route("/console/auth/callback", get(auth_callback))
        .route("/console/auth/saml/login", get(saml_login))
        .route("/console/auth/saml/acs", post(saml_acs))
        .route("/console/auth/saml/metadata", get(saml_metadata))
        .route("/console/auth/logout", post(auth_logout))
        .route("/console/auth/password", post(auth_password))
        .route("/console/auth/reset", post(auth_reset))
        .route("/console/{*path}", get(asset))
}

#[derive(Serialize)]
struct AuthConfig {
    oidc: bool,
    saml: bool,
    /// Email + password (+ TOTP) login is always available once a user exists.
    password: bool,
    /// No console users yet: the UI offers "create the first admin" with the bootstrap key.
    needs_bootstrap: bool,
}

async fn auth_config(State(state): State<AppState>) -> Json<AuthConfig> {
    let users = state.console_users.repo().count().await.unwrap_or(1);
    Json(AuthConfig {
        oidc: state.oidc.enabled(),
        saml: state.saml.enabled(),
        password: users > 0,
        needs_bootstrap: users == 0,
    })
}

#[derive(Deserialize)]
struct PasswordLogin {
    email: String,
    password: String,
    #[serde(default)]
    code: Option<String>,
}

/// Email + password (+ TOTP) login; sets the console user session cookie.
async fn auth_password(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PasswordLogin>,
) -> Result<Response, ApiError> {
    use crate::services::console_users::{LoginOutcome, SESSION_COOKIE as USER_COOKIE};
    let ip = if state.cfg.server.trust_proxy {
        headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| peer.ip().to_string())
    } else {
        peer.ip().to_string()
    };
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    match state
        .console_users
        .login(
            &req.email,
            &req.password,
            req.code.as_deref(),
            Some(&ip),
            ua,
        )
        .await?
    {
        LoginOutcome::MfaRequired => Ok((
            StatusCode::UNAUTHORIZED,
            Json(
                serde_json::json!({ "code": "mfa_required", "message": "one-time code required" }),
            ),
        )
            .into_response()),
        LoginOutcome::Ok { user, token, .. } => {
            tracing::info!(email = %user.email, "console password login");
            let secure = state.oidc.secure_cookies()
                || headers
                    .get("x-forwarded-proto")
                    .and_then(|v| v.to_str().ok())
                    == Some("https");
            Ok((
                [(
                    header::SET_COOKIE,
                    cookie(
                        USER_COOKIE,
                        &token,
                        Some(state.console_users.session_ttl().as_secs()),
                        secure,
                    )
                    .parse::<HeaderValue>()
                    .expect("cookie header"),
                )],
                Json(serde_json::json!({
                    "email": user.email,
                    "name": user.name,
                    "must_change_password": user.must_change_password,
                })),
            )
                .into_response())
        }
    }
}

#[derive(Deserialize)]
struct ResetRequest {
    token: String,
    password: String,
}

async fn auth_reset(
    State(state): State<AppState>,
    Json(req): Json<ResetRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = state
        .console_users
        .complete_reset(&req.token, &req.password)
        .await?;
    Ok(Json(serde_json::json!({ "email": user.email })))
}

fn cookie(name: &str, value: &str, max_age: Option<u64>, secure: bool) -> String {
    let mut c = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax");
    if let Some(a) = max_age {
        c.push_str(&format!("; Max-Age={a}"));
    }
    if secure {
        c.push_str("; Secure");
    }
    c
}

async fn auth_login(State(state): State<AppState>) -> Result<Response, ApiError> {
    let start = state.oidc.begin_login().await?;
    let mut resp = Redirect::to(&start.redirect).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        cookie(
            STATE_COOKIE,
            &start.state_cookie,
            Some(600),
            state.oidc.secure_cookies(),
        )
        .parse()
        .expect("cookie header"),
    );
    Ok(resp)
}

#[derive(Deserialize)]
struct Callback {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

async fn auth_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Response {
    let secure = state.oidc.secure_cookies();
    let fail = |msg: String| {
        let mut r = Redirect::to(&format!(
            "/console/?error={}",
            url::form_urlencoded::byte_serialize(msg.as_bytes()).collect::<String>()
        ))
        .into_response();
        r.headers_mut().insert(
            header::SET_COOKIE,
            cookie(STATE_COOKIE, "", Some(0), secure)
                .parse()
                .expect("cookie header"),
        );
        r
    };
    if let Some(e) = q.error {
        return fail(q.error_description.unwrap_or(e));
    }
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return fail("missing code or state".into());
    };
    let state_cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie_value(c, STATE_COOKIE));
    let user = match state.oidc.finish_login(&code, &st, state_cookie).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "console oidc login rejected");
            return fail(e.to_string());
        }
    };
    let token = match state.oidc.issue_session(&user) {
        Ok(t) => t,
        Err(e) => return fail(e.to_string()),
    };
    tracing::info!(email = %user.email, role = user.role.as_str(), "console oidc login");
    let mut r = Redirect::to("/console/").into_response();
    let h = r.headers_mut();
    h.append(
        header::SET_COOKIE,
        cookie(STATE_COOKIE, "", Some(0), secure)
            .parse()
            .expect("cookie header"),
    );
    h.append(
        header::SET_COOKIE,
        cookie(
            SESSION_COOKIE,
            &token,
            Some(state.oidc.session_ttl().as_secs()),
            secure,
        )
        .parse()
        .expect("cookie header"),
    );
    r
}

async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    use crate::services::console_users::SESSION_COOKIE as USER_COOKIE;
    let secure = state.oidc.secure_cookies();
    // Revoke the server-side user session, if the request carried one.
    if let Some(token) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie_value(c, USER_COOKIE))
    {
        if let Ok(Some((session, user))) = state.console_users.verify_session(token).await {
            let _ = state.console_users.logout(user.id, session.id).await;
        }
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    for name in [SESSION_COOKIE, USER_COOKIE] {
        r.headers_mut().append(
            header::SET_COOKIE,
            cookie(name, "", Some(0), secure)
                .parse()
                .expect("cookie header"),
        );
    }
    r
}

async fn index() -> Response {
    serve("index.html").unwrap_or_else(missing)
}

async fn asset(Path(path): Path<String>, _uri: Uri) -> Response {
    match serve(&path) {
        Some(r) => r,
        None if path.contains('.') => StatusCode::NOT_FOUND.into_response(),
        None => index().await,
    }
}

fn serve(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Some(
        (
            [
                (header::CONTENT_TYPE, mime.as_ref().to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'self'; img-src 'self' data: https:; style-src 'self' 'unsafe-inline'; \
                     connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'"
                        .to_string(),
                ),
            ],
            Body::from(file.data.into_owned()),
        )
            .into_response(),
    )
}

fn missing() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "admin console was not built into this binary (run `npm run build` in console/ before cargo build)",
    )
        .into_response()
}

async fn saml_metadata(State(state): State<AppState>) -> Response {
    match state.saml.metadata() {
        Ok(xml) => (
            [(header::CONTENT_TYPE, "application/samlmetadata+xml")],
            xml,
        )
            .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn saml_login(State(state): State<AppState>) -> Result<Response, ApiError> {
    use crate::services::saml::STATE_COOKIE as SAML_STATE;
    let start = state.saml.begin_login()?;
    let mut r = Redirect::to(&start.redirect_url).into_response();
    r.headers_mut().insert(
        header::SET_COOKIE,
        cookie(
            SAML_STATE,
            &start.state_cookie,
            Some(600),
            state.oidc.secure_cookies(),
        )
        .parse()
        .expect("cookie header"),
    );
    Ok(r)
}

#[derive(Deserialize)]
struct SamlAcsForm {
    #[serde(rename = "SAMLResponse")]
    saml_response: String,
    #[serde(default, rename = "RelayState")]
    _relay_state: Option<String>,
}

/// Assertion consumer service: the IdP posts the signed response here.
async fn saml_acs(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<SamlAcsForm>,
) -> Response {
    use crate::services::saml::STATE_COOKIE as SAML_STATE;
    let secure = state.oidc.secure_cookies();
    let fail = |msg: String| {
        let mut r = Redirect::to(&format!(
            "/console/?error={}",
            url::form_urlencoded::byte_serialize(msg.as_bytes()).collect::<String>()
        ))
        .into_response();
        r.headers_mut().insert(
            header::SET_COOKIE,
            cookie(SAML_STATE, "", Some(0), secure)
                .parse()
                .expect("cookie header"),
        );
        r
    };
    let state_cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie_value(c, SAML_STATE));
    let user = match state.saml.finish_login(&form.saml_response, state_cookie) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "console saml login rejected");
            return fail(e.to_string());
        }
    };
    let token = match state.oidc.issue_session(&user) {
        Ok(t) => t,
        Err(e) => return fail(e.to_string()),
    };
    tracing::info!(email = %user.email, role = user.role.as_str(), "console saml login");
    let mut r = Redirect::to("/console/").into_response();
    let h = r.headers_mut();
    h.append(
        header::SET_COOKIE,
        cookie(SAML_STATE, "", Some(0), secure)
            .parse()
            .expect("cookie header"),
    );
    h.append(
        header::SET_COOKIE,
        cookie(
            SESSION_COOKIE,
            &token,
            Some(state.oidc.session_ttl().as_secs()),
            secure,
        )
        .parse()
        .expect("cookie header"),
    );
    r
}
