//! Mask edits. Every route here fences on the session header before the active project is
//! resolved or any lock is taken (design M20), translates wire types, and hands the work to
//! [`ProjectEditCoordinator`]; none of them reproduces the write ordering.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::{Json, http::HeaderName};
use cellstudio_core::axes::Axis;
use serde::Deserialize;

use crate::auth::json_body;
use crate::edit::{MaskCommand, RESERVE_DEFAULT, Stroke};
use crate::error::{ApiError, ApiResult};
use crate::routes::project::session_json;
use crate::state::{ActiveProject, AppState};
use crate::wire::{LabelLeaseWire, MaskEditWire, SESSION_HEADER};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveBody {
    #[serde(default)]
    count: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrokeBody {
    t: u64,
    label: u32,
    mode: MaskMode,
    /// Level-0 pixels along x; the other axes scale by voxel size.
    radius: f64,
    /// The axis a slice-view disk is pinned to; null is a 3D orb.
    #[serde(default)]
    plane: Option<Axis>,
    /// Stamp centres in level-0 pixels, `[z, y, x]`, fractional.
    stamps: Vec<[f64; 3]>,
    /// Eraser scope: clear only this label, or any label when null.
    #[serde(default)]
    only: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaskMode {
    Paint,
    Erase,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBody {
    t: u64,
    label: u32,
}

/// Advances the id counter and nothing else: selecting the brush on a project the user never
/// paints leaves no store behind (design M1, M10).
pub async fn reserve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<ReserveBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = fenced(&state, &headers)?;
    let count = body
        .map(|Json(body)| body)
        .unwrap_or_default()
        .count
        .unwrap_or(RESERVE_DEFAULT);
    let session = active.session_id.clone();
    let (first, count) = state
        .io(move || Ok(active.coordinator.reserve(&active.session_id, count)?))
        .await?;
    Ok(session_json(&session, LabelLeaseWire { first, count }))
}

pub async fn stroke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<StrokeBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = fenced(&state, &headers)?;
    let body = json_body(body)?;
    let command = MaskCommand::Stroke(Stroke {
        t: body.t,
        label: body.label,
        erase: body.mode == MaskMode::Erase,
        radius: body.radius,
        plane: match body.plane {
            Some(Axis::T | Axis::C) => {
                return Err(ApiError::BadRequest("plane must be z, y or x".to_owned()));
            }
            plane => plane,
        },
        stamps: body.stamps,
        only: body.only,
    });
    run(&state, active, command).await
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<DeleteBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = fenced(&state, &headers)?;
    let body = json_body(body)?;
    run(
        &state,
        active,
        MaskCommand::Delete {
            t: body.t,
            label: body.label,
        },
    )
    .await
}

pub async fn run(
    state: &Arc<AppState>,
    active: Arc<ActiveProject>,
    command: MaskCommand,
) -> ApiResult<Response> {
    let session = active.session_id.clone();
    let commit = state
        .io(move || {
            Ok(active
                .coordinator
                .execute(&active.image, &active.session_id, command)?)
        })
        .await?;
    Ok(session_json(&session, MaskEditWire::new(&session, commit)))
}

/// The open project, only when it is the one the request addresses. A mutation carrying no
/// session identifier is refused rather than applied to whichever project is current.
pub fn fenced(state: &AppState, headers: &HeaderMap) -> ApiResult<Arc<ActiveProject>> {
    let presented = headers
        .get(HeaderName::from_static(SESSION_HEADER))
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{SESSION_HEADER} is required on a request that modifies project state"
            ))
        })?;
    state.require_for(presented)
}
