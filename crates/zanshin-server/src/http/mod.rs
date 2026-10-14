//! REST/JSON surface (axum).

pub mod admin;
pub mod audiences;
pub mod audit;
pub mod auth;
pub mod console;
pub mod error;
pub mod experiments_v2;
pub mod limits;
pub mod liveops;
pub mod server_api;
pub mod taxonomy;
pub mod v1;
pub mod ws;

use std::net::SocketAddr;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderValue, Method},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use tokio::sync::watch;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::{state::AppState, telemetry};

pub fn router(state: AppState) -> Router {
    let cors = if state.cfg.server.cors_origins.is_empty() {
        CorsLayer::new().allow_origin(AllowOrigin::any())
    } else {
        let origins: Vec<HeaderValue> = state
            .cfg
            .server
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins)
    }
    .allow_methods([
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ])
    .allow_headers([
        header::AUTHORIZATION,
        header::CONTENT_TYPE,
        header::HeaderName::from_static("x-api-key"),
    ]);

    let mut security = tower::ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .option_layer(None);
    if state.cfg.server.hsts {
        security = tower::ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::if_not_present(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ))
            .layer(SetResponseHeaderLayer::if_not_present(
                header::X_FRAME_OPTIONS,
                HeaderValue::from_static("DENY"),
            ))
            .layer(SetResponseHeaderLayer::if_not_present(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ))
            .option_layer(Some(SetResponseHeaderLayer::if_not_present(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static("max-age=63072000; includeSubDomains"),
            )));
    }

    Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .nest("/v1", v1::router())
        .nest("/server/v1", server_api::router())
        .nest(
            "/admin/v1",
            admin::router()
                .merge(taxonomy::router())
                .merge(audiences::router())
                .merge(liveops::router())
                .merge(experiments_v2::router()),
        )
        .merge(console::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            audit::audit,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            limits::limit,
        ))
        .layer(security)
        .layer(DefaultBodyLimit::max(state.cfg.server.body_limit))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            state.cfg.server.request_timeout,
        ))
        .layer(CompressionLayer::new())
        .layer(cors)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}

pub async fn serve(
    state: AppState,
    addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "http listening");
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.wait_for(|v| *v).await;
    })
    .await?;
    Ok(())
}

async fn root(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "name": "zanshin",
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": state.started_at,
        "docs": "https://github.com/rep0rtDev/zanshin",
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn readyz(State(state): State<AppState>) -> error::ApiResult<Json<serde_json::Value>> {
    state.stores.ping().await?;
    Ok(Json(serde_json::json!({ "status": "ready" })))
}

async fn metrics() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        telemetry::prometheus().render(),
    )
}
