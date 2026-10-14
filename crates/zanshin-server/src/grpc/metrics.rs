//! Tower layer recording `zanshin_grpc_requests_total{method,code}` and
//! `zanshin_grpc_request_seconds{method}` for every unary/stream call. The gRPC status is read
//! from response headers (tonic sends errors as trailers-only responses, so failures are
//! attributed correctly); successful streams are counted when the headers go out.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

use http::{Request, Response};
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub struct GrpcMetricsLayer;

impl<S> Layer<S> for GrpcMetricsLayer {
    type Service = GrpcMetrics<S>;
    fn layer(&self, inner: S) -> Self::Service {
        GrpcMetrics { inner }
    }
}

#[derive(Clone)]
pub struct GrpcMetrics<S> {
    inner: S,
}

impl<S, B, RB> Service<Request<B>> for GrpcMetrics<S>
where
    S: Service<Request<B>, Response = Response<RB>> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let method = req
            .uri()
            .path()
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_string();
        let started = Instant::now();
        let fut = self.inner.call(req);
        Box::pin(async move {
            let res = fut.await;
            let code = match &res {
                Ok(r) => r
                    .headers()
                    .get("grpc-status")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("0")
                    .to_string(),
                Err(_) => "transport".into(),
            };
            metrics::counter!("zanshin_grpc_requests_total", "method" => method.clone(), "code" => code)
                .increment(1);
            metrics::histogram!("zanshin_grpc_request_seconds", "method" => method)
                .record(started.elapsed().as_secs_f64());
            res
        })
    }
}
