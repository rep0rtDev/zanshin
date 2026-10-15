//! Taxonomy admin endpoints (mounted under `/admin/v1`): labels, event definitions and the
//! rejected-event debugger, value schemas, property configuration, instance settings,
//! play sessions and identity merges.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::Error;
use zanshin_store::taxonomy::{
    EventDefinition, EventDefinitionInput, IdentityMerge, Label, LabelInput, PlaySession,
    PropertyConfig, PropertyConfigInput, ValueSchema, ValueSchemaInput,
};

use crate::services::taxonomy::{RejectedEvent, TaxonomyMode, TaxonomyService};
use crate::state::AppState;

use super::{auth::AdminAuth, error::ApiResult};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/labels", get(list_labels).post(create_label))
        .route(
            "/labels/{id}",
            axum::routing::put(update_label).delete(delete_label),
        )
        .route("/labels/{id}/usage", get(label_usage))
        .route(
            "/taxonomy/events",
            get(list_event_defs).post(create_event_def),
        )
        .route(
            "/taxonomy/events/{name}",
            get(get_event_def)
                .put(upsert_event_def)
                .delete(delete_event_def),
        )
        .route(
            "/taxonomy/rejected",
            get(list_rejected).delete(clear_rejected),
        )
        .route("/taxonomy/properties", get(list_properties))
        .route(
            "/taxonomy/properties/{key}",
            axum::routing::put(upsert_property).delete(delete_property),
        )
        .route("/taxonomy/settings", get(get_settings).put(put_settings))
        .route("/schemas", get(list_schemas).post(create_schema))
        .route(
            "/schemas/{id}",
            get(get_schema).put(update_schema).delete(delete_schema),
        )
        .route("/schemas/validate", post(validate_schema))
        .route("/schemas/presets", post(create_schema_presets))
        .route("/identities/{id}/sessions", get(identity_sessions))
        .route("/identities/{id}/merges", get(identity_merges))
}

fn svc(state: &AppState) -> &TaxonomyService {
    &state.taxonomy
}

// ---- labels ---------------------------------------------------------------------------------

async fn list_labels(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<Label>>> {
    Ok(Json(svc(&state).repo.list_labels().await?))
}

async fn create_label(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(input): Json<LabelInput>,
) -> ApiResult<Json<Label>> {
    Ok(Json(svc(&state).repo.create_label(input).await?))
}

async fn update_label(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
    Json(input): Json<LabelInput>,
) -> ApiResult<Json<Label>> {
    let l = svc(&state).repo.update_label(id, input).await?;
    state.bus.reload_all().await;
    Ok(Json(l))
}

async fn delete_label(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    svc(&state).repo.delete_label(id).await?;
    state.bus.reload_all().await;
    Ok(Json(serde_json::json!({})))
}

#[derive(Serialize)]
struct Usage {
    table: String,
    count: i64,
}

async fn label_usage(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Usage>>> {
    let labels = svc(&state).repo.list_labels().await?;
    let Some(l) = labels.into_iter().find(|l| l.id == id) else {
        return Err(Error::not_found("label").into());
    };
    let usage = svc(&state).repo.label_usage(&l.name).await?;
    Ok(Json(
        usage
            .into_iter()
            .map(|(table, count)| Usage { table, count })
            .collect(),
    ))
}

// ---- event definitions ----------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct DefFilter {
    /// Only core (`true`) or only custom (`false`) definitions.
    #[serde(default)]
    core: Option<bool>,
    /// Only synthetic `$…` events (`true`) or hide them (`false`).
    #[serde(default)]
    synthetic: Option<bool>,
    #[serde(default)]
    discovered: Option<bool>,
    #[serde(default)]
    q: Option<String>,
}

async fn list_event_defs(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(f): Query<DefFilter>,
) -> ApiResult<Json<Vec<EventDefinition>>> {
    let q = f.q.unwrap_or_default().to_lowercase();
    let items = svc(&state)
        .repo
        .list_event_definitions()
        .await?
        .into_iter()
        .filter(|d| f.core.is_none_or(|c| d.is_core == c))
        .filter(|d| f.synthetic.is_none_or(|s| d.name.starts_with('$') == s))
        .filter(|d| f.discovered.is_none_or(|x| d.discovered == x))
        .filter(|d| {
            q.is_empty()
                || d.name.to_lowercase().contains(&q)
                || d.description.to_lowercase().contains(&q)
        })
        .collect();
    Ok(Json(items))
}

#[derive(Deserialize)]
struct CreateEventDef {
    name: String,
    #[serde(flatten)]
    rest: EventDefinitionInput,
}

async fn create_event_def(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(input): Json<CreateEventDef>,
) -> ApiResult<Json<EventDefinition>> {
    zanshin_core::events::EventInput {
        name: input.name.clone(),
        ..Default::default()
    }
    .validate(false)?;
    if svc(&state)
        .repo
        .get_event_definition(&input.name)
        .await?
        .is_some_and(|d| !d.discovered)
    {
        return Err(Error::conflict("event definition already exists").into());
    }
    let d = svc(&state)
        .repo
        .upsert_event_definition(&input.name, input.rest)
        .await?;
    svc(&state).invalidate();
    Ok(Json(d))
}

async fn get_event_def(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(name): Path<String>,
) -> ApiResult<Json<EventDefinition>> {
    let d = svc(&state)
        .repo
        .get_event_definition(&name)
        .await?
        .ok_or_else(|| Error::not_found("event definition"))?;
    Ok(Json(d))
}

async fn upsert_event_def(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(name): Path<String>,
    Json(input): Json<EventDefinitionInput>,
) -> ApiResult<Json<EventDefinition>> {
    zanshin_core::events::EventInput {
        name: name.clone(),
        ..Default::default()
    }
    .validate(false)?;
    let d = svc(&state)
        .repo
        .upsert_event_definition(&name, input)
        .await?;
    svc(&state).invalidate();
    Ok(Json(d))
}

async fn delete_event_def(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    svc(&state).repo.delete_event_definition(&name).await?;
    svc(&state).invalidate();
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

async fn list_rejected(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<LimitQuery>,
) -> ApiResult<Json<Vec<RejectedEvent>>> {
    Ok(Json(svc(&state).rejected(q.limit).await?))
}

async fn clear_rejected(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    svc(&state).clear_rejected().await?;
    Ok(Json(serde_json::json!({})))
}

// ---- properties -----------------------------------------------------------------------------

#[derive(Serialize)]
struct ObservedProperty {
    key: String,
    json_type: String,
    identities: i64,
}

#[derive(Serialize)]
struct PropertiesResponse {
    config: Vec<PropertyConfig>,
    default: Vec<ObservedProperty>,
    computed: Vec<ObservedProperty>,
    custom: Vec<ObservedProperty>,
}

async fn list_properties(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<PropertiesResponse>> {
    let repo = &svc(&state).repo;
    let sample = 20_000;
    let (config, d, c, u) = tokio::try_join!(
        repo.list_property_config(),
        repo.observed_properties("default", sample),
        repo.observed_properties("computed", sample),
        repo.observed_properties("custom", sample),
    )?;
    let conv = |v: Vec<(String, String, i64)>| {
        v.into_iter()
            .map(|(key, json_type, identities)| ObservedProperty {
                key,
                json_type,
                identities,
            })
            .collect()
    };
    Ok(Json(PropertiesResponse {
        config,
        default: conv(d),
        computed: conv(c),
        custom: conv(u),
    }))
}

async fn upsert_property(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(input): Json<PropertyConfigInput>,
) -> ApiResult<Json<PropertyConfig>> {
    let mut m = zanshin_core::properties::PropertyMap::new();
    m.insert(key.clone(), Value::Null);
    zanshin_core::properties::validate(&m)?;
    let p = svc(&state).repo.upsert_property_config(&key, input).await?;
    svc(&state).invalidate();
    Ok(Json(p))
}

async fn delete_property(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    svc(&state).repo.delete_property_config(&key).await?;
    svc(&state).invalidate();
    Ok(Json(serde_json::json!({})))
}

// ---- settings -------------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct TaxonomySettings {
    mode: TaxonomyMode,
    #[serde(default)]
    auto_discover: bool,
    #[serde(default)]
    geoip_enabled: bool,
    #[serde(default)]
    session_inactivity_secs: u64,
}

async fn get_settings(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<TaxonomySettings>> {
    let s = svc(&state);
    Ok(Json(TaxonomySettings {
        mode: s.mode().await,
        auto_discover: s.config().auto_discover,
        geoip_enabled: s.geo_enabled(),
        session_inactivity_secs: s.config().session_inactivity.as_secs(),
    }))
}

#[derive(Deserialize)]
struct PutSettings {
    mode: TaxonomyMode,
}

async fn put_settings(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Json(input): Json<PutSettings>,
) -> ApiResult<Json<TaxonomySettings>> {
    svc(&state).set_mode(input.mode).await?;
    get_settings(State(state), AdminAuth(p)).await
}

// ---- schemas --------------------------------------------------------------------------------

async fn list_schemas(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Vec<ValueSchema>>> {
    Ok(Json(svc(&state).repo.list_schemas().await?))
}

/// Built-in validators (Satori parity): ANY / Number / String / Object / Revenue metadata.
/// Idempotent — presets that already exist by name are left untouched.
pub const SCHEMA_PRESETS: &[(&str, &str, &str)] = &[
    ("ANY", "Accepts any value; no validation.", r#"{}"#),
    ("Number", "Expects a numeric value.", r#"{"type":"number"}"#),
    ("String", "Expects a string.", r#"{"type":"string"}"#),
    (
        "Object",
        "Expects a JSON object (arbitrary keys / values).",
        r#"{"type":"object"}"#,
    ),
    (
        "Event-Revenue-Metadata",
        "Revenue events: `amount` and ISO-4217 `currency` are required so USD conversion and revenue reports work; optional `test`, `product_id`, `store`.",
        r#"{"type":"object","required":["amount","currency"],"properties":{"amount":{"type":"number","minimum":0},"currency":{"type":"string","pattern":"^[A-Z]{3}$"},"test":{"type":"boolean"},"product_id":{"type":"string"},"store":{"type":"string"}}}"#,
    ),
];

async fn create_schema_presets(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<serde_json::Value>> {
    let existing: std::collections::HashSet<String> = svc(&state)
        .repo
        .list_schemas()
        .await?
        .into_iter()
        .map(|s| s.name)
        .collect();
    let mut created = Vec::new();
    for (name, description, schema) in SCHEMA_PRESETS {
        if existing.contains(*name) {
            continue;
        }
        let schema: Value = serde_json::from_str(schema).expect("preset schema");
        svc(&state)
            .repo
            .create_schema(ValueSchemaInput {
                name: name.to_string(),
                description: description.to_string(),
                schema,
            })
            .await?;
        created.push(*name);
    }
    if !created.is_empty() {
        svc(&state).invalidate();
    }
    Ok(Json(serde_json::json!({
        "created": created,
        "skipped": SCHEMA_PRESETS.len() - created.len(),
    })))
}

async fn create_schema(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(input): Json<ValueSchemaInput>,
) -> ApiResult<Json<ValueSchema>> {
    TaxonomyService::dry_run_schema(&input.schema, &[])?;
    let s = svc(&state).repo.create_schema(input).await?;
    svc(&state).invalidate();
    Ok(Json(s))
}

#[derive(Serialize)]
struct SchemaWithUsage {
    #[serde(flatten)]
    schema: ValueSchema,
    flags: i64,
    live_events: i64,
}

async fn get_schema(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SchemaWithUsage>> {
    let schema = svc(&state)
        .repo
        .get_schema(id)
        .await?
        .ok_or_else(|| Error::not_found("schema"))?;
    let (flags, live_events) = svc(&state).repo.schema_usage(id).await?;
    Ok(Json(SchemaWithUsage {
        schema,
        flags,
        live_events,
    }))
}

async fn update_schema(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
    Json(input): Json<ValueSchemaInput>,
) -> ApiResult<Json<ValueSchema>> {
    TaxonomyService::dry_run_schema(&input.schema, &[])?;
    let s = svc(&state).repo.update_schema(id, input).await?;
    svc(&state).invalidate();
    Ok(Json(s))
}

async fn delete_schema(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    svc(&state).repo.delete_schema(id).await?;
    svc(&state).invalidate();
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
struct ValidateRequest {
    schema: Value,
    #[serde(default)]
    samples: Vec<Value>,
}

#[derive(Serialize)]
struct ValidateResponse {
    results: Vec<Option<String>>,
}

async fn validate_schema(
    AdminAuth(_): AdminAuth,
    Json(req): Json<ValidateRequest>,
) -> ApiResult<Json<ValidateResponse>> {
    let results = TaxonomyService::dry_run_schema(&req.schema, &req.samples)?;
    Ok(Json(ValidateResponse { results }))
}

// ---- identity extras ------------------------------------------------------------------------

#[derive(Deserialize)]
struct SessionsQuery {
    #[serde(default = "default_sessions_limit")]
    limit: i64,
}

fn default_sessions_limit() -> i64 {
    50
}

async fn identity_sessions(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
    Query(q): Query<SessionsQuery>,
) -> ApiResult<Json<Vec<PlaySession>>> {
    Ok(Json(
        svc(&state)
            .repo
            .sessions_for_identity(id, q.limit.clamp(1, 500))
            .await?,
    ))
}

async fn identity_merges(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<IdentityMerge>>> {
    Ok(Json(svc(&state).repo.merges_into(id).await?))
}
