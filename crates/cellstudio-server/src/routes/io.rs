use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use cellstudio_core::axes::Dims;
use cellstudio_core::labels;
use cellstudio_core::reader::ImageReader;
use cellstudio_core::rechunk::DEFAULT_BRICK;
use cellstudio_core::tracks::open_tracking;
use cellstudio_core::volume::{PROXY_STORE_NAME, build_proxy, choose_proxy_level};
use cellstudio_db::inventory::store_identity;
use cellstudio_db::queries::VersionCounter;
use cellstudio_db::{ImportError, Project};
use serde::Deserialize;

use crate::auth::json_body;
use crate::error::{ApiError, ApiResult};
use crate::events::Event;
use crate::jobs::{JobHandle, JobKind};
use crate::routes::mask::fenced;
use crate::routes::project::{bump_and_announce, register_labels, session_json, session_response};
use crate::state::{ActiveProject, AppState, WORKING_COPY_NAME};
use crate::wire::{JobRef, VersionsWire};

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
    headers: HeaderMap,
    body: Result<Json<PathBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = fenced(&state, &headers)?;
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
    if kind == JobKind::ImportTracks {
        return start_tracks_import(&state, &active, path);
    }

    let handle = state.jobs.start(&active.session_id, kind);
    let id = handle.id.clone();
    handle.fail(
        "label mask import is not implemented yet (needs cellstudio-core label import). \
         An importer owes more than the voxels: it must seed mask_labels with every \
         (t, label) it writes and mask_extent with each label's exact area and centroid \
         sums, or id reservation re-issues a live id and incremental cell stats count \
         only the voxels this session painted",
    );
    Ok(session_json(&active.session_id, JobRef { id }))
}

/// Progress budget: staging reads the whole file, validation and materialization are
/// index-backed SQL over what it staged.
const IMPORT_STAGE_SHARE: f32 = 0.8;
const IMPORT_VALIDATED_AT: f32 = 0.9;

/// The per-process import lock. one import runs at a time, released when the
/// job body finishes on any path.
struct ImportLock(Arc<AppState>);

impl ImportLock {
    fn acquire(state: &Arc<AppState>) -> Option<Self> {
        state
            .import_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        Some(Self(state.clone()))
    }
}

impl Drop for ImportLock {
    fn drop(&mut self) {
        self.0.import_active.store(false, Ordering::SeqCst);
    }
}

fn start_tracks_import(
    state: &Arc<AppState>,
    active: &Arc<ActiveProject>,
    path: PathBuf,
) -> ApiResult<Response> {
    // the v1 policy and the inventory dependency are checked before a job exists: a request
    // that can never publish is refused, not failed later
    if let Some(reason) = active.project.db.import_blocker()? {
        return Err(ApiError::Conflict(reason));
    }
    if active.project.has_labels() && active.project.db.inventory_pending()? {
        return Err(ApiError::Conflict(
            "the label store inventory has not completed; tracking import validates mask \
             resolution against it"
                .to_owned(),
        ));
    }
    let Some(lock) = ImportLock::acquire(state) else {
        return Err(ApiError::Conflict(
            "an import is already running; one import runs at a time".to_owned(),
        ));
    };

    let handle = Arc::new(state.jobs.start(&active.session_id, JobKind::ImportTracks));
    let id = handle.id.clone();
    let session = active.session_id.clone();
    let project = active.project.clone();
    let worker = state.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = lock;
        run_tracks_import(&worker, &session, &project, &path, &handle);
    });
    Ok(session_json(&active.session_id, JobRef { id }))
}

fn run_tracks_import(
    state: &AppState,
    session: &str,
    project: &Project,
    path: &std::path::Path,
    handle: &Arc<JobHandle>,
) {
    let stream = match open_tracking(path) {
        Ok(stream) => stream,
        Err(e) => return handle.fail(e.to_string()),
    };
    let probe = stream.progress_probe();
    let definitions = stream.header.metadata.label_definitions.clone();
    let colors = stream.header.metadata.label_colors.clone();
    let progress = handle.clone();
    let staged = project.db.stage_records(stream.records, &move |_| {
        progress.progress(probe.fraction() * IMPORT_STAGE_SHARE)
    });
    // stage_records clears staging itself on every abort path
    if let Err(e) = staged {
        return handle.fail(e.to_string());
    }
    handle.progress(IMPORT_STAGE_SHARE);

    let abort = |message: String| {
        if let Err(e) = project.db.clear_staging() {
            tracing::error!("staging cleanup after an aborted import failed: {e}");
        }
        message
    };
    let offenders = match project.db.validate_staged(project.has_labels()) {
        Ok(offenders) => offenders,
        Err(e) => return handle.fail(abort(e.to_string())),
    };
    if !offenders.is_empty() {
        return handle.fail(abort(validation_message(&offenders)));
    }
    handle.progress(IMPORT_VALIDATED_AT);

    if handle.cancelled() || !state.is_current(session) {
        return handle.cancel(abort(
            "session was replaced before the import was published".to_owned(),
        ));
    }
    let summary = match project.db.materialize_staged(&definitions, &colors) {
        Ok(summary) => summary,
        Err(e) => return handle.fail(abort(e.to_string())),
    };
    // materialize_staged bumps version.graph inside its transaction; announce, don't re-bump.
    // An empty `tracks` list means "refetch everything".
    match project.db.versions() {
        Ok(versions) => {
            state.events.publish(Event::GraphChanged {
                session_id: session.to_owned(),
                graph_version: versions.graph,
                tracks: Vec::new(),
            });
            state.events.publish(Event::Versions {
                versions: VersionsWire::new(session, versions),
            });
        }
        Err(e) => tracing::error!("cannot announce versions after import: {e}"),
    }
    handle.finish(Some(format!(
        "imported {} cells, {} links, {} tracks, {} divisions",
        summary.cells, summary.links, summary.tracks, summary.divisions
    )));
}

/// Snapshots live under the project container.; no file dialog, fixed destination.
const SNAPSHOTS_DIR: &str = "snapshots";

/// Starts the tracking snapshot job (`POST /export/tracks`): one read transaction streams
/// `cells` + `links` through gzip into a temp sibling, renamed into `snapshots/` only after
/// cancellation and session currency re-check. The completion message carries the path.
pub async fn start_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let active = fenced(&state, &headers)?;
    if !active.project.db.has_graph()? {
        return Err(ApiError::Conflict(
            "the project has no track graph to snapshot".to_owned(),
        ));
    }
    let handle = Arc::new(state.jobs.start(&active.session_id, JobKind::Export));
    let id = handle.id.clone();
    let session = active.session_id.clone();
    let project = active.project.clone();
    let worker = state.clone();
    tokio::task::spawn_blocking(move || {
        run_tracks_export(&worker, &session, &project, &handle);
    });
    Ok(session_json(&active.session_id, JobRef { id }))
}

fn run_tracks_export(state: &AppState, session: &str, project: &Project, handle: &Arc<JobHandle>) {
    let dir = project.root.join(SNAPSHOTS_DIR);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return handle.fail(format!("cannot create {}: {e}", dir.display()));
    }
    // temp sibling named by the job id: a failed or cancelled export removes it, and no
    // partial write ever carries a final-looking name
    let tmp = dir.join(format!(".tracking-{}.json.gz.tmp", handle.id));
    let written = write_snapshot(project, &tmp, handle);
    let summary = match written {
        Ok(summary) => summary,
        Err(message) => {
            let _ = std::fs::remove_file(&tmp);
            return handle.fail(message);
        }
    };
    if handle.cancelled() || !state.is_current(session) {
        let _ = std::fs::remove_file(&tmp);
        return handle.cancel("session was replaced before the snapshot was published");
    }
    let target = snapshot_target(&dir, &summary.created);
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return handle.fail(format!("cannot publish {}: {e}", target.display()));
    }
    handle.finish(Some(target.display().to_string()));
}

fn write_snapshot(
    project: &Project,
    tmp: &std::path::Path,
    handle: &Arc<JobHandle>,
) -> Result<cellstudio_db::export::ExportSummary, String> {
    let file = std::fs::File::create(tmp).map_err(|e| format!("cannot write a snapshot: {e}"))?;
    let mut out = std::io::BufWriter::new(flate2::write::GzEncoder::new(
        file,
        flate2::Compression::default(),
    ));
    let progress = handle.clone();
    let summary = project
        .db
        .export_graph(env!("CARGO_PKG_VERSION"), &mut out, &|fraction| {
            progress.progress(fraction)
        })
        .map_err(|e| e.to_string())?;
    let encoder = out
        .into_inner()
        .map_err(|e| format!("cannot flush the snapshot: {}", e.error()))?;
    encoder
        .finish()
        .map_err(|e| format!("cannot finish the snapshot: {e}"))?;
    Ok(summary)
}

/// `tracking-<UTC>.json.gz` from the RFC3339 `created` stamp (`2026-08-26T03:15:00Z` →
/// `20260826T031500Z`), with a numeric suffix on collision.
fn snapshot_target(dir: &std::path::Path, created: &str) -> PathBuf {
    let stamp: String = created
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut suffix = 0u32;
    loop {
        let name = match suffix {
            0 => format!("tracking-{stamp}.json.gz"),
            n => format!("tracking-{stamp}-{n}.json.gz"),
        };
        let path = dir.join(name);
        if !path.exists() {
            return path;
        }
        suffix += 1;
    }
}

fn validation_message(offenders: &[ImportError]) -> String {
    let head: Vec<String> = offenders
        .iter()
        .take(8)
        .map(|e| format!("cell {}: {}", e.cell_id, e.message))
        .collect();
    let more = offenders.len().saturating_sub(8);
    let listed = match more {
        0 => head.join("; "),
        _ => format!("{}; … {more} more", head.join("; ")),
    };
    format!("tracking import aborted, nothing was written — {listed}")
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
    // after a re-chunk for the rest of the session.
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

/// Schedules the one-time inventory of an adopted label store. a store at open
/// whose completeness marker is absent or names another store was not written by this app's
/// own create path, so every (t, label) it holds must be recorded before label-id
/// reservation or mask writes are allowed. Until the job publishes, both are refused.
pub fn schedule_inventory(state: &Arc<AppState>, active: &Arc<ActiveProject>) {
    let project = active.project.clone();
    if !project.has_labels() {
        // a requirement left behind by a store that no longer exists gates nothing real
        if let Err(e) = project.db.clear_inventory_requirement() {
            tracing::error!("cannot clear a stale inventory requirement: {e}");
        }
        return;
    }
    let store_path = project.labels_store_path();
    let Some(identity) = store_identity(&store_path) else {
        tracing::warn!("label store at {store_path:?} has no readable root metadata");
        return;
    };
    match project.db.inventory_marker() {
        Ok(Some(marker)) if marker == identity => {
            if let Err(e) = project.db.clear_inventory_requirement() {
                tracing::error!("cannot clear a satisfied inventory requirement: {e}");
            }
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("cannot read the inventory marker: {e}");
            return;
        }
    }
    if let Err(e) = project.db.require_inventory(&identity) {
        tracing::error!("cannot record the inventory requirement: {e}");
        return;
    }

    let handle = Arc::new(state.jobs.start(&active.session_id, JobKind::Inventory));
    let session = active.session_id.clone();
    let image = active.image.clone();
    let worker = state.clone();
    tokio::task::spawn_blocking(move || {
        let progress = handle.clone();
        let store = match labels::open_store(&store_path, image.dataset()) {
            Ok(store) => store,
            Err(e) => return handle.fail(e.to_string()),
        };
        let inventory =
            match labels::scan_inventory(&store, &|fraction| progress.progress(fraction)) {
                Ok(inventory) => inventory,
                Err(e) => return handle.fail(e.to_string()),
            };
        if !inventory.oversized.is_empty() || !inventory.multi_frame.is_empty() {
            return handle.fail(inventory_flags(&inventory));
        }
        if handle.cancelled() || !worker.is_current(&session) {
            return handle.cancel("session was replaced before the inventory was published");
        }
        match project
            .db
            .publish_inventory(&inventory.rows, inventory.max_id, &identity)
        {
            Ok(()) => handle.finish(Some(format!(
                "inventoried {} (frame, label) pairs, max id {}",
                inventory.rows.len(),
                inventory.max_id
            ))),
            Err(e) => handle.fail(e.to_string()),
        }
    });
}

fn inventory_flags(inventory: &labels::Inventory) -> String {
    let list = |ids: &[u32]| {
        let head: Vec<String> = ids.iter().take(8).map(u32::to_string).collect();
        let more = ids.len().saturating_sub(8);
        match more {
            0 => head.join(", "),
            _ => format!("{}, … {more} more", head.join(", ")),
        }
    };
    let mut parts = Vec::new();
    if !inventory.oversized.is_empty() {
        parts.push(format!(
            "ids past the renderable ceiling {}: {}",
            labels::MAX_LABEL_ID,
            list(&inventory.oversized)
        ));
    }
    if !inventory.multi_frame.is_empty() {
        parts.push(format!(
            "ids present on more than one frame: {}",
            list(&inventory.multi_frame)
        ));
    }
    format!(
        "the label store violates the one-id-one-frame contract and cannot be adopted — {}",
        parts.join("; ")
    )
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

#[cfg(test)]
mod tests {
    use super::snapshot_target;

    #[test]
    fn snapshot_names_are_filename_safe_and_collision_suffixed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let created = "2026-08-26T03:15:00Z";

        let first = snapshot_target(dir.path(), created);
        assert_eq!(
            first.file_name().unwrap().to_str().unwrap(),
            "tracking-20260826T031500Z.json.gz"
        );

        std::fs::write(&first, b"taken").expect("occupy");
        let second = snapshot_target(dir.path(), created);
        assert_eq!(
            second.file_name().unwrap().to_str().unwrap(),
            "tracking-20260826T031500Z-1.json.gz"
        );

        std::fs::write(&second, b"taken").expect("occupy");
        let third = snapshot_target(dir.path(), created);
        assert_eq!(
            third.file_name().unwrap().to_str().unwrap(),
            "tracking-20260826T031500Z-2.json.gz"
        );
    }
}
