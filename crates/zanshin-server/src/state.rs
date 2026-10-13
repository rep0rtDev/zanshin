use std::sync::Arc;

use zanshin_core::token::TokenIssuer;
use zanshin_store::{
    api_keys::ApiKeyRepo, audiences::AudienceRepo, experiments::ExperimentRepo, flags::FlagRepo,
    identities::IdentityRepo, sessions::SessionRepo, Stores,
};

use crate::{
    config::Config,
    services::{
        apikeys::ApiKeyService,
        audiences::AudienceService,
        bus::{ChangeBus, ChangeKind},
        eval::Evaluator,
        experiments::ExperimentService,
        flags::FlagService,
        identity::IdentityService,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub stores: Stores,
    pub identity: IdentityService,
    pub api_keys: ApiKeyService,
    pub bus: ChangeBus,
    pub flags: FlagService,
    pub audiences: AudienceService,
    pub experiments: ExperimentService,
    pub eval: Evaluator,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

impl AppState {
    pub async fn build(cfg: Arc<Config>) -> anyhow::Result<Self> {
        let stores = Stores::connect(&cfg.database, &cfg.redis, &cfg.clickhouse).await?;
        let state = Self::with_stores(cfg, stores)?;
        state
            .bus
            .register(ChangeKind::Flag, state.flags.catalog.clone())
            .await;
        state
            .bus
            .register(ChangeKind::Audience, state.audiences.catalog.clone())
            .await;
        state
            .bus
            .register(ChangeKind::Experiment, state.experiments.catalog.clone())
            .await;
        let (f, a, e) = tokio::try_join!(
            state.flags.catalog.load_all(),
            state.audiences.catalog.load_all(),
            state.experiments.catalog.load_all()
        )?;
        tracing::info!(flags = f, audiences = a, experiments = e, "catalogs loaded");
        state.bus.spawn_listener();
        Ok(state)
    }

    pub fn with_stores(cfg: Arc<Config>, stores: Stores) -> anyhow::Result<Self> {
        let tokens = TokenIssuer::new(
            cfg.auth.jwt_secret.as_bytes(),
            cfg.auth.issuer.clone(),
            cfg.auth.access_ttl,
            cfg.auth.refresh_ttl,
        )?;
        let identities = IdentityRepo::new(stores.pg.clone());
        let identity = IdentityService::new(
            identities.clone(),
            SessionRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            tokens,
        );
        let api_keys = ApiKeyService::new(
            ApiKeyRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            cfg.admin.bootstrap_key.clone(),
        );
        let bus = ChangeBus::new(stores.cache.clone(), cfg.redis.url.clone());
        let flags = FlagService::new(
            FlagRepo::new(stores.pg.clone()),
            identities.clone(),
            bus.clone(),
        );
        let audiences = AudienceService::new(AudienceRepo::new(stores.pg.clone()), bus.clone());
        let experiments =
            ExperimentService::new(ExperimentRepo::new(stores.pg.clone()), bus.clone());
        let eval = Evaluator::new(
            flags.clone(),
            audiences.clone(),
            experiments.clone(),
            identities,
        );
        Ok(Self {
            cfg,
            stores,
            identity,
            api_keys,
            bus,
            flags,
            audiences,
            experiments,
            eval,
            started_at: chrono::Utc::now(),
        })
    }
}
