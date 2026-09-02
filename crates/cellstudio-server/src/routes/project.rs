use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path as AxumPath, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, http::HeaderName};
use cellstudio_core::labels;
use cellstudio_core::reader::ImageReader;
use cellstudio_core::{LayerId, dataset::Dataset};
use cellstudio_db::queries::VersionCounter;
use cellstudio_db::{DbError, Project, StoredLabel};
use serde::Deserialize;
use serde_json::Value;

use crate::auth::json_body;
use crate::edit::{EditCommand, GraphCommand, ProjectEditCoordinator};
use crate::error::{ApiError, ApiResult};
use crate::events::{Event, EventBus};
use crate::routes::{io, mask};
use crate::state::{ActiveProject, AppState, assembled_layer, layout_of};
use crate::wire::{
    ChannelWire, HealthInfo, LabelDefinitionWire, LabelDefinitionsWire, LayoutAdvisory, LevelWire,
    ProjectInfo, ReadStats, SESSION_HEADER, VersionsWire,
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
    // the coordinator comes with it: both wrappers address one labels.zarr.
    let reopening = state
        .current()
        .filter(|active| same_store(&active.source, &path));

    let active = match reopening {
        Some(previous) => Arc::new(ActiveProject {
            session_id: new_session_id(),
            image: previous.image.clone(),
            project: previous.project.clone(),
            coordinator: previous.coordinator.clone(),
            source: path,
            assembled_root: previous.assembled_root.clone(),
            layout: previous.layout.clone(),
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
            let events = state.events.clone();
            let opened = state
                .read({
                    let path = path.clone();
                    move || open_validated(&path, cache_bytes, layout, events)
                })
                .await?;
            Arc::new(ActiveProject {
                session_id: new_session_id(),
                image: opened.image,
                project: opened.project,
                coordinator: opened.coordinator,
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
    io::schedule_inventory(&state, &active);
    tracing::info!(session = %active.session_id, source = ?active.source, "project opened");
    Ok(session_json(&active.session_id, info))
}

struct Opened {
    project: Arc<Project>,
    image: Arc<ImageReader>,
    coordinator: Arc<ProjectEditCoordinator>,
    assembled_root: PathBuf,
    layout: cellstudio_core::dataset::LayoutReport,
}

fn validate_dataset(path: &std::path::Path) -> ApiResult<cellstudio_core::LayoutReport> {
    let source = cellstudio_core::open(path)?;
    layout_of(&source)
}

fn open_validated(
    path: &std::path::Path,
    cache_bytes: usize,
    layout: cellstudio_core::LayoutReport,
    events: EventBus,
) -> ApiResult<Opened> {
    let project = Arc::new(Project::create_or_open(path)?);
    let (assembled_root, dataset) = assembled_layer(&project, path)?;
    let image = Arc::new(ImageReader::new(Arc::new(dataset), cache_bytes));
    register_labels(&project, &image)?;
    attach_existing_proxy(&project, &image);
    let coordinator = ProjectEditCoordinator::new(project.clone(), events);
    let rolled = coordinator.recover(image.dataset())?;
    if rolled > 0 {
        tracing::warn!(rolled, "rolled back mask edits an earlier run left pending");
    }
    Ok(Opened {
        project,
        image,
        coordinator,
        assembled_root,
        layout,
    })
}

/// Adopts the project's label store, refusing the project when it does not satisfy the
/// contract: a half-usable project with an overlay you cannot edit and no visible reason is
/// worse than a refusal that names the reason Called at open and again after a
/// re-chunk rebuilds the reader.
pub fn register_labels(project: &Project, image: &ImageReader) -> ApiResult<()> {
    if !project.has_labels() {
        return Ok(());
    }
    let path = project.labels_store_path();
    let refuse = |check: String| {
        ApiError::BadRequest(format!(
            "label store at {} does not satisfy the label contract: {check}",
            path.display()
        ))
    };
    let labels = cellstudio_core::open(&path).map_err(|e| refuse(e.to_string()))?;
    labels::check_contract(&labels, image.dataset()).map_err(|e| refuse(e.to_string()))?;
    check_label_ids(project).map_err(|e| refuse(e.to_string()))?;
    image.register_layer(LayerId::Labels, Arc::new(labels));
    Ok(())
}

/// The id half of the contract, which needs the database rather than the store: the overlay
/// hands the fragment shader a float, so an adopted store whose recorded ids run past 2^24
/// cannot be drawn Reserving nothing reads the counter without moving it.
fn check_label_ids(project: &Project) -> Result<(), cellstudio_core::labels::ContractError> {
    let next = match project.db.reserve_label_ids(0) {
        Ok(next) => u64::from(next),
        Err(DbError::LabelIdsExhausted { first, .. }) => first.max(1) as u64,
        // a counter or table this check cannot read is not a contract failure
        Err(_) => return Ok(()),
    };
    labels::check_max_label(next.saturating_sub(1))
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
        has_graph: active.project.db.has_graph()?,
        label_definitions: definitions_of(active)?,
    })
}

fn definitions_of(active: &ActiveProject) -> ApiResult<Vec<LabelDefinitionWire>> {
    Ok(active
        .project
        .db
        .label_definitions()?
        .iter()
        .map(LabelDefinitionWire::from)
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct DefinitionsBody {
    definitions: Vec<StoredLabel>,
}

pub async fn get_label_definitions(State(state): State<Arc<AppState>>) -> ApiResult<Response> {
    let active = state.require()?;
    Ok(session_json(
        &active.session_id,
        LabelDefinitionsWire {
            session_id: active.session_id.clone(),
            definitions: definitions_of(&active)?,
            edit: None,
        },
    ))
}

/// Replaces the stored list; names still on cells stay listed through the union.
pub async fn put_label_definitions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<DefinitionsBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    let body = json_body(body)?;
    active.project.db.put_label_definitions(&body.definitions)?;
    let versions = VersionsWire::new(&active.session_id, active.versions()?);
    state.events.publish(Event::Versions { versions });
    Ok(session_json(
        &active.session_id,
        LabelDefinitionsWire {
            session_id: active.session_id.clone(),
            definitions: definitions_of(&active)?,
            edit: None,
        },
    ))
}

/// Strips the name from every cell as one journaled edit when any carries it, then drops
/// it from the stored list. Ordered so a failure between the two leaves the name listed
/// with zero uses rather than on cells the sheet cannot show.
pub async fn delete_label_definition(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    let in_use = active
        .project
        .db
        .label_definitions()?
        .iter()
        .any(|d| d.name == name && d.uses > 0);
    let edit = match in_use {
        true => Some(
            mask::commit(
                &state,
                active.clone(),
                EditCommand::Graph(GraphCommand::StripLabel { name: name.clone() }),
            )
            .await?,
        ),
        false => None,
    };
    let remaining: Vec<StoredLabel> = active
        .project
        .db
        .label_definitions()?
        .into_iter()
        .filter(|d| d.name != name)
        .map(|d| StoredLabel {
            name: d.name,
            color: d.color,
        })
        .collect();
    active.project.db.put_label_definitions(&remaining)?;
    let versions = VersionsWire::new(&active.session_id, active.versions()?);
    state.events.publish(Event::Versions { versions });
    Ok(session_json(
        &active.session_id,
        LabelDefinitionsWire {
            session_id: active.session_id.clone(),
            definitions: definitions_of(&active)?,
            edit,
        },
    ))
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
