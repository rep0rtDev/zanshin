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
        .route("/api-keys/{id}/rotate", axum::routing::post(rotate_key))
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
        .route("/flags/{key}/diff", get(diff_flag_values))
        .route("/flags/{key}/sizes", get(flag_sizes))
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
        .route("/holdouts", get(list_holdouts).post(create_holdout))
        .route(
            "/holdouts/{key}",
            get(get_holdout).put(update_holdout).delete(delete_holdout),
        )
        .route(
            "/experiments/{key}/results.csv",
            get(experiment_results_csv),
        )
        .route("/experiments/{key}/phases", get(experiment_phases))
        .route("/live-events/{key}/metrics", get(live_event_metrics))
        .route("/live-events/{key}/runs", get(live_event_runs))
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
        .route("/system", get(system))
        .route("/onboarding", get(onboarding))
        .route("/health", get(health))
        .route("/system/config", get(system_config))
        .route(
            "/console/settings",
            get(console_settings).put(put_console_settings),
        )
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
    /// Admin keys only: explicit `resource -> [read, write, delete]` grants (override `role`).
    #[serde(default)]
    pub permissions: Option<zanshin_core::permissions::Permissions>,
    /// Admin keys only: named preset (`developer`, `game_designer`, …) used when `permissions` is absent.
    #[serde(default)]
    pub archetype: Option<String>,
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
                permissions: match (&req.permissions, &req.archetype) {
                    (Some(p), _) => {
                        p.validate().map_err(zanshin_core::Error::invalid)?;
                        Some(p.clone())
                    }
                    (None, Some(a)) => {
                        Some(zanshin_core::permissions::archetype(a).ok_or_else(|| {
                            zanshin_core::Error::invalid(format!("unknown archetype {a:?}"))
                        })?)
                    }
                    (None, None) => None,
                },
            },
        )
        .await?;
    Ok(Json(CreatedKey { key, secret }))
}

#[derive(Debug, Deserialize, Default)]
pub struct RotateKeyRequest {
    /// How long the old key keeps working (hours, default 24, max 720).
    #[serde(default)]
    pub grace_hours: Option<i64>,
}

async fn rotate_key(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
    body: Option<Json<RotateKeyRequest>>,
) -> ApiResult<Json<CreatedKey>> {
    let hours = body
        .map(|Json(b)| b.grace_hours.unwrap_or(24))
        .unwrap_or(24)
        .clamp(0, 720);
    let (key, secret) = state
        .api_keys
        .rotate(id, chrono::Duration::hours(hours))
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
    let before = state.flags.get(&key).await?;
    let after = state.flags.update(&key, patch.into()).await?;
    emit_rule_events(&state, &before, &after);
    Ok(Json(after))
}

/// Satori's `FEATURE_FLAG_VARIANT_{CREATED,UPDATED,DELETED}`: one webhook per targeting rule
/// (our "variant") that appeared, changed or vanished in this update.
fn emit_rule_events(state: &AppState, before: &Flag, after: &Flag) {
    let old: std::collections::HashMap<&str, &zanshin_core::flags::Rule> =
        before.rules.iter().map(|r| (r.id.as_str(), r)).collect();
    let new: std::collections::HashMap<&str, &zanshin_core::flags::Rule> =
        after.rules.iter().map(|r| (r.id.as_str(), r)).collect();
    let data = |r: &zanshin_core::flags::Rule| {
        serde_json::json!({
            "flag": after.key, "version": after.version,
            "rule": { "id": r.id, "name": r.name, "percentage": r.percentage,
                      "audiences": r.audiences, "variants": r.variants.iter().map(|v| &v.key).collect::<Vec<_>>() },
        })
    };
    for (id, r) in &new {
        match old.get(id) {
            None => state.webhooks.emit("flag.rule.created", data(r)),
            Some(o) if *o != *r => state.webhooks.emit("flag.rule.updated", data(r)),
            Some(_) => {}
        }
    }
    for (id, r) in &old {
        if !new.contains_key(id) {
            state.webhooks.emit("flag.rule.deleted", data(r));
        }
    }
}

async fn delete_flag(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::services::refs::ensure_unreferenced(&state, crate::services::refs::RefKind::Flag, &key)
        .await?;
    state.flags.delete(&key).await?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Serialize)]
pub struct OverrideRow {
    pub identity_id: Uuid,
    pub external_id: String,
    pub value: serde_json::Value,
}

#[derive(Deserialize)]
struct DiffQuery {
    a: String,
    b: String,
}

#[derive(Serialize)]
struct DiffSide {
    selector: String,
    bytes: usize,
    value: serde_json::Value,
}

#[derive(Serialize)]
struct FlagDiff {
    a: DiffSide,
    b: DiffSide,
    changes: Vec<zanshin_core::flags::ValueDiff>,
}

async fn diff_flag_values(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<FlagDiff>> {
    let flag = state.flags.get(&key).await?;
    let pick = |sel: &str| {
        flag.named_value(sel)
            .cloned()
            .ok_or_else(|| zanshin_core::Error::not_found(format!("flag value {sel:?}")))
    };
    let (a, b) = (pick(&q.a)?, pick(&q.b)?);
    Ok(Json(FlagDiff {
        changes: zanshin_core::flags::diff_values(&a, &b),
        a: DiffSide {
            selector: q.a,
            bytes: zanshin_core::flags::payload_bytes(&a),
            value: a,
        },
        b: DiffSide {
            selector: q.b,
            bytes: zanshin_core::flags::payload_bytes(&b),
            value: b,
        },
    }))
}

#[derive(Serialize)]
struct PayloadSize {
    selector: String,
    bytes: usize,
}

async fn flag_sizes(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Vec<PayloadSize>>> {
    let flag = state.flags.get(&key).await?;
    Ok(Json(
        flag.payload_sizes()
            .into_iter()
            .map(|(selector, bytes)| PayloadSize { selector, bytes })
            .collect(),
    ))
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
    crate::services::refs::ensure_unreferenced(
        &state,
        crate::services::refs::RefKind::Audience,
        &key,
    )
    .await?;
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
    crate::services::refs::ensure_unreferenced(
        &state,
        crate::services::refs::RefKind::Experiment,
        &key,
    )
    .await?;
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
    /// Add CUPED variance-reduced estimates for count / sum / avg metrics.
    #[serde(default)]
    pub cuped: Option<bool>,
    /// Pre-experiment covariate window in days (default 14).
    #[serde(default)]
    pub pre_days: Option<i64>,
}

// ---- holdouts -------------------------------------------------------------------------------

async fn list_holdouts(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let items = state.experiments.list_holdouts().await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
struct CreateHoldoutRequest {
    key: String,
    #[serde(flatten)]
    patch: crate::services::experiments::HoldoutPatch,
}

async fn create_holdout(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(req): Json<CreateHoldoutRequest>,
) -> ApiResult<(
    axum::http::StatusCode,
    Json<zanshin_core::experiments::Holdout>,
)> {
    let h = state
        .experiments
        .create_holdout(&req.key, req.patch)
        .await?;
    Ok((axum::http::StatusCode::CREATED, Json(h)))
}

async fn get_holdout(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let h = state.experiments.get_holdout(&key).await?;
    let used_by = state.experiments.holdouts.used_by(&key).await?;
    Ok(Json(
        serde_json::json!({ "holdout": h, "used_by": used_by }),
    ))
}

async fn update_holdout(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(patch): Json<crate::services::experiments::HoldoutPatch>,
) -> ApiResult<Json<zanshin_core::experiments::Holdout>> {
    Ok(Json(state.experiments.update_holdout(&key, patch).await?))
}

async fn delete_holdout(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.experiments.delete_holdout(&key).await?;
    Ok(Json(serde_json::json!({ "deleted": key })))
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
    let cuped = if q.cuped == Some(true) {
        Some(
            state
                .metrics
                .cuped_for_experiment(
                    &exp,
                    q.from.unwrap_or(exp.created_at),
                    q.to.unwrap_or_else(chrono::Utc::now),
                    q.pre_days.unwrap_or(14),
                )
                .await,
        )
    } else {
        None
    };
    let srm = zanshin_core::stats::srm(
        &exp.variants
            .iter()
            .map(|v| {
                (
                    v.weight,
                    stats
                        .iter()
                        .find(|s| s.variant == v.key)
                        .map(|s| s.assigned)
                        .unwrap_or(0),
                )
            })
            .collect::<Vec<_>>(),
    );
    Ok(Json(serde_json::json!({
        "experiment": exp.key, "status": exp.status, "control": exp.control, "total_assigned": total,
        "winner": exp.winner, "interrupted_at": exp.interrupted_at, "interrupt_reason": exp.interrupt_reason,
        "locked": exp.locked, "max_participants": exp.max_participants, "enrol_until": exp.enrol_until,
        "srm": srm,
        "metrics": catalog_metrics,
        "cuped": cuped,
        "layer": exp.layer,
        "holdout": exp.holdout,
        "guardrails": exp.guardrails,
        "bandit": exp.bandit,
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

async fn experiment_phases(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let phases = state.experiments.phases(&key).await?;
    let items: Vec<serde_json::Value> = phases
        .iter()
        .enumerate()
        .map(|(i, e)| {
            serde_json::json!({
                "phase": i + 1, "key": e.key, "name": e.name, "status": e.status, "phase_of": e.phase_of,
                "starts_at": e.starts_at, "ends_at": e.ends_at, "audiences": e.audiences,
                "variants": e.variants.iter().map(|v| &v.key).collect::<Vec<_>>(),
                "winner": e.winner, "created_at": e.created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

fn csv_cell(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Results as CSV: one row per (metric, variant) plus assignment rows.
async fn experiment_results_csv(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<ResultsQuery>,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (exp, stats) = state.experiments.results(&key).await?;
    let metrics = state
        .metrics
        .for_experiment(
            &exp,
            &stats,
            q.from.unwrap_or(exp.created_at),
            q.to.unwrap_or_else(chrono::Utc::now),
        )
        .await;
    let mut out = String::from(
        "experiment,metric,aggregation,kind,variant,is_control,assigned,exposed,exposures,value,lift,ci_low,ci_high,prob_beats_control\n",
    );
    for v in &exp.variants {
        let s = stats.iter().find(|s| s.variant == v.key);
        let base = [
            exp.key.clone(),
            String::new(),
            String::new(),
            String::new(),
            v.key.clone(),
            (exp.control.as_deref() == Some(&v.key)).to_string(),
            s.map(|s| s.assigned).unwrap_or(0).to_string(),
            s.map(|s| s.exposed).unwrap_or(0).to_string(),
            s.map(|s| s.exposures).unwrap_or(0).to_string(),
        ];
        out.push_str(&base.join(","));
        out.push_str(",,,,,\n");
        for m in &metrics {
            let Some(row) = m["variants"]
                .as_array()
                .and_then(|a| a.iter().find(|r| r["variant"] == v.key))
            else {
                continue;
            };
            let cells = [
                csv_cell(&serde_json::json!(exp.key)),
                csv_cell(&m["metric"]),
                csv_cell(&m["aggregation"]),
                csv_cell(&m["kind"]),
                csv_cell(&serde_json::json!(v.key)),
                (exp.control.as_deref() == Some(&v.key)).to_string(),
                s.map(|s| s.assigned).unwrap_or(0).to_string(),
                s.map(|s| s.exposed).unwrap_or(0).to_string(),
                s.map(|s| s.exposures).unwrap_or(0).to_string(),
                csv_cell(&row["value"]),
                csv_cell(&row["lift"]),
                csv_cell(&row["stats"]["ci_low"]),
                csv_cell(&row["stats"]["ci_high"]),
                csv_cell(&row["stats"]["prob_beats_control"]),
            ];
            out.push_str(&cells.join(","));
            out.push('\n');
        }
    }
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/csv; charset=utf-8".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}-results.csv\"", exp.key),
            ),
        ],
        out,
    ))
}

#[derive(Debug, Default, serde::Deserialize)]
struct LiveMetricsQuery {
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    metric: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Monitor metrics of a live event (per variant) over a window, plus a daily series for `metric`.
async fn live_event_metrics(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<LiveMetricsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let ev = state.live_events.get(&key).await?;
    let to = q.to.unwrap_or_else(chrono::Utc::now);
    let from = q.from.unwrap_or(ev.starts_at.min(to));
    let metrics = state.metrics.for_live_event(&ev, from, to).await;
    let series = match q
        .metric
        .as_deref()
        .or(ev.metrics.first().map(|s| s.as_str()))
    {
        Some(m) => Some(state.metrics.live_series(&ev, m, from, to).await?),
        None => None,
    };
    Ok(Json(serde_json::json!({
        "live_event": ev.key, "from": from, "to": to, "variants": ev.variants.iter().map(|v| &v.key).collect::<Vec<_>>(),
        "metrics": metrics, "series": series,
    })))
}

/// Run report: every past / current occurrence with its monitor metrics, newest first.
async fn live_event_runs(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<LiveMetricsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let ev = state.live_events.get(&key).await?;
    let now = chrono::Utc::now();
    let mut occurrences = Vec::new();
    let mut cursor = ev.starts_at - chrono::Duration::seconds(1);
    while let Some(o) = ev.current(cursor).or_else(|| ev.next(cursor)) {
        if o.start > now || occurrences.len() >= 500 {
            break;
        }
        cursor = o.end + chrono::Duration::seconds(1);
        occurrences.push(o);
    }
    occurrences.reverse();
    occurrences.truncate(q.limit.unwrap_or(12).clamp(1, 100));
    let mut runs = Vec::new();
    for (i, o) in occurrences.iter().enumerate() {
        let to = o.end.min(now);
        let metrics = state.metrics.for_live_event(&ev, o.start, to).await;
        runs.push(serde_json::json!({
            "run": occurrences.len() - i, "start": o.start, "end": o.end, "live": o.start <= now && now < o.end,
            "metrics": metrics,
        }));
    }
    Ok(Json(
        serde_json::json!({ "live_event": ev.key, "runs": runs }),
    ))
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
    crate::services::refs::ensure_unreferenced(
        &state,
        crate::services::refs::RefKind::LiveEvent,
        &key,
    )
    .await?;
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

#[derive(Deserialize)]
struct IdentityEventsQuery {
    limit: Option<i64>,
    name: Option<String>,
    q: Option<String>,
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
}

async fn identity_events(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(identity_id): Path<Uuid>,
    Query(q): Query<IdentityEventsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    state.identity.get(identity_id).await?;
    let events = state
        .events
        .search_for_identity(
            identity_id,
            &zanshin_store::events::EventSearch {
                name: q.name.clone(),
                q: q.q.clone(),
                from: q.from,
                to: q.to,
            },
            q.limit.unwrap_or(100),
        )
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
    /// `api_key`, `bootstrap`, `oidc` or `user`.
    pub auth: &'static str,
    pub role: Role,
    pub read_only: bool,
    pub permissions: zanshin_core::permissions::Permissions,
    pub user_id: Option<Uuid>,
    pub mfa_enabled: bool,
    pub must_change_password: bool,
    /// Console language saved for this console user (`PUT /admin/v1/me/locale`); null = browser default.
    pub locale: Option<String>,
}

/// Effective server configuration (defaults + file + environment) with secrets masked —
/// Settings → Configuration in the console, exportable as JSON.
async fn system_config(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "config": state.cfg.redacted(),
        "sources": ["defaults", "zanshin.yml (if present)", "ZANSHIN__SECTION__KEY environment", "ZANSHIN_FILE__SECTION__KEY secret files"],
    })))
}

const CONSOLE_SETTINGS_KEY: &str = "console.settings";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConsoleSettings {
    /// IANA timezone used to display every date in the console for all users; empty = browser's.
    #[serde(default)]
    timezone: String,
    /// Default date range (days) for analytics pages.
    #[serde(default)]
    default_range_days: Option<u32>,
}

async fn console_settings(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<ConsoleSettings>> {
    let v = state
        .taxonomy
        .repo
        .get_setting(CONSOLE_SETTINGS_KEY)
        .await?;
    Ok(Json(
        v.and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
    ))
}

async fn put_console_settings(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(input): Json<ConsoleSettings>,
) -> ApiResult<Json<ConsoleSettings>> {
    if !input.timezone.is_empty() {
        let ok = input.timezone.len() <= 64
            && input
                .timezone
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'));
        if !ok {
            return Err(zanshin_core::Error::invalid(
                "timezone must be an IANA name like Europe/Berlin",
            )
            .into());
        }
    }
    if input
        .default_range_days
        .is_some_and(|d| !(1..=365).contains(&d))
    {
        return Err(zanshin_core::Error::invalid("default_range_days must be 1..365").into());
    }
    state
        .taxonomy
        .repo
        .set_setting(
            CONSOLE_SETTINGS_KEY,
            &serde_json::to_value(&input).map_err(zanshin_core::Error::from)?,
        )
        .await?;
    Ok(Json(input))
}

async fn system(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let cfg = &state.cfg;
    let stores_ok = state.stores.ping().await.is_ok();
    let now = chrono::Utc::now();
    let count = |r: zanshin_core::Result<usize>| r.unwrap_or(0);
    let counts = serde_json::json!({
        "flags": state.flags.list().await.len(),
        "audiences": state.audiences.list().await.len(),
        "experiments": state.experiments.list().await.len(),
        "live_events": state.live_events.list().await.len(),
        "messages": state.messages.list().await.len(),
        "templates": count(state.templates.list().await.map(|v| v.len())),
        "journeys": count(state.journeys.list().await.map(|v| v.len())),
        "webhooks": count(state.webhooks.list().await.map(|v| v.len())),
        "integrations": count(state.integrations.list().await.map(|v| v.len())),
        "metrics": count(state.metrics.list().await.map(|v| v.len())),
    });
    Ok(Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build": crate::build_info::json(),
        "started_at": state.started_at,
        "uptime_secs": (now - state.started_at).num_seconds().max(0),
        "stores": {
            "postgres": stores_ok,
            "redis": stores_ok,
            "clickhouse": state.stores.clickhouse.is_some(),
            "event_store": if state.stores.clickhouse.is_some() { "clickhouse" } else { "postgres" },
        },
        "push": { "provider": cfg.push.provider },
        "console": {
            "audience_delete_all_enabled": cfg.admin.audience_delete_all_enabled && !cfg.server.production,
        },
        "auth": {
            "oidc": cfg.admin.oidc.is_some(),
            "saml": state.saml.enabled(),
            "scim": cfg.admin.scim.as_ref().is_some_and(|s| !s.token().is_empty()),
            "access_ttl_secs": cfg.auth.access_ttl.as_secs(),
            "refresh_ttl_secs": cfg.auth.refresh_ttl.as_secs(),
        },
        "limits": {
            "enabled": if cfg.limits.enabled { 1 } else { 0 },
            "ip_per_minute": cfg.limits.ip_per_minute,
            "client_per_minute": cfg.limits.client_per_minute,
            "session_per_minute": cfg.limits.session_per_minute,
            "server_per_minute": cfg.limits.server_per_minute,
        },
        "taxonomy": {
            "mode": cfg.taxonomy.mode,
            "auto_discover": cfg.taxonomy.auto_discover,
            "geoip": !cfg.taxonomy.geoip_db.is_empty(),
        },
        "audit_retention_days": cfg.audit.retention.as_secs() / 86_400,
        "counts": counts,
    })))
}

async fn me(State(state): State<AppState>, AdminAuth(p): AdminAuth) -> ApiResult<Json<Me>> {
    let auth = match (p.key_id, p.user_id, p.name.as_str()) {
        (Some(_), _, _) => "api_key",
        (None, Some(_), _) => "user",
        (None, None, "bootstrap") => "bootstrap",
        (None, None, _) => "oidc",
    };
    let user = match p.user_id {
        Some(id) => Some(state.console_users.repo().get(id).await?),
        None => None,
    };
    let permissions = p.permissions();
    Ok(Json(Me {
        read_only: !permissions.can_write_anything(),
        name: p.name,
        auth,
        role: p.role,
        permissions,
        user_id: p.user_id,
        mfa_enabled: user.as_ref().is_some_and(|u| u.totp_enabled),
        must_change_password: user.as_ref().is_some_and(|u| u.must_change_password),
        locale: user.as_ref().and_then(|u| u.locale.clone()),
    }))
}

/// Day-1 checklist computed from real data: which first steps a fresh deployment still has ahead.
async fn health(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(crate::services::health::overview(&state).await?))
}

async fn onboarding(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let identities = zanshin_store::identities::IdentityRepo::new(state.stores.pg.clone())
        .count()
        .await?;
    let users = zanshin_store::console_users::ConsoleUserRepo::new(state.stores.pg.clone())
        .count()
        .await?;
    let keys = state.api_keys.list().await?;
    let channels = state.notifications.list().await?;
    let cfg = &state.cfg;
    let step = |id: &str, title: &str, done: bool, href: &str, hint: &str| serde_json::json!({ "id": id, "title": title, "done": done, "href": href, "hint": hint });
    let steps = vec![
        step("server_key", "Create a server API key", keys.iter().any(|k| k.kind == "server"), "/api-keys",
            "Your game backend (or the Nakama module) authenticates with a server key."),
        step("first_identity", "Connect a backend — first player seen", identities > 0, "/identities",
            "Drop the Nakama module in or call POST /server/v1/identities from any SDK."),
        step("first_flag", "Create a feature flag", !state.flags.list().await.is_empty(), "/flags",
            "Flags are evaluated per player with rules, rollouts and variants."),
        step("first_audience", "Define an audience", !state.audiences.list().await.is_empty(), "/audiences",
            "Audiences target flags, experiments, live events and messages."),
        step("teammate", "Invite a teammate", users > 1, "/users",
            "Console users with archetypes replace the bootstrap key."),
        step("sso", "Set up SSO (OIDC or SAML)", cfg.admin.oidc.is_some() || cfg.admin.saml.is_some(), "/settings",
            "Configure admin.oidc or admin.saml; SCIM provisioning is optional."),
        step("notifications", "Connect a notification channel (Slack / Discord / webhook)", !channels.is_empty(), "/notifications",
            "Config changes, alerts and guardrail pauses are posted there."),
        step("backups", "Verify a backup restore", std::env::var("ZANSHIN_ONBOARDING_BACKUPS_OK").is_ok(), "/settings",
            "Run restore-drill.sh from the backup kit; set ZANSHIN_ONBOARDING_BACKUPS_OK=1 once it is scheduled."),
    ];
    let done = steps.iter().filter(|s| s["done"] == true).count();
    Ok(Json(
        serde_json::json!({ "steps": steps, "done": done, "total": steps.len() }),
    ))
}
