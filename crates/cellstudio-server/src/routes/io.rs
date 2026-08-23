use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::response::Response;
use cellstudio_core::axes::Dims;
use cellstudio_core::reader::ImageReader;
use cellstudio_core::rechunk::DEFAULT_BRICK;
use cellstudio_core::volume::{PROXY_STORE_NAME, build_proxy, choose_proxy_level};
use cellstudio_db::queries::VersionCounter;
use serde::Deserialize;

use crate::auth::json_body;
use crate::error::{ApiError, ApiResult};
use crate::jobs::{JobHandle, JobKind};
use crate::routes::project::{bump_and_announce, register_labels, session_json, session_response};
use crate::state::{ActiveProject, AppState, WORKING_COPY_NAME};
use crate::wire::JobRef;

#[derive(Debug, Deserialize)]
pub struct PathBody {
    path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RechunkBody {
    /// Brick extent.
    #[serde(default)]
    z: Option<u64>,
    #[serde(default)]
    y: Option<u64>,
    #[serde(default)]
    x: Option<u64>,
}

pub async fn list_jobs(State(state): State<Arc<AppState>>) -> Response {
    let body = axum::response::IntoResponse::into_response(Json(state.jobs.list()));
    match state.session() {
        Some(session) => session_response(&session, body),
        None => body,
    }
}

pub async fn start_import(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    body: Result<Json<PathBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let kind = match kind.as_str() {
        "tracks" => JobKind::ImportTracks,
        "labels" => JobKind::ImportLabels,
        other => {
            return Err(ApiError::BadRequest(format!(
                "unknown import kind `{other}`"
            )));
        }
    };
    let path = PathBuf::from(json_body(body)?.path);
    if !path.exists() {
        return Err(ApiError::NotFound(format!(
            "{} does not exist",
            path.display()
        )));
    }

    let handle = state.jobs.start(&active.session_id, kind);
    let id = handle.id.clone();
    handle.fail(match kind {
        JobKind::ImportTracks => {
            "tracking import is not implemented yet (needs cellstudio-core::tracks::parse_stream \
             and cellstudio-db staged import)"
        }
        _ => {
            "label mask import is not implemented yet (needs cellstudio-core label import). \
             An importer owes more than the voxels: it must seed mask_labels with every \
             (t, label) it writes and mask_extent with each label's exact area and centroid \
             sums, or id reservation re-issues a live id and incremental cell stats count \
             only the voxels this session painted"
        }
    });
    Ok(session_json(&active.session_id, JobRef { id }))
}

pub async fn start_rechunk(
    State(state): State<Arc<AppState>>,
    body: Result<Json<RechunkBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let requested = body.map(|Json(b)| b).unwrap_or_default();
    let target = Dims {
        t: 1,
        c: 1,
        z: requested.z.unwrap_or(DEFAULT_BRICK.z),
        y: requested.y.unwrap_or(DEFAULT_BRICK.y),
        x: requested.x.unwrap_or(DEFAULT_BRICK.x),
    };
    if [target.z, target.y, target.x].contains(&0) {
        return Err(ApiError::BadRequest(
            "brick extents must be positive".to_owned(),
        ));
    }

    let handle = Arc::new(state.jobs.start(&active.session_id, JobKind::Rechunk));
    let id = handle.id.clone();
    let out = active.project.cache_dir().join(WORKING_COPY_NAME);
    let session = active.session_id.clone();
    let worker = state.clone();
    let source = active.source.clone();
    let cache_bytes = state.config.brick_cache_bytes;

    tokio::task::spawn_blocking(move || {
        let progress = handle.clone();
        let dataset = match cellstudio_core::open(&source) {
            Ok(dataset) => dataset,
            Err(e) => return handle.fail(e.to_string()),
        };
        let written = cellstudio_core::rechunk(&dataset, &out, target, &|fraction| {
            progress.progress(fraction)
        });
        match written {
            Ok(path) => adopt_working_copy(&worker, &session, &path, cache_bytes, &handle),
            Err(e) => handle.fail(e.to_string()),
        }
    });

    Ok(session_json(&active.session_id, JobRef { id }))
}

fn adopt_working_copy(
    state: &AppState,
    session: &str,
    path: &std::path::Path,
    cache_bytes: usize,
    handle: &JobHandle,
) {
    if handle.cancelled() || !state.is_current(session) {
        return handle.cancel("session was replaced before the working copy was adopted");
    }
    let Some(previous) = state.current() else {
        return handle.cancel("no project is open");
    };
    let dataset = match cellstudio_core::open(path) {
        Ok(dataset) => dataset,
        Err(e) => return handle.fail(format!("working copy is not readable: {e}")),
    };
    let image = Arc::new(ImageReader::new(Arc::new(dataset), cache_bytes));
    // the rebuilt reader knows no layers; without this the label overlay silently disappears
    // after a re-chunk for the rest of the session (design M1)
    if let Err(e) = register_labels(&previous.project, &image) {
        return handle.fail(format!("label store cannot be re-registered: {e}"));
    }
    if let Some(level) = previous.image.proxy_level() {
        tracing::debug!(
            level,
            "working copy adopted; the existing proxy stays attached"
        );
    }
    let adopted = Arc::new(ActiveProject {
        session_id: previous.session_id.clone(),
        image,
        project: previous.project.clone(),
        coordinator: previous.coordinator.clone(),
        source: previous.source.clone(),
        assembled_root: path.to_path_buf(),
        layout: previous.layout.clone(),
    });
    if handle.cancelled() || !state.is_current(session) {
        return handle.cancel("session was replaced while the working copy was being opened");
    }
    state.publish(adopted.clone());
    if let Err(e) = bump_and_announce(state, &adopted, VersionCounter::Image) {
        tracing::error!("image version bump failed after re-chunk: {e}");
    }
    handle.finish(Some(format!("working copy at {}", path.display())));
}

fn volume_chunks_per_timepoint(
    dataset: &cellstudio_core::Dataset,
    level: u32,
) -> Result<u64, cellstudio_core::OpenError> {
    let level = dataset.level(level)?;
    let (dims, chunks) = (level.dims, level.chunks);
    let along = |extent: u64, chunk: u64| extent.div_ceil(chunk.max(1));
    Ok(along(dims.z, chunks.z) * along(dims.y, chunks.y) * along(dims.x, chunks.x))
}

const MAX_VOLUME_CHUNKS_WITHOUT_PROXY: u64 = 8;

pub fn schedule_proxy(state: &Arc<AppState>, active: &Arc<ActiveProject>) {
    if active.image.proxy_level().is_some() {
        return;
    }
    let level = choose_proxy_level(active.dataset(), state.config.volume_budget_bytes);
    match volume_chunks_per_timepoint(active.dataset(), level) {
        Ok(chunks) if chunks <= MAX_VOLUME_CHUNKS_WITHOUT_PROXY => {
            tracing::info!(
                level,
                chunks,
                "pyramid already reads volumes in few chunks; skipping the proxy"
            );
            return;
        }
        Err(e) => {
            tracing::warn!("cannot size the volume read at level {level}: {e}");
            return;
        }
        _ => {}
    }
    let out = active.project.cache_dir().join(PROXY_STORE_NAME);
    let handle = Arc::new(state.jobs.start(&active.session_id, JobKind::Proxy));
    let session = active.session_id.clone();
    let reader = active.image.clone();
    let worker = state.clone();

    tokio::task::spawn_blocking(move || {
        let progress = handle.clone();
        match build_proxy(&reader, level, &out, &|fraction| {
            progress.progress(fraction)
        }) {
            Ok(proxy) => {
                if handle.cancelled() || !worker.is_current(&session) {
                    return handle.cancel("session was replaced before the proxy was attached");
                }
                reader.attach_proxy(proxy);
                handle.finish(Some(format!("volume proxy at level {level}")));
            }
            Err(e) => handle.fail(e.to_string()),
        }
    });
}
