//! Per-route HTTP metrics: `zanshin_http_requests_total{method,route,status}` and
//! `zanshin_http_request_seconds{method,route}`. Routes are the matched axum templates
//! (`/v1/flags/{name}`), never raw paths, so cardinality stays bounded.

use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};

pub async fn record(req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().as_str().to_string();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".into());
    let res = next.run(req).await;
    let status = res.status().as_u16().to_string();
    let elapsed = started.elapsed().as_secs_f64();
    metrics::counter!(
        "zanshin_http_requests_total",
        "method" => method.clone(), "route" => route.clone(), "status" => status
    )
    .increment(1);
    metrics::histogram!("zanshin_http_request_seconds", "method" => method, "route" => route)
        .record(elapsed);
    res
}
