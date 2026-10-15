//! Admin API for audiences v2: members, include / exclude lists, recompute, usage, copy and the
//! expression language. Mounted under `/admin/v1` next to the basic audience CRUD.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;
use zanshin_core::{audiences::Audience, rules::Condition};

use super::{auth::AdminAuth, error::ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/audiences/{key}/members",
            get(members).post(add_members).delete(remove_members),
        )
        .route(
            "/audiences/{key}/lists",
            get(lists).put(set_list).delete(unset_list),
        )
        .route(
            "/audiences/{key}/members/all",
            axum::routing::delete(delete_all_members),
        )
        .route("/audiences/{key}/recompute", post(recompute))
        .route("/audiences/{key}/usage", get(usage))
        .route("/audiences/{key}/copy", post(copy))
        .route("/audiences/{key}/expression", get(expression))
        .route("/audiences/expression", post(parse_expression))
}

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    pub after: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn members(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = state
        .audiences
        .members(&key, q.after, q.limit.unwrap_or(100))
        .await?;
    let next = items.last().map(|m| m.identity_id);
    Ok(Json(serde_json::json!({ "items": items, "next": next })))
}

#[derive(Debug, Deserialize)]
pub struct MembersBody {
    pub external_ids: Vec<String>,
}

async fn add_members(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<MembersBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let (n, missing) = state
        .audiences
        .set_managed(&key, &b.external_ids, true)
        .await?;
    Ok(Json(serde_json::json!({ "added": n, "missing": missing })))
}

async fn remove_members(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<MembersBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let (n, missing) = state
        .audiences
        .set_managed(&key, &b.external_ids, false)
        .await?;
    Ok(Json(
        serde_json::json!({ "removed": n, "missing": missing }),
    ))
}

async fn lists(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Vec<zanshin_store::audiences::ListEntry>>> {
    Ok(Json(state.audiences.lists(&key).await?))
}

#[derive(Debug, Deserialize)]
pub struct ListBody {
    pub external_id: String,
    /// `include` or `exclude`.
    pub mode: String,
    #[serde(default)]
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub reason: String,
}

async fn set_list(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<ListBody>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .audiences
        .set_list(&key, &b.external_id, &b.mode, b.until, &b.reason)
        .await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Deserialize)]
pub struct UnsetBody {
    pub external_id: String,
}

async fn unset_list(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<UnsetBody>,
) -> ApiResult<Json<serde_json::Value>> {
    state.audiences.unset_list(&key, &b.external_id).await?;
    Ok(Json(serde_json::json!({})))
}

async fn recompute(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let n = state.audiences.recompute(&key).await?;
    Ok(Json(serde_json::json!({ "member_count": n })))
}

async fn usage(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Vec<zanshin_store::audiences::UsageRef>>> {
    Ok(Json(state.audiences.usage(&key).await?))
}

#[derive(Debug, Deserialize)]
pub struct CopyBody {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
}

async fn copy(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<CopyBody>,
) -> ApiResult<(axum::http::StatusCode, Json<Audience>)> {
    let a = state.audiences.copy(&key, &b.key, b.name).await?;
    Ok((axum::http::StatusCode::CREATED, Json(a)))
}

/// Permanently deletes every identity in the audience (GDPR-style, events included). Gated by
/// `admin.audience_delete_all_enabled` and refused in production, like Satori's
/// `console.audience.is_delete_all_identities_enabled`.
async fn delete_all_members(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    if !state.cfg.admin.audience_delete_all_enabled || state.cfg.server.production {
        return Err(zanshin_core::Error::denied(
            "delete-all is disabled: set admin.audience_delete_all_enabled on a non-production server",
        )
        .into());
    }
    if !p.allows("identities", zanshin_core::permissions::Action::Delete) {
        return Err(zanshin_core::Error::denied("permission required: identities:delete").into());
    }
    state.audiences.get(&key).await?;
    let mut deleted = 0usize;
    let mut after = None;
    loop {
        let page = state.audiences.members(&key, after, 500).await?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|m| m.identity_id);
        for m in &page {
            if crate::services::data::gdpr_delete(&state, m.identity_id)
                .await
                .is_ok()
            {
                deleted += 1;
            }
        }
    }
    Ok(Json(
        serde_json::json!({ "audience": key, "deleted": deleted }),
    ))
}

async fn expression(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let a = state.audiences.get(&key).await?;
    Ok(Json(
        serde_json::json!({ "expression": zanshin_core::expr::format(&a.condition) }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ExpressionBody {
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// Convert between the expression language and the JSON condition (either direction).
async fn parse_expression(
    AdminAuth(_): AdminAuth,
    Json(b): Json<ExpressionBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let condition = match (b.expression, b.condition) {
        (Some(e), _) => zanshin_core::expr::parse(&e)?,
        (None, Some(c)) => {
            c.validate()?;
            c
        }
        (None, None) => Condition::always(),
    };
    let expression = zanshin_core::expr::format(&condition);
    Ok(Json(
        serde_json::json!({ "condition": condition, "expression": expression }),
    ))
}
