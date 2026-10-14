use crate::services::taxonomy::TaxonomyService;
use crate::services::{
    audit::AuditService, integrations::IntegrationService, limits::RateLimiter,
    metrics::MetricService, oidc::OidcService, templates::TemplateService,
    webhooks::WebhookService,
};
use std::sync::Arc;
use zanshin_store::taxonomy::TaxonomyRepo;

use zanshin_core::token::TokenIssuer;
use zanshin_store::audit::AuditRepo;
use zanshin_store::{
    api_keys::ApiKeyRepo, audiences::AudienceRepo, experiments::ExperimentRepo, flags::FlagRepo,
    identities::IdentityRepo, integrations::IntegrationRepo, live_events::LiveEventRepo,
    messages::MessageRepo, metrics::MetricRepo, push::PushTokenRepo, sessions::SessionRepo,
    templates::TemplateRepo, webhooks::WebhookRepo, Stores,
};

use crate::{
    config::Config,
    services::{
        apikeys::ApiKeyService,
        audiences::AudienceService,
        bus::{ChangeBus, ChangeKind},
        eval::Evaluator,
        events::EventService,
        experiments::ExperimentService,
        flags::FlagService,
        identity::IdentityService,
        live_events::LiveEventService,
        messages::MessageService,
        push::PushService,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub stores: Stores,
    pub identity: IdentityService,
    pub api_keys: ApiKeyService,
    pub oidc: OidcService,
    pub limits: RateLimiter,
    pub audit: AuditService,
    pub bus: ChangeBus,
    pub flags: FlagService,
    pub audiences: AudienceService,
    pub experiments: ExperimentService,
    pub metrics: MetricService,
    pub events: EventService,
    pub live_events: LiveEventService,
    pub messages: MessageService,
    pub templates: TemplateService,
    pub integrations: IntegrationService,
    pub webhooks: WebhookService,
    pub push: PushService,
    pub eval: Evaluator,
    pub taxonomy: TaxonomyService,
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
        state
            .bus
            .register(ChangeKind::LiveEvent, state.live_events.catalog.clone())
            .await;
        state
            .bus
            .register(ChangeKind::Message, state.messages.catalog.clone())
            .await;
        let (f, a, e, l, m) = tokio::try_join!(
            state.flags.catalog.load_all(),
            state.audiences.catalog.load_all(),
            state.experiments.catalog.load_all(),
            state.live_events.catalog.load_all(),
            state.messages.catalog.load_all(),
        )?;
        tracing::info!(
            flags = f,
            audiences = a,
            experiments = e,
            live_events = l,
            messages = m,
            "catalogs loaded"
        );
        state.bus.spawn_listener();
        state.events.spawn_writer();
        tracing::info!(backend = state.events.backend(), "event sink ready");
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
        let oidc = OidcService::new(
            cfg.admin.oidc.clone(),
            &cfg.auth.jwt_secret,
            cfg.admin.console_session_ttl,
        );
        let limits = RateLimiter::new(stores.cache.clone(), cfg.limits.clone());
        let audit = AuditService::new(AuditRepo::new(stores.pg.clone()), cfg.audit.clone());
        let bus = ChangeBus::new(stores.cache.clone(), cfg.redis.url.clone());
        let flags = FlagService::new(
            FlagRepo::new(stores.pg.clone()),
            identities.clone(),
            bus.clone(),
        );
        let audiences = AudienceService::new(
            AudienceRepo::new(stores.pg.clone()),
            IdentityRepo::new(stores.pg.clone()),
            bus.clone(),
        );
        let event_store = match &stores.clickhouse {
            Some(ch) => zanshin_store::events::EventStore::ClickHouse(Box::new(ch.clone())),
            None => zanshin_store::events::EventStore::Postgres(stores.pg.clone()),
        };
        let hooks = crate::services::webhooks::new_bus();
        let events = EventService::new(
            event_store,
            identities.clone(),
            ExperimentRepo::new(stores.pg.clone()),
            hooks.clone(),
        );
        let experiments = ExperimentService::new(
            ExperimentRepo::new(stores.pg.clone()),
            bus.clone(),
            events.clone(),
        );
        let live_events = LiveEventService::new(LiveEventRepo::new(stores.pg.clone()), bus.clone());
        let push = PushService::new(PushTokenRepo::new(stores.pg.clone()), &cfg.push)?;
        let templates = TemplateService::new(TemplateRepo::new(stores.pg.clone()));
        let integrations =
            IntegrationService::new(IntegrationRepo::new(stores.pg.clone()), push.clone());
        let messages = MessageService::new(
            MessageRepo::new(stores.pg.clone()),
            bus.clone(),
            push.clone(),
            TemplateRepo::new(stores.pg.clone()),
            integrations.clone(),
            LiveEventRepo::new(stores.pg.clone()),
            hooks.clone(),
        );
        let webhooks = WebhookService::new(WebhookRepo::new(stores.pg.clone()), hooks, bus.clone());
        let taxonomy = TaxonomyService::new(
            TaxonomyRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            cfg.taxonomy.clone(),
        );
        let metrics = MetricService::new(MetricRepo::new(stores.pg.clone()), events.clone());
        let eval = Evaluator::new(
            flags.clone(),
            audiences.clone(),
            experiments.clone(),
            live_events.clone(),
            messages.clone(),
            identities,
            stores.cache.clone(),
        );
        Ok(Self {
            cfg,
            stores,
            identity,
            api_keys,
            oidc,
            limits,
            audit,
            bus,
            flags,
            audiences,
            experiments,
            events,
            live_events,
            messages,
            templates,
            integrations,
            webhooks,
            push,
            eval,
            taxonomy,
            metrics,
            started_at: chrono::Utc::now(),
        })
    }
}
