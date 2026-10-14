use crate::services::currency::CurrencyService;
use crate::services::notifications::NotificationService;
use crate::services::taxonomy::TaxonomyService;
use crate::services::{
    analytics::AnalyticsService,
    audit::AuditService,
    console_users::ConsoleUserService,
    export::ExportService,
    integrations::IntegrationService,
    journeys::{JourneyDeps, JourneyService},
    limits::RateLimiter,
    metrics::MetricService,
    oidc::OidcService,
    templates::TemplateService,
    webhooks::WebhookService,
};
use std::sync::Arc;
use zanshin_store::taxonomy::TaxonomyRepo;

use zanshin_core::token::TokenIssuer;
use zanshin_store::audit::AuditRepo;
use zanshin_store::{
    analytics::{AdSpendRepo, AnnotationRepo, DashboardRepo, PlayerMetricsRepo},
    api_keys::ApiKeyRepo,
    audiences::AudienceRepo,
    console_users::ConsoleUserRepo,
    experiments::ExperimentRepo,
    export::{ConfigHistoryRepo, ExportRunRepo, ExportSinkRepo},
    flags::FlagRepo,
    identities::IdentityRepo,
    integrations::IntegrationRepo,
    journeys::JourneyRepo,
    live_events::LiveEventRepo,
    messages::MessageRepo,
    metrics::MetricRepo,
    push::PushTokenRepo,
    sessions::SessionRepo,
    templates::TemplateRepo,
    webhooks::WebhookRepo,
    Stores,
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
    pub console_users: ConsoleUserService,
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
    pub journeys: JourneyService,
    pub analytics: AnalyticsService,
    pub export: ExportService,
    pub config_history: ConfigHistoryRepo,
    pub push: PushService,
    pub eval: Evaluator,
    pub taxonomy: TaxonomyService,
    pub currency: CurrencyService,
    pub notifications: NotificationService,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Cluster-wide job locks (Postgres advisory locks).
    pub locks: zanshin_store::locks::JobLock,
    /// Flipped to `true` once the process starts shutting down; long-lived streams subscribe to it.
    pub shutdown: Arc<tokio::sync::watch::Sender<bool>>,
    draining: Arc<std::sync::atomic::AtomicBool>,
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

    /// Mark the process as draining: `/readyz` answers 503 so load balancers stop routing here.
    pub fn begin_drain(&self) {
        self.draining
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(std::sync::atomic::Ordering::Relaxed)
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
        let console_users = ConsoleUserService::new(
            ConsoleUserRepo::new(stores.pg.clone()),
            cfg.admin.console_session_ttl,
            cfg.telemetry.service_name.clone(),
        );
        let limits = RateLimiter::new(stores.cache.clone(), cfg.limits.clone());
        let audit = AuditService::new(AuditRepo::new(stores.pg.clone()), cfg.audit.clone());
        let notifications = NotificationService::new(
            zanshin_store::notifications::NotificationRepo::new(stores.pg.clone()),
        );
        audit.attach_notifier(notifications.clone());
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
        )
        .with_cache(stores.cache.clone());
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
        let webhooks = WebhookService::new(
            WebhookRepo::new(stores.pg.clone()),
            hooks.clone(),
            bus.clone(),
        );
        let taxonomy = TaxonomyService::new(
            TaxonomyRepo::new(stores.pg.clone()),
            stores.cache.clone(),
            cfg.taxonomy.clone(),
        );
        let currency = CurrencyService::new(
            zanshin_store::currency::CurrencyRepo::new(stores.pg.clone()),
            cfg.taxonomy.currency_rates_url.clone(),
        );
        {
            let c = currency.clone();
            tokio::spawn(async move {
                if let Err(e) = c.load().await {
                    tracing::warn!(error = %e, "currency rates not loaded");
                }
            });
        }
        events.attach_currency(currency.clone());
        {
            let ev = events.clone();
            let idr = identity.clone();
            taxonomy.set_error_sink(Box::new(move |identity_id, errors| {
                let (ev, idr) = (ev.clone(), idr.clone());
                tokio::spawn(async move {
                    if let Ok(i) = idr.get(identity_id).await {
                        ev.track(&i, errors, true, None).await.ok();
                    }
                });
            }));
        }
        let analytics = AnalyticsService::new(
            events.clone(),
            PlayerMetricsRepo::new(stores.pg.clone()),
            DashboardRepo::new(stores.pg.clone()),
            AnnotationRepo::new(stores.pg.clone()),
            taxonomy.clone(),
            AdSpendRepo::new(stores.pg.clone()),
            bus.clone(),
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
        events.attach_evaluator(eval.clone());
        let journeys = JourneyService::new(
            JourneyRepo::new(stores.pg.clone()),
            JourneyDeps {
                identities: IdentityRepo::new(stores.pg.clone()),
                eval: eval.clone(),
                messages: messages.clone(),
                events: events.clone(),
                flags: flags.clone(),
                experiments: experiments.clone(),
                integrations: integrations.clone(),
                live_events: LiveEventRepo::new(stores.pg.clone()),
                live_event_svc: live_events.clone(),
            },
            hooks,
        );
        let export = ExportService::new(
            ExportSinkRepo::new(stores.pg.clone()),
            ExportRunRepo::new(stores.pg.clone()),
            events.clone(),
        );
        export.attach_notifier(notifications.clone());
        let config_history = ConfigHistoryRepo::new(stores.pg.clone());
        let locks = zanshin_store::locks::JobLock::new(stores.pg.clone());
        Ok(Self {
            cfg,
            stores,
            identity,
            api_keys,
            oidc,
            console_users,
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
            journeys,
            analytics,
            export,
            config_history,
            push,
            eval,
            taxonomy,
            currency,
            notifications,
            metrics,
            started_at: chrono::Utc::now(),
            locks,
            shutdown: Arc::new(tokio::sync::watch::channel(false).0),
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
}
