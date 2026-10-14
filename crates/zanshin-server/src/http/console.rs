//! Admin console: the compiled React app from `console/dist` embedded into the binary
//! and served under `/console` with SPA fallback (deep links render `index.html`).

use axum::{
    body::Body,
    extract::Path,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "../../console/dist/"]
#[exclude = ".gitkeep"]
struct Assets;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/console",
            get(|| async { Redirect::permanent("/console/") }),
        )
        .route("/console/", get(index))
        .route("/console/{*path}", get(asset))
}

async fn index() -> Response {
    serve("index.html").unwrap_or_else(missing)
}

async fn asset(Path(path): Path<String>, _uri: Uri) -> Response {
    match serve(&path) {
        Some(r) => r,
        None if path.contains('.') => StatusCode::NOT_FOUND.into_response(),
        None => index().await,
    }
}

fn serve(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Some(
        (
            [
                (header::CONTENT_TYPE, mime.as_ref().to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            Body::from(file.data.into_owned()),
        )
            .into_response(),
    )
}

fn missing() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "admin console was not built into this binary (run `npm run build` in console/ before cargo build)",
    )
        .into_response()
}
