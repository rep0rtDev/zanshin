//! Administrative API, `/admin/v1`. Requires an admin API key.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zanshin_core::{
    apikey::{ApiKeyKind, Role},
    properties::PropertyMap,
    Error,
};
use zanshin_store::{
    api_keys::{ApiKeyRow, KeyOptions},
    audit::{AuditEntry, AuditFilter},
};

use crate::state::AppState;

use super::{auth::AdminAuth, error::ApiResult, v1::IdentityResponse};

/// `?tags=a,b` keeps only items carrying every listed label.
#[derive(Debug, Default, Deserialize)]
pub struct TagsQuery {
    #[serde(default)]
    pub tags: Option<String>,
}

impl TagsQuery {
    pub fn matches(&self, tags: &[String]) -> bool {
        match &self.tags {
            None => true,
            Some(t) => t
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .all(|x| tags.iter().any(|y| y == x)),
        }
    }
}

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
        .route("/live-events/occurrences", get(live_event_occurrences))
        .route(
            "/live-events",
            get(list_live_events).post(create_live_event),
        )
        .route(
            "/live-events/{key}",
            get(get_live_event)
                .put(update_live_event)
                .patch(update_live_event)
                .delete(delete_live_event),
        )
        .route("/messages", get(list_messages).post(create_message))
        .route(
            "/messages/{key}",
            get(get_message)
                .put(update_message_def)
                .patch(update_message_def)
                .delete(delete_message_def),
        )
        .route("/messages/{key}/stats", get(message_stats))
        .route(
            "/identities/{identity_id}/messages",
            axum::routing::post(send_direct_message),
        )
        .route("/analytics/events", get(analytics_timeseries))
        .route("/analytics/events/names", get(analytics_names))
        .route("/identities/{identity_id}/events", get(identity_events))
        .route("/push/outbox", get(push_outbox))
        .route("/push/stats", get(push_stats))
        .route("/push/sweep", axum::routing::post(push_sweep))
        .route(
            "/identities/{identity_id}/push-tokens",
            get(identity_push_tokens),
        )
        .route("/stats", get(stats))
        .route("/me", get(me))
        .route("/audit", get(list_audit))
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    pub kind: ApiKeyKind,
    /// Admin keys only: GET-only access to `/admin/v1` (same as `role: viewer`).
    #[serde(default)]
    pub read_only: bool,
    /// Admin keys only: `admin` (default) | `editor` | `viewer`.
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Per-key request budget, overrides `limits.*_per_minute` for this key.
    #[serde(default)]
    pub rate_limit_per_minute: Option<i32>,
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
    let (key, secret) = state
        .api_keys
        .create(
            &req.name,
            req.kind,
            KeyOptions {
                read_only: req.read_only,
                expires_at: req.expires_at,
                rate_limit_per_minute: req.rate_limit_per_minute,
                role: req.role,
            },
        )
        .await?;
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
    let _ = state.events.delete_identity(id).await;
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
    Query(tq): Query<TagsQuery>,
) -> ApiResult<Json<Vec<Flag>>> {
    Ok(Json(
        state
            .flags
            .list()
            .await
            .iter()
            .filter(|f| tq.matches(&f.tags))
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
    Query(tq): Query<TagsQuery>,
) -> ApiResult<Json<Vec<Audience>>> {
    Ok(Json(
        state
            .audiences
            .list()
            .await
            .iter()
            .filter(|a| tq.matches(&a.tags))
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
    Query(tq): Query<TagsQuery>,
) -> ApiResult<Json<Vec<Experiment>>> {
    Ok(Json(
        state
            .experiments
            .list()
            .await
            .iter()
            .filter(|e| tq.matches(&e.tags))
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

#[derive(Debug, Default, Deserialize)]
pub struct ResultsQuery {
    /// Event name to measure per variant (e.g. `purchase`).
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

async fn experiment_results(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<ResultsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let (exp, stats) = state.experiments.results(&key).await?;
    let total: i64 = stats.iter().map(|s| s.assigned).sum();
    let metric = match &q.metric {
        Some(m) => {
            let to = q.to.unwrap_or_else(chrono::Utc::now);
            let from = q.from.unwrap_or(exp.created_at);
            Some((
                m.clone(),
                state
                    .events
                    .experiment_metric(&exp.key, m, from, to)
                    .await?,
            ))
        }
        None => None,
    };
    let catalog_metrics = state
        .metrics
        .for_experiment(
            &exp,
            &stats,
            q.from.unwrap_or(exp.created_at),
            q.to.unwrap_or_else(chrono::Utc::now),
        )
        .await;
    Ok(Json(serde_json::json!({
        "experiment": exp.key, "status": exp.status, "control": exp.control, "total_assigned": total,
        "winner": exp.winner, "interrupted_at": exp.interrupted_at, "interrupt_reason": exp.interrupt_reason,
        "locked": exp.locked, "max_participants": exp.max_participants, "enrol_until": exp.enrol_until,
        "metrics": catalog_metrics,
        "metric": metric.as_ref().map(|(m, _)| m.clone()),
        "variants": exp.variants.iter().map(|v| {
            let s = stats.iter().find(|s| s.variant == v.key);
            let assigned = s.map(|s| s.assigned).unwrap_or(0);
            let mut row = serde_json::json!({
                "variant": v.key, "weight": v.weight,
                "assigned": assigned,
                "exposed": s.map(|s| s.exposed).unwrap_or(0),
                "exposures": s.map(|s| s.exposures).unwrap_or(0),
            });
            if let Some((_, rows)) = &metric {
                let m = rows.iter().find(|r| r.variant == v.key);
                let identities = m.map(|m| m.identities).unwrap_or(0);
                row["metric"] = serde_json::json!({
                    "count": m.map(|m| m.count).unwrap_or(0),
                    "identities": identities,
                    "sum": m.map(|m| m.sum).unwrap_or(0.0),
                    "conversion": if assigned > 0 { identities as f64 / assigned as f64 } else { 0.0 },
                });
            }
            row
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

// ---- live events ---------------------------------------------------------------------------

use crate::services::{
    live_events::LiveEventPatch,
    messages::{DirectMessage, MessagePatch},
};
use zanshin_core::{live_events::LiveEvent, messages::MessageCampaign};

#[derive(Debug, Deserialize)]
pub struct CreateLiveEventRequest {
    pub key: String,
    #[serde(flatten)]
    pub patch: LiveEventPatch,
}

async fn list_live_events(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(tq): Query<TagsQuery>,
) -> ApiResult<Json<Vec<LiveEvent>>> {
    Ok(Json(
        state
            .live_events
            .list()
            .await
            .iter()
            .filter(|e| tq.matches(&e.tags))
            .map(|e| (**e).clone())
            .collect(),
    ))
}

async fn create_live_event(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateLiveEventRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<LiveEvent>)> {
    Ok((
        axum::http::StatusCode::CREATED,
        Json(state.live_events.create(&req.key, req.patch).await?),
    ))
}

async fn get_live_event(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<LiveEvent>> {
    Ok(Json((*state.live_events.get(&key).await?).clone()))
}

async fn update_live_event(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(patch): Json<LiveEventPatch>,
) -> ApiResult<Json<LiveEvent>> {
    Ok(Json(state.live_events.update(&key, patch).await?))
}

async fn delete_live_event(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.live_events.delete(&key).await?;
    Ok(Json(serde_json::json!({})))
}

// ---- messages ------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateMessageRequest {
    pub key: String,
    #[serde(flatten)]
    pub patch: MessagePatch,
}

async fn list_messages(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(tq): Query<TagsQuery>,
) -> ApiResult<Json<Vec<MessageCampaign>>> {
    Ok(Json(
        state
            .messages
            .list()
            .await
            .iter()
            .filter(|m| tq.matches(&m.tags))
            .map(|m| (**m).clone())
            .collect(),
    ))
}

async fn create_message(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateMessageRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<MessageCampaign>)> {
    Ok((
        axum::http::StatusCode::CREATED,
        Json(state.messages.create(&req.key, req.patch).await?),
    ))
}

async fn get_message(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<MessageCampaign>> {
    Ok(Json((*state.messages.get(&key).await?).clone()))
}

async fn update_message_def(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(patch): Json<MessagePatch>,
) -> ApiResult<Json<MessageCampaign>> {
    Ok(Json(state.messages.update(&key, patch).await?))
}

async fn delete_message_def(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.messages.delete(&key).await?;
    Ok(Json(serde_json::json!({})))
}

async fn message_stats(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (m, (delivered, read, consumed)) = state.messages.stats(&key).await?;
    Ok(Json(
        serde_json::json!({ "message": m.key, "delivered": delivered, "read": read, "consumed": consumed }),
    ))
}

async fn send_direct_message(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
    Json(msg): Json<DirectMessage>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    let identity = state.identity.get(identity_id).await?;
    let id = state.messages.send_direct(&identity, msg).await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({ "id": id })),
    ))
}

// ---- analytics ------------------------------------------------------------------------------

use zanshin_store::events::Granularity;

#[derive(Debug, Deserialize)]
pub struct TimeseriesQuery {
    pub name: String,
    #[serde(default)]
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub granularity: Option<Granularity>,
}

#[derive(Debug, Default, Deserialize)]
pub struct NamesQuery {
    #[serde(default)]
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub limit: Option<i64>,
}

fn window(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
) -> ApiResult<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    let to = to.unwrap_or_else(chrono::Utc::now);
    let from = from.unwrap_or(to - chrono::Duration::days(7));
    if from >= to {
        return Err(Error::invalid("from must be before to").into());
    }
    Ok((from, to))
}

async fn analytics_timeseries(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<TimeseriesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let (from, to) = window(q.from, q.to)?;
    let g = q.granularity.unwrap_or(Granularity::Day);
    let buckets = state.events.timeseries(&q.name, from, to, g).await?;
    Ok(Json(
        serde_json::json!({ "name": q.name, "from": from, "to": to, "granularity": g, "backend": state.events.backend(), "buckets": buckets }),
    ))
}

async fn analytics_names(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let (from, to) = window(q.from, q.to)?;
    let names = state
        .events
        .top_names(from, to, q.limit.unwrap_or(50))
        .await?;
    Ok(Json(
        serde_json::json!({ "from": from, "to": to, "backend": state.events.backend(), "names": names }),
    ))
}

#[derive(Debug, Default, Deserialize)]
pub struct LimitQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn identity_events(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    state.identity.get(identity_id).await?;
    let events = state
        .events
        .recent_for_identity(identity_id, q.limit.unwrap_or(100))
        .await?;
    Ok(Json(
        serde_json::json!({ "identity_id": identity_id, "events": events }),
    ))
}

// ---- push ----------------------------------------------------------------------------------

async fn push_outbox(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::json!({ "mode": state.push.mode(), "sent": state.push.outbox(q.limit.unwrap_or(100).clamp(1, 1000) as usize).await }),
    ))
}

async fn push_stats(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let (tokens, identities) = state.push.stats().await?;
    Ok(Json(
        serde_json::json!({ "mode": state.push.mode(), "tokens": tokens, "identities": identities }),
    ))
}

/// Deliver live push campaigns to every identity with a device right now (also runs every minute
/// and after each campaign change).
async fn push_sweep(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let identities = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone());
    let n = state
        .messages
        .sweep_push_campaigns(&state.eval, &identities)
        .await?;
    Ok(Json(serde_json::json!({ "delivered": n })))
}

async fn identity_push_tokens(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    state.identity.get(identity_id).await?;
    Ok(Json(
        serde_json::json!({ "tokens": state.push.tokens(identity_id).await? }),
    ))
}

// ---- audit log -------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub after: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    /// Path prefix, e.g. `/admin/v1/flags`.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
}

async fn list_audit(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Page<AuditEntry>>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let items = state
        .audit
        .list(&AuditFilter {
            after: q.after,
            limit,
            actor: q.actor,
            method: q.method.map(|m| m.to_ascii_uppercase()),
            path_prefix: q.path,
            since: q.since,
        })
        .await?;
    let next = if items.len() as i64 >= limit {
        items.last().map(|e| e.id)
    } else {
        None
    };
    Ok(Json(Page { items, next }))
}

// ---- live event calendar ----------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct OccurrencesQuery {
    #[serde(default)]
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OccurrenceRow {
    pub key: String,
    pub name: String,
    pub enabled: bool,
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
}

/// All occurrences of (enabled or disabled) live events overlapping `[from, to)` — the data behind
/// the console calendar. Defaults to the next 31 days; at most 500 occurrences per event.
async fn live_event_occurrences(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<OccurrencesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let from = q.from.unwrap_or_else(chrono::Utc::now);
    let to = q.to.unwrap_or(from + chrono::Duration::days(31));
    if to <= from || to - from > chrono::Duration::days(400) {
        return Err(Error::invalid("window must be positive and at most 400 days").into());
    }
    let mut rows = Vec::new();
    for ev in state.live_events.list().await {
        if q.key.as_ref().is_some_and(|k| k != &ev.key) {
            continue;
        }
        let mut cursor = from;
        if let Some(o) = ev.current(from) {
            rows.push(OccurrenceRow {
                key: ev.key.clone(),
                name: ev.name.clone(),
                enabled: ev.enabled,
                start: o.start,
                end: o.end,
            });
            cursor = o.start;
        }
        let mut n = 0;
        while let Some(o) = ev.next(cursor) {
            if o.start >= to || n >= 500 {
                break;
            }
            rows.push(OccurrenceRow {
                key: ev.key.clone(),
                name: ev.name.clone(),
                enabled: ev.enabled,
                start: o.start,
                end: o.end,
            });
            cursor = o.start;
            n += 1;
        }
    }
    rows.sort_by_key(|r| r.start);
    Ok(Json(
        serde_json::json!({ "from": from, "to": to, "occurrences": rows }),
    ))
}

#[derive(Serialize)]
pub struct Me {
    pub name: String,
    /// `api_key`, `bootstrap` or `oidc`.
    pub auth: &'static str,
    pub role: Role,
    pub read_only: bool,
}

async fn me(AdminAuth(p): AdminAuth) -> Json<Me> {
    let auth = match (p.key_id, p.name.as_str()) {
        (Some(_), _) => "api_key",
        (None, "bootstrap") => "bootstrap",
        (None, _) => "oidc",
    };
    Json(Me {
        name: p.name,
        auth,
        role: p.role,
        read_only: p.role == Role::Viewer,
    })
}
