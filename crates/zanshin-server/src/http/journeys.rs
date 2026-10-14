//! Admin API: journeys (CRUD), runs, manual entry.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{journeys::Journey, Error};

use super::{auth::AdminAuth, error::ApiResult};
use crate::{services::journeys::JourneyPatch, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/journeys", get(list).post(create))
        .route("/journeys/{key}", get(get_one).put(update).delete(delete))
        .route("/journeys/{key}/enter", post(enter))
        .route("/journeys/{key}/runs", get(runs))
        .route("/journeys/{key}/runs/{id}", get(run_detail))
        .route("/journeys/{key}/runs/{id}/exit", post(exit_run))
        .route("/identities/{id}/journeys", get(identity_runs))
}

async fn list(State(state): State<AppState>, AdminAuth(_): AdminAuth) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.journeys.list().await? })))
}

#[derive(Debug, Deserialize)]
struct Create {
    key: String,
    #[serde(flatten)]
    patch: JourneyPatch,
}

async fn create(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<Create>,
) -> ApiResult<(axum::http::StatusCode, Json<Journey>)> {
    let j = state.journeys.create(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(j)))
}

async fn get_one(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Journey>> {
    Ok(Json(state.journeys.get(&key).await?))
}

async fn update(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<JourneyPatch>,
) -> ApiResult<Json<Journey>> {
    Ok(Json(state.journeys.update(&key, p).await?))
}

async fn delete(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    state.journeys.delete(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

#[derive(Debug, Default, Deserialize)]
struct Target {
    #[serde(default)]
    identity_id: Option<Uuid>,
    #[serde(default)]
    external_id: Option<String>,
}

async fn enter(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(t): Json<Target>,
) -> ApiResult<Json<Value>> {
    let identity = match (&t.identity_id, &t.external_id) {
        (Some(id), _) => state.identity.get(*id).await?,
        (None, Some(ext)) => state.identity.get_by_external_id(ext).await?,
        _ => return Err(Error::invalid("identity_id or external_id is required").into()),
    };
    Ok(Json(json!(state.journeys.enter(&key, &identity).await?)))
}

#[derive(Debug, Default, Deserialize)]
struct RunsQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    after: Option<Uuid>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn runs(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<RunsQuery>,
) -> ApiResult<Json<Value>> {
    let (j, items, by_status, by_step) = state
        .journeys
        .runs(&key, q.status.as_deref(), q.after, q.limit.unwrap_or(100))
        .await?;
    let next = items.last().map(|r| r.id);
    let steps: Vec<Value> = j
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let active = by_step
                .iter()
                .find(|(st, _)| *st as usize == i)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            json!({ "index": i, "id": s.id, "active": active })
        })
        .collect();
    let by_status: Vec<Value> = by_status
        .into_iter()
        .map(|(s, n)| json!({ "status": s, "count": n }))
        .collect();
    Ok(Json(
        json!({ "items": items, "next": next, "stats": by_status, "steps": steps }),
    ))
}

async fn run_detail(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Value>> {
    let (run, log) = state.journeys.run_detail(&key, id).await?;
    Ok(Json(json!({ "run": run, "log": log })))
}

async fn exit_run(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(state.journeys.exit_run(&key, id).await?)))
}

async fn identity_runs(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let items: Vec<Value> = state
        .journeys
        .runs_for_identity(id)
        .await?
        .into_iter()
        .map(|(key, run)| json!({ "journey": key, "run": run }))
        .collect();
    Ok(Json(json!({ "items": items })))
}
