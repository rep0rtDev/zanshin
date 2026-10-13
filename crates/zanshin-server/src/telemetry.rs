use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::LogConfig;

static PROM: OnceLock<PrometheusHandle> = OnceLock::new();

pub fn init(cfg: &LogConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(&cfg.level))?;
    let registry = tracing_subscriber::registry().with(filter);
    if cfg.json {
        registry
            .with(fmt::layer().json().flatten_event(true))
            .try_init()?;
    } else {
        registry.with(fmt::layer().compact()).try_init()?;
    }
    Ok(())
}

/// Process-wide Prometheus recorder; safe to call many times (tests).
pub fn prometheus() -> PrometheusHandle {
    PROM.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("install prometheus recorder")
    })
    .clone()
}
