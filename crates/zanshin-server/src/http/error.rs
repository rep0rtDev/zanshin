use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use zanshin_core::Error;

#[derive(Debug)]
pub struct ApiError(pub Error);

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Serialize)]
struct Body<'a> {
    code: &'a str,
    message: String,
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(Error::Internal(e))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::InvalidArgument(_) => StatusCode::BAD_REQUEST,
            Error::Unauthenticated(_) => StatusCode::UNAUTHORIZED,
            Error::PermissionDenied(_) => StatusCode::FORBIDDEN,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::ResourceExhausted(_) => StatusCode::TOO_MANY_REQUESTS,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match &self.0 {
            Error::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                "internal error".to_string()
            }
            other => other.to_string(),
        };
        metrics::counter!("zanshin_http_errors_total", "code" => self.0.code()).increment(1);
        (
            status,
            Json(Body {
                code: self.0.code(),
                message,
            }),
        )
            .into_response()
    }
}
