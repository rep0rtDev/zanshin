//! Console users, the caller's own account (`/me/*`) and the permission catalog.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zanshin_core::{
    permissions::{Permissions, ARCHETYPES, RESOURCES},
    Error,
};
use zanshin_store::console_users::{ConsoleSessionRow, ConsoleUserRow, UserPatch};

use crate::state::AppState;

use super::{auth::AdminAuth, error::ApiResult};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/permissions", get(permissions_catalog))
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/{id}",
            get(get_user).patch(update_user).delete(delete_user),
        )
        .route("/users/{id}/reset-link", post(reset_link))
        .route("/users/{id}/mfa", delete(admin_disable_mfa))
        .route("/users/{id}/sessions", delete(admin_revoke_sessions))
        .route("/me/password", post(change_password))
        .route("/me/mfa/setup", post(mfa_setup))
        .route("/me/mfa/enable", post(mfa_enable))
        .route("/me/mfa/disable", post(mfa_disable))
        .route("/me/sessions", get(my_sessions))
        .route("/me/sessions/{id}", delete(revoke_my_session))
}

#[derive(Serialize)]
struct Catalog {
    resources: &'static [&'static str],
    actions: [&'static str; 3],
    archetypes: Vec<ArchetypeOut>,
}

#[derive(Serialize)]
struct ArchetypeOut {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    permissions: Permissions,
}

async fn permissions_catalog(AdminAuth(_): AdminAuth) -> Json<Catalog> {
    Json(Catalog {
        resources: RESOURCES,
        actions: ["read", "write", "delete"],
        archetypes: ARCHETYPES
            .iter()
            .map(|a| ArchetypeOut {
                name: a.name,
                title: a.title,
                description: a.description,
                permissions: zanshin_core::permissions::archetype(a.name).unwrap_or_default(),
            })
            .collect(),
    })
}

#[derive(Serialize)]
struct UserOut {
    #[serde(flatten)]
    user: ConsoleUserRow,
    recovery_codes_left: usize,
}

fn out(user: ConsoleUserRow) -> UserOut {
    UserOut {
        recovery_codes_left: user.recovery_codes.len(),
        user,
    }
}

async fn list_users(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<UserOut>>> {
    Ok(Json(
        state
            .console_users
            .repo()
            .list()
            .await?
            .into_iter()
            .map(out)
            .collect(),
    ))
}

async fn get_user(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<UserOut>> {
    Ok(Json(out(state.console_users.repo().get(id).await?)))
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub name: String,
    /// Initial password (the user is asked to change it on first login). Omit to hand out a
    /// reset link instead.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub archetype: Option<String>,
    #[serde(default)]
    pub permissions: Option<Permissions>,
}

#[derive(Serialize)]
struct CreatedUser {
    #[serde(flatten)]
    user: UserOut,
    /// Present when no password was supplied: one-time link token for `POST /console/auth/reset`.
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_token: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<CreatedUser>)> {
    let user = state
        .console_users
        .create(
            &req.email,
            &req.name,
            req.password.as_deref(),
            req.archetype.as_deref(),
            req.permissions,
            &p.name,
        )
        .await?;
    let reset_token = if req.password.is_none() {
        Some(state.console_users.issue_reset(user.id).await?)
    } else {
        None
    };
    Ok((
        StatusCode::CREATED,
        Json(CreatedUser {
            user: out(user),
            reset_token,
        }),
    ))
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub permissions: Option<Permissions>,
    /// `null` clears the preset (custom permissions); a name applies that preset.
    #[serde(default, deserialize_with = "serde_with_double_option::deserialize")]
    pub archetype: Option<Option<String>>,
    #[serde(default)]
    pub disabled: Option<bool>,
}

mod serde_with_double_option {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<String>>, D::Error> {
        Option::<String>::deserialize(d).map(Some)
    }
}

async fn update_user(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> ApiResult<Json<UserOut>> {
    if p.user_id == Some(id)
        && (req.disabled == Some(true) || req.permissions.is_some() || req.archetype.is_some())
    {
        return Err(
            Error::invalid("you cannot change your own permissions or disable yourself").into(),
        );
    }
    let user = state
        .console_users
        .update(
            id,
            UserPatch {
                name: req.name,
                permissions: req.permissions,
                archetype: None,
                disabled: req.disabled,
            },
            req.archetype.as_ref().map(|a| a.as_deref()),
        )
        .await?;
    Ok(Json(out(user)))
}

async fn delete_user(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    if p.user_id == Some(id) {
        return Err(Error::invalid("you cannot delete yourself").into());
    }
    state.console_users.repo().delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_link(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let token = state.console_users.issue_reset(id).await?;
    Ok(Json(serde_json::json!({
        "reset_token": token,
        "path": format!("/console/reset?token={token}"),
        "expires_in_secs": 86_400,
    })))
}

async fn admin_disable_mfa(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let user = state.console_users.repo().get(id).await?;
    state.console_users.mfa_disable(&user, "", true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_revoke_sessions(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let n = state.console_users.repo().revoke_all_sessions(id).await?;
    Ok(Json(serde_json::json!({ "revoked": n })))
}

async fn current_user(
    state: &AppState,
    p: &crate::services::apikeys::Principal,
) -> ApiResult<ConsoleUserRow> {
    let id = p.user_id.ok_or_else(|| {
        Error::denied("this endpoint is for console user accounts (password login)")
    })?;
    Ok(state.console_users.repo().get(id).await?)
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    let user = current_user(&state, &p).await?;
    state
        .console_users
        .change_password(&user, &req.current_password, &req.new_password)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mfa_setup(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let user = current_user(&state, &p).await?;
    let setup = state.console_users.mfa_setup(&user).await?;
    Ok(Json(serde_json::json!({
        "secret": setup.secret,
        "otpauth_url": setup.otpauth_url,
    })))
}

#[derive(Deserialize)]
pub struct CodeRequest {
    pub code: String,
}

async fn mfa_enable(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Json(req): Json<CodeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let user = current_user(&state, &p).await?;
    let codes = state.console_users.mfa_enable(&user, &req.code).await?;
    Ok(Json(serde_json::json!({ "recovery_codes": codes })))
}

async fn mfa_disable(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Json(req): Json<CodeRequest>,
) -> ApiResult<StatusCode> {
    let user = current_user(&state, &p).await?;
    state
        .console_users
        .mfa_disable(&user, &req.code, false)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct SessionOut {
    id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
    last_seen_at: chrono::DateTime<chrono::Utc>,
    ip: Option<String>,
    user_agent: Option<String>,
}

impl From<ConsoleSessionRow> for SessionOut {
    fn from(s: ConsoleSessionRow) -> Self {
        Self {
            id: s.id,
            created_at: s.created_at,
            expires_at: s.expires_at,
            last_seen_at: s.last_seen_at,
            ip: s.ip,
            user_agent: s.user_agent,
        }
    }
}

async fn my_sessions(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
) -> ApiResult<Json<Vec<SessionOut>>> {
    let user = current_user(&state, &p).await?;
    Ok(Json(
        state
            .console_users
            .repo()
            .list_sessions(user.id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

async fn revoke_my_session(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let user = current_user(&state, &p).await?;
    state.console_users.logout(user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
