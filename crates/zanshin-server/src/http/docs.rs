//! API reference: the OpenAPI document (`/openapi.yaml`, `/openapi.json`) and Swagger UI on `/docs`.
//! Swagger UI is vendored (`assets/swagger-ui`, Apache-2.0) so the page works offline / air-gapped.

use axum::{
    http::header,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::Value;

use crate::state::AppState;

const OPENAPI_YAML: &str = include_str!("../../../../docs/openapi.yaml");
const OPENAPI_JSON: &str = include_str!("../../../../docs/openapi.json");
const SWAGGER_CSS: &str = include_str!("../../assets/swagger-ui/swagger-ui.css");
const SWAGGER_JS: &str = include_str!("../../assets/swagger-ui/swagger-ui-bundle.js");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/docs", get(ui))
        .route("/docs/", get(ui))
        .route("/docs/swagger-ui.css", get(css))
        .route("/docs/swagger-ui-bundle.js", get(js))
        .route("/openapi.yaml", get(yaml))
        .route("/openapi.json", get(json))
}

pub fn spec() -> Value {
    serde_json::from_str(OPENAPI_JSON).unwrap_or(Value::Null)
}

async fn yaml() -> Response {
    ([(header::CONTENT_TYPE, "application/yaml")], OPENAPI_YAML).into_response()
}

async fn json() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], OPENAPI_JSON).into_response()
}

async fn css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        SWAGGER_CSS,
    )
        .into_response()
}

async fn js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        SWAGGER_JS,
    )
        .into_response()
}

async fn ui() -> Html<String> {
    Html(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Zanshin API</title>
<link rel="stylesheet" href="/docs/swagger-ui.css"></head>
<body><div id="swagger-ui"></div>
<script src="/docs/swagger-ui-bundle.js"></script>
<script>window.ui = SwaggerUIBundle({{ url: "/openapi.json", dom_id: "#swagger-ui", deepLinking: true, persistAuthorization: true, tryItOutEnabled: true, displayRequestDuration: true }});</script>
</body></html>"##
            .to_string(),
    )
}
