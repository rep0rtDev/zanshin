//! Admin API: message templates, delivery integrations, per-channel delivery log.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use zanshin_core::{integrations::Integration, templates::Template, Error};

use super::{auth::AdminAuth, error::ApiResult};
use crate::{
    services::{integrations::IntegrationPatch, templates::TemplatePatch},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/templates", get(list_templates).post(create_template))
        .route(
            "/templates/{key}",
            get(get_template)
                .put(update_template)
                .delete(delete_template),
        )
        .route("/templates/{key}/render", post(render_template))
        .route("/templates/{key}/test", post(test_template))
        .route(
            "/integrations",
            get(list_integrations).post(create_integration),
        )
        .route(
            "/integrations/{key}",
            get(get_integration)
                .put(update_integration)
                .delete(delete_integration),
        )
        .route("/integrations/{key}/test", post(test_integration))
        .route("/messages/{key}/deliveries", get(deliveries))
        .route("/messages/{key}/variants", get(message_variants))
        .route("/messages/{key}/deferrals", get(message_deferrals))
        .route(
            "/identities/{id}/inbox/{msg}",
            axum::routing::delete(admin_delete_inbox),
        )
}

async fn list_templates(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.templates.list().await? })))
}

#[derive(Debug, Deserialize)]
struct CreateTemplate {
    key: String,
    #[serde(flatten)]
    patch: TemplatePatch,
}

async fn create_template(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<CreateTemplate>,
) -> ApiResult<(axum::http::StatusCode, Json<Template>)> {
    let t = state.templates.create(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(t)))
}

async fn get_template(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let t = state.templates.get(&key).await?;
    let used_by = state.templates.used_by(&key).await?;
    Ok(Json(json!({ "template": t, "used_by": used_by })))
}

async fn update_template(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<TemplatePatch>,
) -> ApiResult<Json<Template>> {
    let t = state.templates.update(&key, p).await?;
    state
        .bus
        .publish(
            state
                .bus
                .change(crate::services::bus::ChangeKind::Message, &key, t.version),
        )
        .await;
    Ok(Json(t))
}

async fn delete_template(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    crate::services::refs::ensure_unreferenced(
        &state,
        crate::services::refs::RefKind::Template,
        &key,
    )
    .await?;
    state.templates.delete(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

#[derive(Debug, Default, Deserialize)]
struct Target {
    #[serde(default)]
    identity_id: Option<Uuid>,
    #[serde(default)]
    external_id: Option<String>,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    push: bool,
}

async fn resolve(state: &AppState, t: &Target) -> ApiResult<zanshin_core::identity::Identity> {
    Ok(match (&t.identity_id, &t.external_id) {
        (Some(id), _) => state.identity.get(*id).await?,
        (None, Some(ext)) => state.identity.get_by_external_id(ext).await?,
        _ => return Err(Error::invalid("identity_id or external_id is required").into()),
    })
}

/// Preview a template for a player (or with no player: default locale, empty properties).
async fn render_template(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(t): Json<Target>,
) -> ApiResult<Json<Value>> {
    let (locale, flat) = if t.identity_id.is_some() || t.external_id.is_some() {
        let identity = resolve(&state, &t).await?;
        let flat = crate::services::eval::flatten_with_identity(&identity, &identity.properties);
        (
            t.locale.clone().or_else(|| Template::locale_of(&flat)),
            flat,
        )
    } else {
        (t.locale.clone(), Default::default())
    };
    let (tpl, r) = state
        .templates
        .render(&key, locale.as_deref(), &flat)
        .await?;
    Ok(Json(
        json!({ "template": tpl.key, "format": tpl.format, "rendered": r }),
    ))
}

async fn test_template(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(t): Json<Target>,
) -> ApiResult<Json<Value>> {
    let identity = resolve(&state, &t).await?;
    let flat = crate::services::eval::flatten_with_identity(&identity, &identity.properties);
    let (r, id) = state
        .messages
        .send_template(&identity, &flat, &key, t.locale.as_deref(), t.push)
        .await?;
    Ok(Json(
        json!({ "inbox_id": id, "identity_id": identity.id, "rendered": r }),
    ))
}

async fn list_integrations(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": state.integrations.list().await? })))
}

#[derive(Debug, Deserialize)]
struct CreateIntegration {
    key: String,
    #[serde(flatten)]
    patch: IntegrationPatch,
}

async fn create_integration(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Json(b): Json<CreateIntegration>,
) -> ApiResult<(axum::http::StatusCode, Json<Integration>)> {
    let i = state.integrations.create(&b.key, b.patch).await?;
    Ok((axum::http::StatusCode::CREATED, Json(i)))
}

async fn get_integration(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Integration>> {
    Ok(Json(state.integrations.get(&key).await?))
}

async fn update_integration(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(p): Json<IntegrationPatch>,
) -> ApiResult<Json<Integration>> {
    Ok(Json(state.integrations.update(&key, p).await?))
}

async fn delete_integration(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    crate::services::refs::ensure_unreferenced(
        &state,
        crate::services::refs::RefKind::Integration,
        &key,
    )
    .await?;
    state.integrations.delete(&key).await?;
    Ok(Json(json!({ "deleted": key })))
}

/// Send a sample message to one player through the integration and report the outcome.
async fn test_integration(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Json(t): Json<Target>,
) -> ApiResult<Json<Value>> {
    let identity = resolve(&state, &t).await?;
    let flat = crate::services::eval::flatten_with_identity(&identity, &identity.properties);
    let payload = json!({ "test": true });
    let out = crate::services::integrations::Outbound {
        message_key: "test",
        identity_id: identity.id,
        external_id: &identity.external_id,
        occurrence: None,
        locale: t.locale.as_deref().unwrap_or("en"),
        title: "Zanshin test message",
        body: "If you can read this, the integration works.",
        image_url: "",
        payload: &payload,
        props: &flat,
    };
    let o = state.integrations.deliver(&key, &out).await;
    Ok(Json(json!(o)))
}

#[derive(Debug, Default, Deserialize)]
struct DeliveriesQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    after: Option<Uuid>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn deliveries(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
    Query(q): Query<DeliveriesQuery>,
) -> ApiResult<Json<Value>> {
    let (items, stats) = state
        .messages
        .deliveries(
            &key,
            q.status.as_deref(),
            q.channel.as_deref(),
            q.after,
            q.limit.unwrap_or(100),
        )
        .await?;
    let next = items.last().map(|d| d.id);
    let stats: Vec<Value> = stats
        .into_iter()
        .map(|(channel, status, count)| json!({"channel": channel, "status": status, "count": count}))
        .collect();
    Ok(Json(
        json!({ "items": items, "next": next, "stats": stats }),
    ))
}

async fn admin_delete_inbox(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path((id, msg)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    let identity = state.identity.get(id).await?;
    state.messages.delete_inbox(&identity, msg).await?;
    Ok(Json(json!({ "deleted": msg })))
}

/// Sent / read / consumed per copy variant (A/B templates).
async fn message_variants(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let (m, stats) = state.messages.variant_stats(&key).await?;
    let total: i64 = stats.iter().map(|s| s.sent).sum();
    let items: Vec<Value> = m
        .variants
        .iter()
        .map(|v| {
            let s = stats.iter().find(|s| s.variant == v.key);
            let sent = s.map(|s| s.sent).unwrap_or(0);
            let read = s.map(|s| s.read).unwrap_or(0);
            json!({
                "key": v.key, "weight": v.weight, "title": v.title, "template_key": v.template_key,
                "sent": sent, "read": read, "consumed": s.map(|s| s.consumed).unwrap_or(0),
                "read_rate": if sent > 0 { Some(read as f64 / sent as f64) } else { None },
            })
        })
        .collect();
    let untagged = stats
        .iter()
        .filter(|s| s.variant.is_empty())
        .map(|s| s.sent)
        .sum::<i64>();
    Ok(Json(
        json!({ "message": m.key, "total_sent": total, "untagged": untagged, "items": items }),
    ))
}

async fn message_deferrals(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    Path(key): Path<String>,
) -> ApiResult<Json<Value>> {
    let items = state.messages.deferrals(&key).await?;
    Ok(Json(json!({ "items": items })))
}
