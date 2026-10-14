//! Load shedding: caps in-flight HTTP requests per process so an overload degrades into fast 503s
//! instead of multi-second latencies and unbounded memory. Health probes, metrics and WebSocket
//! upgrades (long-lived by design) are never counted.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct Shed(pub Option<Arc<Semaphore>>);

impl Shed {
    pub fn new(limit: usize) -> Self {
        Self((limit > 0).then(|| Arc::new(Semaphore::new(limit))))
    }
}

fn exempt(req: &Request) -> bool {
    matches!(req.uri().path(), "/healthz" | "/readyz" | "/metrics")
        || req
            .headers()
            .get(header::UPGRADE)
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

pub async fn shed(State(shed): State<Shed>, req: Request, next: Next) -> Response {
    let Some(sem) = shed.0.as_ref() else {
        return next.run(req).await;
    };
    if exempt(&req) {
        return next.run(req).await;
    }
    match sem.clone().try_acquire_owned() {
        Ok(_permit) => next.run(req).await,
        Err(_) => {
            metrics::counter!("zanshin_http_shed_total").increment(1);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "1")],
                Json(serde_json::json!({
                    "error": "overloaded",
                    "message": "too many in-flight requests, retry shortly"
                })),
            )
                .into_response()
        }
    }
}
