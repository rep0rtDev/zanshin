//! Administrative API, `/admin/v1`. Requires an admin API key.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zanshin_core::{apikey::ApiKeyKind, properties::PropertyMap, Error};
use zanshin_store::api_keys::ApiKeyRow;

use crate::state::AppState;

use super::{auth::AdminAuth, error::ApiResult, v1::IdentityResponse};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api-keys", get(list_keys).post(create_key))
        .route("/api-keys/{id}", axum::routing::delete(revoke_key))
        .route("/identities", get(list_identities))
        .route(
            "/identities/{id}",
            get(get_identity).delete(delete_identity),
        )
        .route(
            "/identities/{id}/properties",
            axum::routing::put(update_identity_properties),
        )
        .route(
            "/identities/by-external-id/{external_id}",
            get(get_identity_by_external),
        )
        .route("/stats", get(stats))
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    pub kind: ApiKeyKind,
}

#[derive(Debug, Serialize)]
pub struct CreatedKey {
    #[serde(flatten)]
    pub key: ApiKeyRow,
    /// Shown exactly once.
    pub secret: String,
}

async fn create_key(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateKeyRequest>,
) -> ApiResult<Json<CreatedKey>> {
    let (key, secret) = state.api_keys.create(&req.name, req.kind).await?;
    Ok(Json(CreatedKey { key, secret }))
}

async fn list_keys(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<ApiKeyRow>>> {
    Ok(Json(state.api_keys.list().await?))
}

async fn revoke_key(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiKeyRow>> {
    Ok(Json(state.api_keys.revoke(id).await?))
}

#[derive(Debug, Serialize)]
pub struct AdminIdentity {
    #[serde(flatten)]
    pub identity: IdentityResponse,
    pub properties: zanshin_core::PropertySet,
}

impl From<zanshin_core::Identity> for AdminIdentity {
    fn from(i: zanshin_core::Identity) -> Self {
        Self {
            identity: IdentityResponse::from(&i),
            properties: i.properties,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub after: Option<Uuid>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Uuid>,
}

async fn list_identities(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<AdminIdentity>>> {
    let items = state.identity.list(q.after, q.limit).await?;
    let next = if items.len() as i64 >= q.limit.clamp(1, 500) {
        items.last().map(|i| i.id)
    } else {
        None
    };
    Ok(Json(Page {
        items: items.into_iter().map(Into::into).collect(),
        next,
    }))
}

async fn get_identity(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AdminIdentity>> {
    Ok(Json(state.identity.get(id).await?.into()))
}

async fn get_identity_by_external(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(external_id): Path<String>,
) -> ApiResult<Json<AdminIdentity>> {
    Ok(Json(
        state
            .identity
            .get_by_external_id(&external_id)
            .await?
            .into(),
    ))
}

async fn delete_identity(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    state.identity.delete(id).await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdatePropsRequest {
    #[serde(default)]
    pub default: PropertyMap,
    #[serde(default)]
    pub custom: PropertyMap,
}

async fn update_identity_properties(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePropsRequest>,
) -> ApiResult<Json<AdminIdentity>> {
    if req.default.is_empty() && req.custom.is_empty() {
        return Err(Error::invalid("nothing to update").into());
    }
    Ok(Json(
        state
            .identity
            .update_properties(id, req.default, req.custom)
            .await?
            .into(),
    ))
}

async fn stats(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let identities = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone())
        .count()
        .await?;
    Ok(Json(serde_json::json!({
        "identities": identities,
        "uptime_seconds": (chrono::Utc::now() - state.started_at).num_seconds(),
        "clickhouse_enabled": state.stores.clickhouse.is_some(),
    })))
}
