use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cellstudio_core::dataset::OpenError;
use cellstudio_core::reader::ReadError;
use cellstudio_core::rechunk::RechunkError;
use cellstudio_core::volume::ProxyError;
use cellstudio_db::{DbError, OpenError as ProjectError};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("no project is open")]
    NoProject,
    #[error("missing or invalid session token")]
    Unauthorized,
    #[error("origin {0} is not allowed")]
    Origin(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("dataset cannot be opened: {0}")]
    Dataset(#[from] OpenError),
    #[error("read failed: {0}")]
    Read(#[from] ReadError),
    #[error("project cannot be opened: {0}")]
    Project(#[from] ProjectError),
    #[error("database: {0}")]
    Db(#[from] DbError),
    #[error("proxy build failed: {0}")]
    Proxy(#[from] ProxyError),
    #[error("re-chunk failed: {0}")]
    Rechunk(#[from] RechunkError),
    #[error("{0}")]
    Internal(String),
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::NoProject | ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Origin(_) => StatusCode::FORBIDDEN,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Dataset(e) => match e {
                OpenError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_REQUEST,
            },
            ApiError::Read(e) => match e {
                ReadError::OutOfBounds { .. } => StatusCode::BAD_REQUEST,
                ReadError::UnknownLayer(_) => StatusCode::NOT_FOUND,
                ReadError::VolumeTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
                ReadError::Dataset(OpenError::NoSuchLevel { .. }) => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            ApiError::Project(e) => match e {
                ProjectError::Db(DbError::AlreadyOpen(_)) => StatusCode::CONFLICT,
                _ => StatusCode::BAD_REQUEST,
            },
            ApiError::Db(e) => match e {
                DbError::UnknownCell(_) => StatusCode::NOT_FOUND,
                DbError::AlreadyOpen(_) => StatusCode::CONFLICT,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            ApiError::Proxy(_) | ApiError::Rechunk(_) | ApiError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let message = self.to_string();
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), "{message}");
        } else {
            tracing::debug!(status = status.as_u16(), "{message}");
        }
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        ApiError::Internal(format!("blocking task failed: {e}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
