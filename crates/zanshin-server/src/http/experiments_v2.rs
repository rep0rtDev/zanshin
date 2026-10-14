//! Admin API: metrics catalog and experiments v2 (copy, lock, interrupt, participants, series, CSV).

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::metrics::Metric;

use super::{auth::AdminAuth, error::ApiResult};
use crate::{services::metrics::MetricPatch, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(list_metrics).post(create_metric))
        .route(
            "/metrics/{key}",
            get(get_metric).put(update_metric).delete(delete_metric),
        )
        .route("/experiments/{key}/copy", post(copy_experiment))
        .route("/experiments/{key}/lock", post(lock_experiment))
        .route("/experiments/{key}/unlock", post(unlock_experiment))
        .route("/experiments/{key}/interrupt", post(interrupt_experiment))
        .route("/experiments/{key}/participants", get(participants))
        .route("/experiments/{key}/participants.csv", get(participants_csv))
        .route("/experiments/{key}/series", get(series))
}

async fn list_metrics(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.metrics.list().await? })))
}

#[derive(Debug, Deserialize)]
struct CreateMetric {
    key: String,
    #[serde(flatten)]
    patch: MetricPatch,
}

async fn create_metric(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<CreateMetric>,
) -> ApiResult<(axum::http::StatusCode, Json<Metric>)> {
    let m = state.metrics.create(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(m)))
}

async fn get_metric(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let m = state.metrics.get(&key).await?;
    let used_by = state.metrics.used_by(&key).await?;
    Ok(Json(json!({ "metric": m, "used_by": used_by })))
}

async fn update_metric(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<MetricPatch>,
) -> ApiResult<Json<Metric>> {
    Ok(Json(state.metrics.update(&key, p).await?))
}

async fn delete_metric(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    crate::services::refs::ensure_unreferenced(
        &state,
        crate::services::refs::RefKind::Metric,
        &key,
    )
    .await?;
    state.metrics.delete(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

#[derive(Debug, Deserialize)]
struct CopyBody {
    key: String,
    #[serde(default)]
    as_phase: bool,
}

async fn copy_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<CopyBody>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let e = state.experiments.copy(&key, &b.key, b.as_phase).await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!(e))))
}

async fn lock_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let p = crate::services::experiments::ExperimentPatch {
        locked: Some(true),
        ..Default::default()
    };
    Ok(Json(json!(state.experiments.update(&key, p).await?)))
}

async fn unlock_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let p = crate::services::experiments::ExperimentPatch {
        locked: Some(false),
        ..Default::default()
    };
    Ok(Json(json!(state.experiments.update(&key, p).await?)))
}

#[derive(Debug, Default, Deserialize)]
struct InterruptBody {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    winner: Option<String>,
    /// Write the winner's value into the linked flag's default value.
    #[serde(default)]
    apply_to_flag: bool,
}

async fn interrupt_experiment(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(b): Json<InterruptBody>,
) -> ApiResult<Json<Value>> {
    let (e, won) = state
        .experiments
        .interrupt(&key, &b.reason, b.winner.as_deref())
        .await?;
    let mut flag_updated = None;
    if let (true, Some(v), Some(flag_key)) = (b.apply_to_flag, &won, &e.flag_key) {
        let patch = zanshin_store::flags::FlagInput {
            default_value: Some(v.value.clone()),
            ..Default::default()
        };
        let f = state.flags.update(flag_key, patch).await?;
        flag_updated = Some(json!({ "flag": f.key, "default_value": f.default_value }));
    }
    Ok(Json(json!({ "experiment": e, "flag": flag_updated })))
}

#[derive(Debug, Default, Deserialize)]
struct ParticipantsQuery {
    #[serde(default)]
    variant: Option<String>,
    #[serde(default)]
    after: Option<Uuid>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn participants(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<ParticipantsQuery>,
) -> ApiResult<Json<Value>> {
    let items = state
        .experiments
        .participants(&key, q.variant.as_deref(), q.after, q.limit.unwrap_or(100))
        .await?;
    let total = state.experiments.participant_count(&key).await?;
    let next = items.last().map(|p| p.identity_id);
    Ok(Json(
        json!({ "items": items, "next": next, "total": total }),
    ))
}

async fn participants_csv(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<ParticipantsQuery>,
) -> ApiResult<impl IntoResponse> {
    let mut out =
        String::from("identity_id,external_id,variant,assigned_at,exposed_at,exposures\n");
    let mut after = None;
    loop {
        let page = state
            .experiments
            .participants(&key, q.variant.as_deref(), after, 1000)
            .await?;
        if page.is_empty() {
            break;
        }
        for p in &page {
            out.push_str(&format!(
                "{},{},{},{},{},{}\n",
                p.identity_id,
                csv_escape(&p.external_id),
                csv_escape(&p.variant),
                p.assigned_at.to_rfc3339(),
                p.exposed_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                p.exposures
            ));
        }
        after = page.last().map(|p| p.identity_id);
        if page.len() < 1000 {
            break;
        }
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{key}-participants.csv\""),
            ),
        ],
        out,
    ))
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[derive(Debug, Default, Deserialize)]
struct SeriesQuery {
    #[serde(default)]
    metric: Option<String>,
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
}

/// Enrolment curve (assignments per variant per day) plus, when `metric` is given, that
/// metric's daily values per variant.
async fn series(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<SeriesQuery>,
) -> ApiResult<Json<Value>> {
    let exp = state.experiments.get(&key).await?;
    let to = q.to.unwrap_or_else(chrono::Utc::now);
    let from = q.from.unwrap_or(exp.created_at);
    let assignments: Vec<Value> = state
        .experiments
        .assignment_series(&key)
        .await?
        .into_iter()
        .map(|(variant, day, count)| json!({"variant": variant, "day": day, "count": count}))
        .collect();
    let metric = match &q.metric {
        Some(m) => Some(state.metrics.series(&exp, m, from, to).await?),
        None => None,
    };
    Ok(Json(
        json!({ "experiment": exp.key, "assignments": assignments, "metric": metric }),
    ))
}
