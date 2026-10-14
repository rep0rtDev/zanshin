//! API reference: the OpenAPI document (`/openapi.yaml`, `/openapi.json`) and Swagger UI on `/docs`.

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
const SWAGGER_UI: &str = "https://unpkg.com/swagger-ui-dist@5";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/docs", get(ui))
        .route("/docs/", get(ui))
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

async fn ui() -> Html<String> {
    Html(format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Zanshin API</title>
<link rel="stylesheet" href="{SWAGGER_UI}/swagger-ui.css"></head>
<body><div id="swagger-ui"></div>
<script src="{SWAGGER_UI}/swagger-ui-bundle.js" crossorigin></script>
<script>window.ui = SwaggerUIBundle({{ url: "/openapi.json", dom_id: "#swagger-ui", deepLinking: true, persistAuthorization: true, tryItOutEnabled: true, displayRequestDuration: true }});</script>
</body></html>"##
    ))
}
