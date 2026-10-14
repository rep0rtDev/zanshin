//! Multi-project: one `AppState` (schema / Redis prefix / ClickHouse database / housekeeping) per
//! project, plus the HTTP dispatcher that routes a request to its project by the
//! `x-zanshin-project` header, `?project=` query, the `prj` claim of a bearer / `?token=` session
//! token, or the default project.

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use tokio::sync::RwLock;
use tower::ServiceExt;
use zanshin_core::{Error, Result};
use zanshin_store::projects::{Project, ProjectRepo, DEFAULT_SLUG};

use crate::{config::Config, state::AppState};

pub const HEADER: &str = "x-zanshin-project";

struct Entry {
    state: AppState,
    router: Router,
}

#[derive(Clone)]
pub struct Projects {
    cfg: Arc<Config>,
    default: AppState,
    repo: ProjectRepo,
    entries: Arc<RwLock<HashMap<String, Arc<Entry>>>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

impl Projects {
    /// Build the default project and every registered, non-archived project.
    pub async fn start(
        cfg: Arc<Config>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<Self> {
        let default = AppState::build(cfg.clone()).await?;
        let repo = ProjectRepo::new(default.stores.pg.clone());
        let me = Self {
            cfg,
            default: default.clone(),
            repo,
            entries: Arc::new(RwLock::new(HashMap::new())),
            shutdown,
        };
        me.install(default).await;
        for p in me.repo.list().await.unwrap_or_default() {
            if p.slug != DEFAULT_SLUG && !p.archived {
                if let Err(e) = me.load(&p.slug).await {
                    tracing::error!(project = %p.slug, error = %e, "project failed to start");
                }
            }
        }
        Ok(me)
    }

    pub fn default_state(&self) -> &AppState {
        &self.default
    }

    pub fn repo(&self) -> &ProjectRepo {
        &self.repo
    }

    async fn install(&self, state: AppState) {
        let mut router = crate::http::router(state.clone());
        if state.project == DEFAULT_SLUG {
            router = router.layer(axum::Extension(self.clone()));
        }
        if state.project != DEFAULT_SLUG {
            tokio::spawn(crate::housekeeping::run(
                state.clone(),
                self.shutdown.clone(),
            ));
        }
        self.entries
            .write()
            .await
            .insert(state.project.clone(), Arc::new(Entry { state, router }));
    }

    /// Connect and start a project that is registered but not running yet.
    pub async fn load(&self, slug: &str) -> anyhow::Result<AppState> {
        if let Some(e) = self.entries.read().await.get(slug) {
            return Ok(e.state.clone());
        }
        let state = AppState::build_project(
            self.cfg.clone(),
            slug,
            Some((
                self.default.console_users.clone(),
                self.default.oidc.clone(),
            )),
        )
        .await?;
        self.install(state.clone()).await;
        tracing::info!(project = slug, "project started");
        Ok(state)
    }

    /// Register + start a new project.
    pub async fn create(&self, slug: &str, name: &str, description: &str) -> Result<Project> {
        if slug == DEFAULT_SLUG {
            return Err(Error::invalid("default already exists"));
        }
        let p = self.repo.create(slug, name, description).await?;
        self.load(slug).await.map_err(|e| {
            Error::internal(anyhow::anyhow!(
                "project {slug} registered but failed to start: {e}"
            ))
        })?;
        Ok(p)
    }

    /// Archive (stop routing, keep data) or un-archive (start again).
    pub async fn update(
        &self,
        slug: &str,
        name: Option<&str>,
        description: Option<&str>,
        archived: Option<bool>,
    ) -> Result<Project> {
        if slug == DEFAULT_SLUG && archived == Some(true) {
            return Err(Error::invalid("the default project cannot be archived"));
        }
        let p = self.repo.update(slug, name, description, archived).await?;
        match archived {
            Some(true) => {
                if let Some(e) = self.entries.write().await.remove(slug) {
                    let _ = e.state.shutdown.send(true);
                }
            }
            Some(false) => {
                self.load(slug)
                    .await
                    .map_err(|e| Error::internal(anyhow::anyhow!(e)))?;
            }
            None => {}
        }
        Ok(p)
    }

    pub async fn list(&self) -> Result<Vec<serde_json::Value>> {
        let running: Vec<String> = self.entries.read().await.keys().cloned().collect();
        Ok(self
            .repo
            .list()
            .await?
            .into_iter()
            .map(|p| {
                let mut v = serde_json::to_value(&p).unwrap_or_default();
                v["running"] = serde_json::Value::Bool(running.contains(&p.slug));
                v
            })
            .collect())
    }

    pub async fn state(&self, slug: &str) -> Option<AppState> {
        self.entries.read().await.get(slug).map(|e| e.state.clone())
    }

    pub async fn states(&self) -> Vec<AppState> {
        self.entries
            .read()
            .await
            .values()
            .map(|e| e.state.clone())
            .collect()
    }

    /// Which project a request addresses.
    pub fn slug_of(req: &Request<Body>) -> String {
        if let Some(h) = req.headers().get(HEADER).and_then(|v| v.to_str().ok()) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
        let query = req.uri().query().unwrap_or("");
        for pair in query.split('&') {
            if let Some(v) = pair.strip_prefix("project=") {
                if !v.is_empty() {
                    return v.to_string();
                }
            }
        }
        let bearer = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|a| {
                a.strip_prefix("Bearer ")
                    .or_else(|| a.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|t| t.contains('.'))
            .map(str::to_string)
            .or_else(|| {
                query
                    .split('&')
                    .find_map(|p| p.strip_prefix("token="))
                    .map(str::to_string)
            });
        if let Some(prj) = bearer
            .as_deref()
            .and_then(zanshin_core::token::peek_project)
        {
            return prj;
        }
        DEFAULT_SLUG.to_string()
    }

    /// Route one request to its project's router.
    pub async fn dispatch(self, req: Request<Body>) -> Response {
        let slug = Self::slug_of(&req);
        let entry = self.entries.read().await.get(&slug).cloned();
        match entry {
            Some(e) => e.router.clone().oneshot(req).await.into_response(),
            None => (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({
                    "code": "not_found",
                    "message": format!("unknown project {slug:?}")
                })),
            )
                .into_response(),
        }
    }

    /// Top-level router: every request goes through `dispatch`.
    pub fn router(self) -> Router {
        Router::new().fallback_service(tower::service_fn(move |req: Request<Body>| {
            let me = self.clone();
            async move { Ok::<_, std::convert::Infallible>(me.dispatch(req).await) }
        }))
    }
}
