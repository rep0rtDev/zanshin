use std::sync::Arc;

use zanshin_core::token::TokenIssuer;
use zanshin_store::{
    api_keys::ApiKeyRepo, flags::FlagRepo, identities::IdentityRepo, sessions::SessionRepo, Stores,
};

use crate::{
    config::Config,
    services::{apikeys::ApiKeyService, flags::FlagService, identity::IdentityService},
};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub stores: Stores,
    pub identity: IdentityService,
    pub api_keys: ApiKeyService,
    pub flags: FlagService,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl AppState {
    pub async fn build(cfg: Arc<Config>) -> anyhow::Result<Self> {
        let stores = Stores::connect(&cfg.database, &cfg.redis, &cfg.clickhouse).await?;
        let state = Self::with_stores(cfg, stores)?;
        let n = state.flags.load_all().await?;
        tracing::info!(flags = n, "flag catalog loaded");
        state.flags.spawn_listener();
        Ok(state)
    }

    pub fn with_stores(cfg: Arc<Config>, stores: Stores) -> anyhow::Result<Self> {
        let tokens = TokenIssuer::new(
            cfg.auth.jwt_secret.as_bytes(),
            cfg.auth.issuer.clone(),
            cfg.auth.access_ttl,
            cfg.auth.refresh_ttl,
        )?;
        let identity = IdentityService::new(
            IdentityRepo::new(stores.pg.clone()),
            SessionRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            tokens,
        );
        let api_keys = ApiKeyService::new(
            ApiKeyRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            cfg.admin.bootstrap_key.clone(),
        );
        let flags = FlagService::new(
            FlagRepo::new(stores.pg.clone()),
            IdentityRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            cfg.redis.url.clone(),
        );
        Ok(Self {
            cfg,
            stores,
            identity,
            api_keys,
            flags,
            started_at: chrono::Utc::now(),
        })
    }
}
