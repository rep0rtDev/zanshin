//! Admin API: export sinks, configuration bundles / history / undo, data management tools.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{auth::AdminAuth, error::ApiResult};
use crate::{
    services::{
        config::{self, ImportMode, ImportRequest},
        data::{self, PurgeRequest, SampleRequest},
        export::SinkPatch,
    },
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export-sinks", get(list_sinks).post(create_sink))
        .route(
            "/export-sinks/{key}",
            get(get_sink).put(update_sink).delete(delete_sink),
        )
        .route("/export-sinks/{key}/test", post(test_sink))
        .route("/export-sinks/{key}/run", post(run_sink))
        .route("/export-sinks/{key}/runs", get(sink_runs))
        .route("/export-sinks/{key}/reset-cursor", post(reset_cursor))
        .route("/config/export", get(export_config))
        .route("/config/diff", post(diff_config))
        .route("/config/import", post(import_config))
        .route("/config/import-from", post(import_from))
        .route("/config/history", get(history))
        .route("/config/history/{id}/revert", post(revert))
        .route("/config/kinds", get(kinds))
        .route("/audit/{id}/undo", post(undo))
        .route("/data/identities/{id}", axum::routing::delete(gdpr_delete))
        .route("/data/identities/{id}/takeout", get(takeout))
        .route("/data/events/purge", post(purge_events))
        .route("/data/sample", post(sample))
}

async fn list_sinks(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.export.list().await? })))
}

#[derive(Deserialize)]
struct CreateSink {
    key: String,
    #[serde(flatten)]
    patch: SinkPatch,
}

async fn create_sink(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<CreateSink>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let s = state.export.create(&b.key, b.patch).await?;
    config_note(&state, "export_sinks", &s.key, Some(&json!(s))).await;
    Ok((StatusCode::CREATED, Json(json!(s))))
}

async fn get_sink(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.export.get(&key).await?)))
}

async fn update_sink(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<SinkPatch>,
) -> ApiResult<Json<Value>> {
    let s = state.export.update(&key, p).await?;
    config_note(&state, "export_sinks", &key, Some(&json!(s))).await;
    Ok(Json(json!(s)))
}

async fn delete_sink(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    state.export.delete(&key).await?;
    config_note(&state, "export_sinks", &key, None).await;
    Ok(Json(json!({ "deleted": key })))
}

async fn config_note(state: &AppState, kind: &str, key: &str, body: Option<&Value>) {
    let version = body
        .and_then(|b| b.get("version"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let _ = state
        .config_history
        .insert(kind, key, version, body, body.is_none(), "")
        .await;
}

async fn test_sink(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(state.export.test(&key).await?))
}

async fn run_sink(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.export.run_now(&key).await?)))
}

#[derive(Deserialize)]
struct Limit {
    #[serde(default)]
    limit: Option<i64>,
}

async fn sink_runs(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<Limit>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "items": state.export.runs(&key, q.limit.unwrap_or(50)).await? }),
    ))
}

#[derive(Deserialize)]
struct ResetCursor {
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
}

async fn reset_cursor(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<ResetCursor>,
) -> ApiResult<Json<Value>> {
    state.export.reset_cursor(&key, b.to).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct Kinds {
    #[serde(default)]
    kinds: Option<String>,
    #[serde(default)]
    keys: Option<String>,
}

fn split_kinds(k: &Option<String>) -> Option<Vec<String>> {
    k.as_ref().map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
}

async fn export_config(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<Kinds>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        config::export_bundle(
            &state,
            split_kinds(&q.kinds).as_deref(),
            split_kinds(&q.keys).as_deref(),
        )
        .await?,
    ))
}

async fn kinds(AdminAuth(_): AdminAuth) -> Json<Value> {
    Json(json!({ "kinds": config::KINDS, "format": config::BUNDLE_FORMAT }))
}

#[derive(Deserialize)]
struct DiffBody {
    bundle: Value,
    #[serde(default)]
    mode: ImportMode,
    #[serde(default)]
    kinds: Option<Vec<String>>,
    #[serde(default)]
    keys: Option<Vec<String>>,
}

async fn diff_config(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<DiffBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        config::diff(
            &state,
            &b.bundle,
            b.mode,
            b.kinds.as_deref(),
            b.keys.as_deref(),
        )
        .await?,
    ))
}

async fn import_config(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<ImportRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(config::import(&state, &b).await?))
}

#[derive(Deserialize)]
struct ImportFrom {
    url: String,
    admin_key: String,
    #[serde(default)]
    mode: ImportMode,
    #[serde(default)]
    kinds: Option<Vec<String>>,
    #[serde(default)]
    dry_run: bool,
}

async fn import_from(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<ImportFrom>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        config::import_from(&state, &b.url, &b.admin_key, b.mode, b.kinds, b.dry_run).await?,
    ))
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn history(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "items": config::history(&state, q.kind.as_deref(), q.key.as_deref(), q.limit.unwrap_or(100)).await? }),
    ))
}

async fn revert(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(config::revert(&state, id).await?))
}

async fn undo(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(config::undo_audit(&state, id).await?))
}

async fn gdpr_delete(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(data::gdpr_delete(&state, id).await?))
}

async fn takeout(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(data::takeout(&state, id).await?))
}

async fn purge_events(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<PurgeRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(data::purge_events(&state, &b).await?))
}

async fn sample(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<SampleRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(data::generate_sample(&state, &b).await?))
}
