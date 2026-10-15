//! Flags v3 admin API: kill switch, scheduled changes, evaluation stats / stale detection and
//! change requests with approvals.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{permissions::Action, Error};

use super::{auth::AdminAuth, error::ApiResult};
use crate::{services::config, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/flags/stale", get(stale_flags))
        .route("/flags/{key}/kill", post(kill_flag))
        .route("/flags/{key}/restore", post(restore_flag))
        .route("/flags/{key}/stats", get(flag_stats))
        .route(
            "/flags/{key}/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/flags/{key}/schedules/{id}",
            axum::routing::delete(cancel_schedule),
        )
        .route(
            "/change-requests",
            get(list_change_requests).post(create_change_request),
        )
        .route("/change-requests/{id}", get(get_change_request))
        .route(
            "/change-requests/{id}/approve",
            post(approve_change_request),
        )
        .route("/change-requests/{id}/reject", post(reject_change_request))
        .route("/change-requests/{id}/apply", post(apply_change_request))
        .route("/change-requests/{id}/cancel", post(cancel_change_request))
}

// ---- kill switch --------------------------------------------------------------------------------

async fn set_enabled(
    state: &AppState,
    key: &str,
    enabled: bool,
    who: &str,
) -> ApiResult<Json<Value>> {
    let input = zanshin_store::flags::FlagInput {
        enabled: Some(enabled),
        ..Default::default()
    };
    let flag = state.flags.update(key, input).await?;
    state.webhooks.emit(
        if enabled {
            "flag.restored"
        } else {
            "flag.killed"
        },
        json!({ "flag": key, "version": flag.version, "by": who }),
    );
    Ok(Json(json!({ "flag": flag, "enabled": enabled })))
}

/// One-click kill switch: disable the flag so every client gets the default immediately.
async fn kill_flag(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    set_enabled(&state, &key, false, &p.name).await
}

async fn restore_flag(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    set_enabled(&state, &key, true, &p.name).await
}

// ---- schedules ----------------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateSchedule {
    at: chrono::DateTime<chrono::Utc>,
    patch: Value,
    #[serde(default)]
    note: String,
}

async fn list_schedules(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    state.flags.get(&key).await?;
    let items = state.flags_v3.schedules(&key).await?;
    Ok(Json(json!({ "items": items })))
}

async fn create_schedule(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(key): Path<String>,
    Json(req): Json<CreateSchedule>,
) -> ApiResult<(
    axum::http::StatusCode,
    Json<zanshin_store::flags_v3::FlagSchedule>,
)> {
    state.flags.get(&key).await?;
    // Validate the patch shape now so a typo does not fail silently at apply time.
    let _: crate::services::flags::FlagPatch = serde_json::from_value(req.patch.clone())
        .map_err(|e| Error::invalid(format!("patch: {e}")))?;
    if req.at < chrono::Utc::now() - chrono::Duration::minutes(1) {
        return Err(Error::invalid("schedule time is in the past").into());
    }
    let s = state
        .flags_v3
        .create_schedule(&key, req.at, &req.patch, &p.name, &req.note)
        .await?;
    Ok((axum::http::StatusCode::CREATED, Json(s)))
}

async fn cancel_schedule(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((key, id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Value>> {
    state.flags_v3.cancel_schedule(&key, id).await?;
    Ok(Json(json!({ "cancelled": id })))
}

// ---- evaluation stats ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DaysQuery {
    #[serde(default)]
    days: Option<i64>,
}

async fn flag_stats(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<DaysQuery>,
) -> ApiResult<Json<Value>> {
    state.flags.get(&key).await?;
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let rows = state.flags_v3.eval_stats(&key, days).await?;
    let total: i64 = rows.iter().map(|r| r.count).sum();
    let mut by_variant: std::collections::BTreeMap<String, i64> = Default::default();
    for r in &rows {
        *by_variant.entry(r.variant.clone()).or_default() += r.count;
    }
    let last_day = rows.iter().map(|r| r.day).max();
    Ok(Json(json!({
        "flag": key, "days": days, "total": total, "last_day": last_day,
        "by_variant": by_variant, "series": rows,
        "note": "counters flush every minute; stats start with this release",
    })))
}

/// Flags nobody evaluated in the window and that were not changed in it either.
async fn stale_flags(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<DaysQuery>,
) -> ApiResult<Json<Value>> {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let totals = state.flags_v3.eval_totals(days).await?;
    let active: std::collections::HashMap<&str, (i64, chrono::NaiveDate)> = totals
        .iter()
        .map(|(k, n, d)| (k.as_str(), (*n, *d)))
        .collect();
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let items: Vec<Value> = state
        .flags
        .list()
        .await
        .iter()
        .filter(|f| !active.contains_key(f.key.as_str()) && f.updated_at < cutoff)
        .map(|f| json!({ "key": f.key, "name": f.name, "enabled": f.enabled, "updated_at": f.updated_at, "tags": f.tags }))
        .collect();
    Ok(Json(
        json!({ "days": days, "items": items, "evaluated": active.len() }),
    ))
}

// ---- change requests ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateChangeRequest {
    kind: String,
    key: String,
    body: Value,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    required_approvals: Option<i32>,
}

#[derive(Deserialize, Default)]
struct Review {
    #[serde(default)]
    comment: String,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_change_requests(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let items = state
        .flags_v3
        .change_requests(q.status.as_deref(), q.limit.unwrap_or(100).clamp(1, 500))
        .await?;
    Ok(Json(json!({ "items": items })))
}

async fn create_change_request(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Json(req): Json<CreateChangeRequest>,
) -> ApiResult<(
    axum::http::StatusCode,
    Json<zanshin_store::flags_v3::ChangeRequest>,
)> {
    zanshin_core::flags::validate_key(&req.key)?;
    if !config::KINDS.contains(&req.kind.as_str()) {
        return Err(Error::invalid(format!("unknown config kind {:?}", req.kind)).into());
    }
    if !req.body.is_object() {
        return Err(Error::invalid("body must be a JSON object (the patch)").into());
    }
    let cr = state
        .flags_v3
        .create_change_request(
            &req.kind,
            &req.key,
            &req.body,
            &req.title,
            &req.description,
            &p.name,
            req.required_approvals.unwrap_or(1).clamp(1, 10),
        )
        .await?;
    state.webhooks.emit(
        "change_request.created",
        json!({ "id": cr.id, "kind": cr.kind, "key": cr.key, "author": cr.author, "title": cr.title }),
    );
    Ok((axum::http::StatusCode::CREATED, Json(cr)))
}

async fn get_change_request(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let cr = state.flags_v3.change_request(id).await?;
    let current = config::get_entity(&state, &cr.kind, &cr.key).await?;
    Ok(Json(json!({ "change_request": cr, "current": current })))
}

/// Four-eyes: the author cannot approve their own request.
async fn approve_change_request(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
    body: Option<Json<Review>>,
) -> ApiResult<Json<zanshin_store::flags_v3::ChangeRequest>> {
    let cr = state.flags_v3.change_request(id).await?;
    if cr.status != "open" {
        return Err(Error::conflict(format!("change request is {}", cr.status)).into());
    }
    if cr.author == p.name {
        return Err(Error::denied("you cannot approve your own change request").into());
    }
    let mut approvals = cr.approvals.as_array().cloned().unwrap_or_default();
    if approvals.iter().any(|a| a["by"] == p.name) {
        return Err(Error::conflict("already approved by you").into());
    }
    approvals.push(json!({ "by": p.name, "at": chrono::Utc::now(), "comment": body.map(|b| b.0.comment).unwrap_or_default() }));
    let status = if approvals.len() as i32 >= cr.required_approvals {
        "approved"
    } else {
        "open"
    };
    let cr = state
        .flags_v3
        .update_change_request(id, status, &Value::Array(approvals), "", false)
        .await?;
    state.webhooks.emit(
        "change_request.approved",
        json!({ "id": cr.id, "kind": cr.kind, "key": cr.key, "by": p.name, "status": cr.status }),
    );
    Ok(Json(cr))
}

async fn reject_change_request(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
    body: Option<Json<Review>>,
) -> ApiResult<Json<zanshin_store::flags_v3::ChangeRequest>> {
    let cr = state.flags_v3.change_request(id).await?;
    if !matches!(cr.status.as_str(), "open" | "approved") {
        return Err(Error::conflict(format!("change request is {}", cr.status)).into());
    }
    let comment = body.map(|b| b.0.comment).unwrap_or_default();
    let cr = state
        .flags_v3
        .update_change_request(
            id,
            "rejected",
            &cr.approvals,
            &format!("rejected by {}: {comment}", p.name),
            false,
        )
        .await?;
    state.webhooks.emit(
        "change_request.rejected",
        json!({ "id": cr.id, "kind": cr.kind, "key": cr.key, "by": p.name }),
    );
    Ok(Json(cr))
}

/// Apply an approved request. The actor needs write access to the target resource.
async fn apply_change_request(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<zanshin_store::flags_v3::ChangeRequest>> {
    let cr = state.flags_v3.change_request(id).await?;
    if cr.status != "approved" {
        return Err(Error::conflict(format!(
            "change request must be approved first (status: {})",
            cr.status
        ))
        .into());
    }
    if !p.allows(&cr.kind, Action::Write) {
        return Err(Error::denied(format!("applying needs {}:write", cr.kind)).into());
    }
    let res = config::apply(&state, &cr.kind, &cr.key, &cr.body).await;
    let cr = match res {
        Ok(_) => {
            state
                .flags_v3
                .update_change_request(id, "applied", &cr.approvals, "", true)
                .await?
        }
        Err(e) => {
            state
                .flags_v3
                .update_change_request(id, "approved", &cr.approvals, &e.to_string(), false)
                .await?;
            return Err(e.into());
        }
    };
    state.webhooks.emit(
        "change_request.applied",
        json!({ "id": cr.id, "kind": cr.kind, "key": cr.key, "by": p.name }),
    );
    Ok(Json(cr))
}

async fn cancel_change_request(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<zanshin_store::flags_v3::ChangeRequest>> {
    let cr = state.flags_v3.change_request(id).await?;
    if !matches!(cr.status.as_str(), "open" | "approved") {
        return Err(Error::conflict(format!("change request is {}", cr.status)).into());
    }
    if cr.author != p.name && !p.allows("approvals", Action::Delete) {
        return Err(Error::denied("only the author (or approvals:delete) can cancel").into());
    }
    Ok(Json(
        state
            .flags_v3
            .update_change_request(id, "cancelled", &cr.approvals, "", false)
            .await?,
    ))
}
