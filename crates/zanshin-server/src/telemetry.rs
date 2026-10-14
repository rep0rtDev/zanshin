//! Logs (tracing → stdout, text or JSON), metrics (Prometheus at `/metrics`) and optional
//! distributed traces (OTLP/gRPC exporter, enabled by `telemetry.otlp_endpoint`).

use std::sync::OnceLock;

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use opentelemetry::{trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, TracerProvider},
    Resource,
};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use crate::config::{LogConfig, TelemetryConfig};

static PROM: OnceLock<PrometheusHandle> = OnceLock::new();
static TRACER: OnceLock<TracerProvider> = OnceLock::new();

pub fn init(cfg: &LogConfig) -> anyhow::Result<()> {
    init_with(cfg, &TelemetryConfig::default())
}

pub fn init_with(cfg: &LogConfig, tel: &TelemetryConfig) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(&cfg.level))?;
    let otel = match &tel.otlp_endpoint {
        Some(endpoint) if !endpoint.is_empty() => Some(otel_layer(endpoint, tel)?),
        _ => None,
    };
    let fmt_layer = if cfg.json {
        fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .boxed()
    } else {
        fmt::layer().compact().boxed()
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(otel)
        .try_init()?;
    if let Some(e) = &tel.otlp_endpoint {
        tracing::info!(endpoint = %e, sample_ratio = tel.sample_ratio, "otlp trace export enabled");
    }
    Ok(())
}

fn otel_layer<S>(
    endpoint: &str,
    tel: &TelemetryConfig,
) -> anyhow::Result<tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    let mut attrs = vec![
        KeyValue::new("service.name", tel.service_name.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ];
    for (k, v) in &tel.resource {
        attrs.push(KeyValue::new(k.clone(), v.clone()));
    }
    let sampler = if tel.sample_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else {
        Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(tel.sample_ratio)))
    };
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_sampler(sampler)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(Resource::new(attrs))
        .build();
    let tracer = provider.tracer("zanshin");
    let _ = TRACER.set(provider.clone());
    opentelemetry::global::set_tracer_provider(provider);
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

/// Flush pending spans; call once on shutdown.
pub fn shutdown() {
    if let Some(p) = TRACER.get() {
        if let Err(e) = p.shutdown() {
            tracing::warn!(error = %e, "trace exporter shutdown failed");
        }
    }
}

/// Process-wide Prometheus recorder; safe to call many times (tests).
pub fn prometheus() -> PrometheusHandle {
    PROM.get_or_init(|| {
        PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Suffix("_seconds".into()),
                &[
                    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
            )
            .expect("histogram buckets")
            .install_recorder()
            .expect("install prometheus recorder")
    })
    .clone()
}
