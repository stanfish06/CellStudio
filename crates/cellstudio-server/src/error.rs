use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cellstudio_core::dataset::OpenError;
use cellstudio_core::labels::LabelError;
use cellstudio_core::reader::ReadError;
use cellstudio_core::rechunk::RechunkError;
use cellstudio_core::volume::ProxyError;
use cellstudio_db::{DbError, OpenError as ProjectError};

use crate::edit::EditError;

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
    #[error("{0}")]
    Conflict(String),
    #[error(
        "session {presented} is not the open session; the project was replaced, so this \
         request was refused before anything was read or written"
    )]
    StaleSession { presented: String },
    #[error("{0}")]
    NotImplemented(String),
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
            ApiError::Conflict(_) | ApiError::StaleSession { .. } => StatusCode::CONFLICT,
            ApiError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
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
                DbError::AlreadyOpen(_)
                | DbError::LabelFrameConflict { .. }
                | DbError::LabelIdTaken(_)
                | DbError::LabelIdsExhausted { .. }
                | DbError::TrackIdsExhausted
                | DbError::InventoryPending => StatusCode::CONFLICT,
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

/// Mask edits fail as client errors far more often than as server ones: an id that was not
/// reserved, an id already living on another frame, an empty undo stack.
impl From<EditError> for ApiError {
    fn from(e: EditError) -> Self {
        match e {
            EditError::Labels(LabelError::Contract(_)) | EditError::Invalid(_) => {
                ApiError::BadRequest(e.to_string())
            }
            EditError::Unreserved(_) | EditError::NothingTo(_) | EditError::Pruned(_) => {
                ApiError::Conflict(e.to_string())
            }
            EditError::NoStore(_) => ApiError::NotFound(e.to_string()),
            // structured graph rejections leave the graph unchanged; the reason reaches the
            // status bar verbatim
            EditError::Graph(g) => match g {
                cellstudio_db::GraphError::UnknownCell(_) => ApiError::NotFound(g.to_string()),
                cellstudio_db::GraphError::Db(db) => ApiError::Db(db),
                _ => ApiError::Conflict(g.to_string()),
            },
            EditError::Db(db) => ApiError::Db(db),
            EditError::Dataset(source) => ApiError::Dataset(source),
            EditError::Labels(_) | EditError::Journal { .. } => ApiError::Internal(e.to_string()),
        }
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        ApiError::Internal(format!("blocking task failed: {e}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
