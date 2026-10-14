//! Admin API: projects (served by the default project; needs `system:*`).

use axum::{
    extract::{Extension, Path, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{auth::AdminAuth, error::ApiResult};
use crate::{services::projects::Projects, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list).post(create))
        .route("/projects/{slug}", get(get_one).put(update))
}

fn registry(ext: Option<Extension<Projects>>) -> ApiResult<Projects> {
    ext.map(|Extension(p)| p).ok_or_else(|| {
        zanshin_core::Error::invalid("projects are managed through the default project").into()
    })
}

async fn list(
    State(state): State<AppState>,
    AdminAuth(_): AdminAuth,
    ext: Option<Extension<Projects>>,
) -> ApiResult<Json<Value>> {
    let p = registry(ext)?;
    Ok(Json(
        json!({ "items": p.list().await?, "current": state.project }),
    ))
}

#[derive(Debug, Deserialize)]
struct Create {
    slug: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

async fn create(
    AdminAuth(_): AdminAuth,
    ext: Option<Extension<Projects>>,
    Json(b): Json<Create>,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let p = registry(ext)?;
    let created = p.create(&b.slug, &b.name, &b.description).await?;
    Ok((axum::http::StatusCode::CREATED, Json(json!(created))))
}

async fn get_one(
    AdminAuth(_): AdminAuth,
    ext: Option<Extension<Projects>>,
    Path(slug): Path<String>,
) -> ApiResult<Json<Value>> {
    let p = registry(ext)?;
    let project = p.repo().get(&slug).await?;
    let running = p.state(&slug).await.is_some();
    let mut v = json!(project);
    v["running"] = json!(running);
    Ok(Json(v))
}

#[derive(Debug, Deserialize, Default)]
struct Patch {
    name: Option<String>,
    description: Option<String>,
    archived: Option<bool>,
}

async fn update(
    AdminAuth(_): AdminAuth,
    ext: Option<Extension<Projects>>,
    Path(slug): Path<String>,
    Json(b): Json<Patch>,
) -> ApiResult<Json<Value>> {
    let p = registry(ext)?;
    Ok(Json(json!(
        p.update(
            &slug,
            b.name.as_deref(),
            b.description.as_deref(),
            b.archived
        )
        .await?
    )))
}
