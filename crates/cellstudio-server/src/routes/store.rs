use std::path::{Component, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use cellstudio_core::LayerId;
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::routes::project::session_response;
use crate::state::AppState;

#[derive(Debug, Default, Deserialize)]
pub struct StoreQuery {
    #[serde(default)]
    layer: Option<LayerId>,
    #[serde(default, rename = "v")]
    _version: Option<u64>,
}

pub async fn get_object(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    Query(query): Query<StoreQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let active = state.require()?;
    let root = match query.layer.unwrap_or(LayerId::Image) {
        LayerId::Image => active.source.clone(),
        LayerId::Labels => active.project.labels_store_path(),
    };
    let target = resolve(&root, &path)?;
    let range = headers
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let response = state
        .io(move || {
            let metadata = std::fs::metadata(&target).map_err(|_| absent(&path))?;
            if !metadata.is_file() {
                return Err(absent(&path));
            }
            match range
                .as_deref()
                .map(|header| parse_range(header, metadata.len()))
            {
                None => {
                    let bytes = std::fs::read(&target).map_err(|_| absent(&path))?;
                    Ok(body(StatusCode::OK, bytes, None, metadata.len()))
                }
                Some(Err(())) => Ok(unsatisfiable(metadata.len())),
                Some(Ok(None)) => {
                    let bytes = std::fs::read(&target).map_err(|_| absent(&path))?;
                    Ok(body(StatusCode::OK, bytes, None, metadata.len()))
                }
                Some(Ok(Some((first, last)))) => {
                    let bytes = read_span(&target, first, last - first + 1)?;
                    Ok(body(
                        StatusCode::PARTIAL_CONTENT,
                        bytes,
                        Some((first, last)),
                        metadata.len(),
                    ))
                }
            }
        })
        .await?;

    Ok(session_response(&active.session_id, response))
}

fn absent(path: &str) -> ApiError {
    ApiError::NotFound(format!("no stored object at {path}"))
}

fn resolve(root: &std::path::Path, path: &str) -> ApiResult<PathBuf> {
    if path.is_empty() {
        return Err(ApiError::BadRequest("store path is empty".to_owned()));
    }
    let mut target = root.to_path_buf();
    for component in PathBuf::from(path).components() {
        match component {
            Component::Normal(part) => target.push(part),
            _ => {
                return Err(ApiError::BadRequest(format!(
                    "store path {path} must be relative and free of `..`"
                )));
            }
        }
    }
    Ok(target)
}

fn read_span(path: &std::path::Path, offset: u64, length: u64) -> ApiResult<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)
        .map_err(|e| ApiError::Internal(format!("{}: {e}", path.display())))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| ApiError::Internal(format!("{}: {e}", path.display())))?;
    let mut bytes = vec![0_u8; length as usize];
    file.read_exact(&mut bytes)
        .map_err(|e| ApiError::Internal(format!("{}: {e}", path.display())))?;
    Ok(bytes)
}

fn body(status: StatusCode, bytes: Vec<u8>, range: Option<(u64, u64)>, total: u64) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(ACCEPT_RANGES, "bytes")
        .header(CACHE_CONTROL, "private, max-age=31536000")
        .header(CONTENT_LENGTH, bytes.len());
    if let Some((first, last)) = range {
        response = response.header(CONTENT_RANGE, format!("bytes {first}-{last}/{total}"));
    }
    response
        .body(Body::from(bytes))
        .unwrap_or_else(|_| server_error())
}

fn unsatisfiable(total: u64) -> Response {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(CONTENT_RANGE, format!("bytes */{total}"))
        .body(Body::empty())
        .unwrap_or_else(|_| server_error())
}

fn server_error() -> Response {
    axum::response::IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR)
}

type ParsedRange = Result<Option<(u64, u64)>, ()>;

fn parse_range(header: &str, total: u64) -> ParsedRange {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let (start, end) = spec.split_once('-').ok_or(())?;
    let (start, end) = (start.trim(), end.trim());
    let (first, last) = match (start.is_empty(), end.is_empty()) {
        (true, false) => {
            let suffix: u64 = end.parse().map_err(|_| ())?;
            if suffix == 0 || total == 0 {
                return Err(());
            }
            (total.saturating_sub(suffix), total - 1)
        }
        (false, true) => (start.parse().map_err(|_| ())?, total.saturating_sub(1)),
        (false, false) => {
            let first: u64 = start.parse().map_err(|_| ())?;
            let last: u64 = end.parse().map_err(|_| ())?;
            (first, last.min(total.saturating_sub(1)))
        }
        (true, true) => return Err(()),
    };
    if total == 0 || first >= total || last < first {
        return Err(());
    }
    Ok(Some((first, last)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_forms_resolve_to_inclusive_spans() {
        assert_eq!(parse_range("bytes=0-9", 100), Ok(Some((0, 9))));
        assert_eq!(parse_range("bytes=90-", 100), Ok(Some((90, 99))));
        assert_eq!(parse_range("bytes=-10", 100), Ok(Some((90, 99))));
        assert_eq!(parse_range("bytes=0-999", 100), Ok(Some((0, 99))));
        assert_eq!(parse_range("bytes=0-9,20-29", 100), Ok(None));
        assert_eq!(parse_range("items=0-9", 100), Ok(None));
        assert_eq!(parse_range("bytes=100-101", 100), Err(()));
        assert_eq!(parse_range("bytes=-0", 100), Err(()));
        assert_eq!(parse_range("bytes=abc-1", 100), Err(()));
    }

    #[test]
    fn store_paths_cannot_climb_out_of_the_layer_root() {
        let root = PathBuf::from("/store/root");
        assert_eq!(
            resolve(&root, "0/0.0.0.0.0").unwrap(),
            root.join("0/0.0.0.0.0")
        );
        for hostile in ["../secret", "0/../../secret", "/etc/passwd"] {
            assert!(
                resolve(&root, hostile).is_err(),
                "{hostile} must be refused"
            );
        }
    }
}
