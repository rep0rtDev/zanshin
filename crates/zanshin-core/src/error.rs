use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    InvalidArgument(String),
    #[error("{0}")]
    Unauthenticated(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidArgument(msg.into())
    }
    pub fn unauthenticated(msg: impl Into<String>) -> Self {
        Self::Unauthenticated(msg.into())
    }
    pub fn denied(msg: impl Into<String>) -> Self {
        Self::PermissionDenied(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::Unavailable(msg.into())
    }
    pub fn internal(err: impl Into<anyhow::Error>) -> Self {
        Self::Internal(err.into())
    }

    /// Stable machine-readable code used by both REST and gRPC surfaces.
    pub fn code(&self) -> &'static str {
        match self {
            Error::InvalidArgument(_) => "invalid_argument",
            Error::Unauthenticated(_) => "unauthenticated",
            Error::PermissionDenied(_) => "permission_denied",
            Error::NotFound(_) => "not_found",
            Error::Conflict(_) => "conflict",
            Error::Unavailable(_) => "unavailable",
            Error::Internal(_) => "internal",
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::InvalidArgument(e.to_string())
    }
}
