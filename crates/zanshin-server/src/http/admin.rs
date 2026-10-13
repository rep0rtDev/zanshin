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
        .route("/audiences", get(list_audiences).post(create_audience))
        .route(
            "/audiences/{key}",
            get(get_audience)
                .put(update_audience)
                .patch(update_audience)
                .delete(delete_audience),
        )
        .route(
            "/audiences/{key}/evaluate/{identity_id}",
            get(audience_for_identity),
        )
        .route(
            "/audiences/{key}/evaluate",
            axum::routing::post(audience_dry_run),
        )
        .route(
            "/experiments",
            get(list_experiments).post(create_experiment),
        )
        .route(
            "/experiments/{key}",
            get(get_experiment)
                .put(update_experiment)
                .patch(update_experiment)
                .delete(delete_experiment),
        )
        .route(
            "/experiments/{key}/start",
            axum::routing::post(start_experiment),
        )
        .route(
            "/experiments/{key}/pause",
            axum::routing::post(pause_experiment),
        )
        .route(
            "/experiments/{key}/stop",
            axum::routing::post(stop_experiment),
        )
        .route(
            "/experiments/{key}/reset",
            axum::routing::post(reset_experiment),
        )
        .route("/experiments/{key}/results", get(experiment_results))
        .route(
            "/experiments/{key}/assignments/{identity_id}",
            get(get_assignment)
                .put(force_assignment)
                .delete(delete_assignment),
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
    Ok(Json(state.eval.flag(&identity, &key).await?))
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
    Ok(Json(
        state
            .eval
            .dry_run_flag(
                req.subject.as_deref().unwrap_or("dry-run"),
                &req.properties,
                &key,
            )
            .await?,
    ))
}

// ---- audiences -----------------------------------------------------------------------------

use crate::services::{audiences::AudiencePatch, experiments::ExperimentPatch};
use zanshin_core::{
    audiences::Audience,
    experiments::{Experiment, Status},
};

#[derive(Debug, Deserialize)]
pub struct CreateAudienceRequest {
    pub key: String,
    #[serde(flatten)]
    pub patch: AudiencePatch,
}

async fn list_audiences(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<Audience>>> {
    Ok(Json(
        state
            .audiences
            .list()
            .await
            .iter()
            .map(|a| (**a).clone())
            .collect(),
    ))
}

async fn create_audience(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateAudienceRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<Audience>)> {
    Ok((
        axum::http::StatusCode::CREATED,
        Json(state.audiences.create(&req.key, req.patch).await?),
    ))
}

async fn get_audience(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Audience>> {
    Ok(Json((*state.audiences.get(&key).await?).clone()))
}

async fn update_audience(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(patch): Json<AudiencePatch>,
) -> ApiResult<Json<Audience>> {
    Ok(Json(state.audiences.update(&key, patch).await?))
}

async fn delete_audience(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.audiences.delete(&key).await?;
    Ok(Json(serde_json::json!({})))
}

async fn audience_for_identity(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    let identity = state.identity.get(identity_id).await?;
    let member = state.eval.audience_keys(&identity).await.contains(&key);
    state.audiences.get(&key).await?;
    Ok(Json(
        serde_json::json!({ "audience": key, "identity_id": identity_id, "member": member }),
    ))
}

async fn audience_dry_run(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(req): Json<DryRunRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let member = state
        .eval
        .dry_run_audience(
            req.subject.as_deref().unwrap_or("dry-run"),
            &req.properties,
            &key,
        )
        .await?;
    Ok(Json(
        serde_json::json!({ "audience": key, "member": member }),
    ))
}

// ---- experiments ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateExperimentRequest {
    pub key: String,
    #[serde(flatten)]
    pub patch: ExperimentPatch,
}

async fn list_experiments(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<Experiment>>> {
    Ok(Json(
        state
            .experiments
            .list()
            .await
            .iter()
            .map(|e| (**e).clone())
            .collect(),
    ))
}

async fn create_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateExperimentRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<Experiment>)> {
    Ok((
        axum::http::StatusCode::CREATED,
        Json(state.experiments.create(&req.key, req.patch).await?),
    ))
}

async fn get_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Experiment>> {
    Ok(Json((*state.experiments.get(&key).await?).clone()))
}

async fn update_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(patch): Json<ExperimentPatch>,
) -> ApiResult<Json<Experiment>> {
    Ok(Json(state.experiments.update(&key, patch).await?))
}

async fn delete_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.experiments.delete(&key).await?;
    Ok(Json(serde_json::json!({})))
}

async fn start_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Experiment>> {
    Ok(Json(
        state.experiments.set_status(&key, Status::Running).await?,
    ))
}

async fn pause_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Experiment>> {
    Ok(Json(
        state.experiments.set_status(&key, Status::Paused).await?,
    ))
}

async fn stop_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Experiment>> {
    Ok(Json(
        state.experiments.set_status(&key, Status::Stopped).await?,
    ))
}

async fn reset_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let n = state.experiments.reset(&key).await?;
    Ok(Json(serde_json::json!({ "deleted_assignments": n })))
}

async fn experiment_results(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (exp, stats) = state.experiments.results(&key).await?;
    let total: i64 = stats.iter().map(|s| s.assigned).sum();
    Ok(Json(serde_json::json!({
        "experiment": exp.key, "status": exp.status, "control": exp.control, "total_assigned": total,
        "variants": exp.variants.iter().map(|v| {
            let s = stats.iter().find(|s| s.variant == v.key);
            serde_json::json!({
                "variant": v.key, "weight": v.weight,
                "assigned": s.map(|s| s.assigned).unwrap_or(0),
                "exposed": s.map(|s| s.exposed).unwrap_or(0),
                "exposures": s.map(|s| s.exposures).unwrap_or(0),
            })
        }).collect::<Vec<_>>()
    })))
}

async fn get_assignment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    match state.experiments.assignment(&key, identity_id).await? {
        Some(a) => Ok(Json(serde_json::to_value(a).map_err(Error::internal)?)),
        None => Err(Error::not_found("identity is not assigned").into()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ForceAssignRequest {
    pub variant: String,
}

async fn force_assignment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
    Json(req): Json<ForceAssignRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .experiments
        .force_assign(&key, identity_id, &req.variant)
        .await?;
    Ok(Json(serde_json::json!({})))
}

async fn delete_assignment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, identity_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .experiments
        .delete_assignment(&key, identity_id)
        .await?;
    Ok(Json(serde_json::json!({})))
}
