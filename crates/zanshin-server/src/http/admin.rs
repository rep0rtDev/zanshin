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
        .route("/flags", get(list_flags).post(create_flag))
        .route(
            "/flags/{key}",
            get(get_flag)
                .put(update_flag)
                .patch(update_flag)
                .delete(delete_flag),
        )
        .route("/flags/{key}/overrides", get(list_overrides))
        .route(
            "/flags/{key}/overrides/{identity_id}",
            axum::routing::put(set_override).delete(delete_override),
        )
        .route(
            "/flags/{key}/evaluate/{identity_id}",
            get(evaluate_for_identity),
        )
        .route(
            "/flags/{key}/evaluate",
            axum::routing::post(evaluate_dry_run),
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

// ---- flags ---------------------------------------------------------------------------------

use crate::services::flags::FlagPatch;
use zanshin_core::flags::{Evaluation, Flag};

#[derive(Debug, Deserialize)]
pub struct CreateFlagRequest {
    pub key: String,
    #[serde(flatten)]
    pub patch: FlagPatch,
}

async fn list_flags(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<Flag>>> {
    Ok(Json(
        state
            .flags
            .list()
            .await
            .iter()
            .map(|f| (**f).clone())
            .collect(),
    ))
}

async fn create_flag(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateFlagRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<Flag>)> {
    let f = state.flags.create(&req.key, req.patch.into()).await?;
    Ok((axum::http::StatusCode::CREATED, Json(f)))
}

async fn get_flag(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Flag>> {
    Ok(Json((*state.flags.get(&key).await?).clone()))
}

async fn update_flag(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(patch): Json<FlagPatch>,
) -> ApiResult<Json<Flag>> {
    Ok(Json(state.flags.update(&key, patch.into()).await?))
}

async fn delete_flag(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.flags.delete(&key).await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Serialize)]
pub struct OverrideRow {
    pub identity_id: Uuid,
    pub external_id: String,
    pub value: serde_json::Value,
}

async fn list_overrides(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<OverrideRow>>> {
    let rows = state.flags.list_overrides(&key, q.limit).await?;
    Ok(Json(
        rows.into_iter()
            .map(|(identity_id, external_id, value)| OverrideRow {
                identity_id,
                external_id,
                value,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct OverrideRequest {
    pub value: serde_json::Value,
}

async fn set_override(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
    Json(req): Json<OverrideRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .flags
        .set_override(&key, identity_id, req.value)
        .await?;
    Ok(Json(serde_json::json!({})))
}

async fn delete_override(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    state.flags.delete_override(&key, identity_id).await?;
    Ok(Json(serde_json::json!({})))
}

async fn evaluate_for_identity(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Evaluation>> {
    let identity = state.identity.get(identity_id).await?;
    Ok(Json(state.flags.evaluate_one(&identity, &key).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct DryRunRequest {
    /// Subject used for bucketing (an external id); defaults to "dry-run".
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub properties: zanshin_core::PropertySet,
}

async fn evaluate_dry_run(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(req): Json<DryRunRequest>,
) -> ApiResult<Json<Evaluation>> {
    state.flags.get(&key).await?;
    let now = chrono::Utc::now();
    let identity = zanshin_core::Identity {
        id: Uuid::nil(),
        external_id: req.subject.unwrap_or_else(|| "dry-run".into()),
        properties: req.properties.clone(),
        created_at: now,
        updated_at: now,
        last_seen_at: now,
    };
    let mut evals = state
        .flags
        .evaluate_with(
            &identity,
            &req.properties,
            std::slice::from_ref(&key),
            &Default::default(),
        )
        .await;
    evals
        .pop()
        .map(Json)
        .ok_or_else(|| Error::not_found(format!("flag {key:?} not found")).into())
}
