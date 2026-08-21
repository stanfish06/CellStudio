use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, http::HeaderName};
use cellstudio_core::reader::ImageReader;
use cellstudio_core::{LayerId, dataset::Dataset};
use cellstudio_db::Project;
use cellstudio_db::queries::VersionCounter;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::json_body;
use crate::error::{ApiError, ApiResult};
use crate::events::Event;
use crate::routes::io;
use crate::state::{ActiveProject, AppState, assembled_layer, layout_of};
use crate::wire::{
    ChannelWire, HealthInfo, LayoutAdvisory, LevelWire, ProjectInfo, ReadStats, SESSION_HEADER,
    VersionsWire,
};

#[derive(Debug, Deserialize)]
pub struct OpenBody {
    path: String,
}

pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthInfo> {
    Json(HealthInfo {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        session: state.session(),
        reads: ReadStats {
            inflight: state.reads.inflight(),
            peak: state.reads.peak(),
            permits: state.config.decode_permits.max(2) as u64,
        },
    })
}

pub async fn get_project(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let active = state.require()?;
    let info = project_info(&active)?;
    Ok(session_json(&active.session_id, info))
}

pub async fn open_project(
    State(state): State<Arc<AppState>>,
    body: Result<Json<OpenBody>, JsonRejection>,
) -> ApiResult<Response> {
    let path = PathBuf::from(json_body(body)?.path);
    let reopening = state
        .current()
        .filter(|active| same_store(&active.source, &path))
        .map(|active| {
            (
                active.project.clone(),
                active.image.clone(),
                active.assembled_root.clone(),
                active.layout.clone(),
            )
        });

    let active = match reopening {
        Some((project, image, assembled_root, layout)) => Arc::new(ActiveProject {
            session_id: new_session_id(),
            image,
            project,
            source: path,
            assembled_root,
            layout,
        }),
        None => {
            let layout = state
                .read({
                    let path = path.clone();
                    move || validate_dataset(&path)
                })
                .await?;
            drop(state.take());
            let cache_bytes = state.config.brick_cache_bytes;
            let opened = state
                .read({
                    let path = path.clone();
                    move || open_validated(&path, cache_bytes, layout)
                })
                .await?;
            Arc::new(ActiveProject {
                session_id: new_session_id(),
                image: opened.image,
                project: opened.project,
                source: path,
                assembled_root: opened.assembled_root,
                layout: opened.layout,
            })
        }
    };

    let info = project_info(&active)?;
    state.publish(active.clone());
    state.events.publish(Event::Versions {
        versions: info.versions.clone(),
    });
    if state.config.build_proxy {
        io::schedule_proxy(&state, &active);
    }
    tracing::info!(session = %active.session_id, source = ?active.source, "project opened");
    Ok(session_json(&active.session_id, info))
}

struct Opened {
    project: Arc<Project>,
    image: Arc<ImageReader>,
    assembled_root: PathBuf,
    layout: cellstudio_core::dataset::LayoutReport,
}

fn validate_dataset(path: &std::path::Path) -> ApiResult<cellstudio_core::LayoutReport> {
    let source = cellstudio_core::open(path)?;
    Ok(layout_of(&source)?)
}

fn open_validated(
    path: &std::path::Path,
    cache_bytes: usize,
    layout: cellstudio_core::LayoutReport,
) -> ApiResult<Opened> {
    let project = Arc::new(Project::create_or_open(path)?);
    let (assembled_root, dataset) = assembled_layer(&project, path)?;
    let image = Arc::new(ImageReader::new(Arc::new(dataset), cache_bytes));
    register_labels(&project, &image);
    attach_existing_proxy(&project, &image);
    Ok(Opened {
        project,
        image,
        assembled_root,
        layout,
    })
}

fn register_labels(project: &Project, image: &ImageReader) {
    if !project.has_labels() {
        return;
    }
    let path = project.labels_store_path();
    match cellstudio_core::open(&path) {
        Ok(labels) => image.register_layer(LayerId::Labels, Arc::new(labels)),
        Err(e) => tracing::warn!("label store at {path:?} is not readable: {e}"),
    }
}

fn attach_existing_proxy(project: &Project, image: &ImageReader) {
    let path = project
        .cache_dir()
        .join(cellstudio_core::volume::PROXY_STORE_NAME);
    if !path.exists() {
        return;
    }
    match cellstudio_core::volume::ProxyStore::open(&path) {
        Ok(proxy) => {
            tracing::info!(level = proxy.level, "reusing volume proxy");
            image.attach_proxy(proxy);
        }
        Err(e) => tracing::warn!("ignoring unreadable volume proxy at {path:?}: {e}"),
    }
}

pub async fn get_settings(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let active = state.require()?;
    let settings = active.project.db.settings()?;
    Ok(session_json(&active.session_id, settings))
}

pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let settings = json_body(body)?;
    if !settings.is_object() {
        return Err(ApiError::BadRequest(
            "settings must be a JSON object".to_owned(),
        ));
    }
    active.project.db.put_settings(&settings)?;
    let versions = VersionsWire::new(&active.session_id, active.versions()?);
    state.events.publish(Event::Versions { versions });
    Ok(session_response(
        &active.session_id,
        StatusCode::NO_CONTENT.into_response(),
    ))
}

pub fn bump_and_announce(
    state: &AppState,
    active: &ActiveProject,
    counter: VersionCounter,
) -> ApiResult<()> {
    active.project.db.bump(counter)?;
    let versions = VersionsWire::new(&active.session_id, active.versions()?);
    state.events.publish(Event::Versions { versions });
    Ok(())
}

pub fn project_info(active: &ActiveProject) -> ApiResult<ProjectInfo> {
    let dataset: &Dataset = active.dataset();
    Ok(ProjectInfo {
        session_id: active.session_id.clone(),
        source_path: active.source.display().to_string(),
        project_path: active.project.root.display().to_string(),
        dims: dataset.dims,
        dtype: dataset.dtype,
        scale: dataset.scale,
        levels: dataset.levels.iter().map(LevelWire::from).collect(),
        channels: dataset.channels.iter().map(ChannelWire::from).collect(),
        versions: VersionsWire::new(&active.session_id, active.versions()?),
        layout: LayoutAdvisory::from(&active.layout),
        has_labels: active.project.has_labels(),
    })
}

pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn same_store(a: &PathBuf, b: &PathBuf) -> bool {
    let normal = |p: &PathBuf| std::path::absolute(p).unwrap_or_else(|_| p.clone());
    normal(a) == normal(b)
}

pub fn session_json<T: serde::Serialize>(session: &str, body: T) -> Response {
    session_response(session, Json(body).into_response())
}

pub fn session_response(session: &str, mut response: Response) -> Response {
    if let Ok(value) = axum::http::HeaderValue::from_str(session) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(SESSION_HEADER), value);
    }
    response
}
