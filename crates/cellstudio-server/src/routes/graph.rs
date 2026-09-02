use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use cellstudio_db::LabelScope;
use cellstudio_db::queries::Bbox;
use serde::Deserialize;

use crate::auth::json_body;
use crate::edit::{EditCommand, GraphCommand};
use crate::error::{ApiError, ApiResult};
use crate::routes::mask;
use crate::routes::project::session_json;
use crate::state::AppState;
use crate::wire::{CellRowWire, EditEntryWire, LabelStateWire, LineageTreeWire};

const DEFAULT_EDIT_LIMIT: u32 = 100;

#[derive(Debug, Deserialize)]
pub struct CellsQuery {
    t0: u64,
    t1: u64,
    #[serde(default)]
    bbox: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LineageQuery {
    cell: u32,
}

#[derive(Debug, Deserialize)]
pub struct EditsQuery {
    #[serde(default)]
    limit: Option<u32>,
}

pub async fn cells(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CellsQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let bbox = query.bbox.as_deref().map(parse_bbox).transpose()?;
    let rows = active.project.db.cells_window(query.t0, query.t1, bbox)?;
    let wire: Vec<CellRowWire> = rows.iter().map(CellRowWire::from).collect();
    Ok(session_json(&active.session_id, wire))
}

pub async fn lineage(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LineageQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let tree = active.project.db.lineage(query.cell)?;
    Ok(session_json(
        &active.session_id,
        LineageTreeWire::from(&tree),
    ))
}

pub async fn edits(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EditsQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let limit = query.limit.unwrap_or(DEFAULT_EDIT_LIMIT);
    let entries = active.project.db.edits(limit)?;
    let wire: Vec<EditEntryWire> = entries.iter().map(EditEntryWire::from).collect();
    Ok(session_json(&active.session_id, wire))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkBody {
    parent_id: u32,
    child_id: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkBody {
    cell_id: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLabelsBody {
    cell_id: u32,
    scope: LabelScope,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct LabelStatesQuery {
    cell: u32,
}

/// Per-definition state for the popover: on the cell, and how much of its chain.
pub async fn label_states(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LabelStatesQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let states = active
        .project
        .db
        .label_states(query.cell)
        .map_err(|e| crate::error::ApiError::from(crate::edit::EditError::Graph(e)))?;
    let wire: Vec<LabelStateWire> = states.iter().map(LabelStateWire::from).collect();
    Ok(session_json(&active.session_id, wire))
}

/// One label edit, journaled beside link/unlink/cut.
pub async fn set_labels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<SetLabelsBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    let body = json_body(body)?;
    if body.add.is_empty() && body.remove.is_empty() {
        return Err(ApiError::BadRequest(
            "a label edit names at least one label to add or remove".to_owned(),
        ));
    }
    mask::run(
        &state,
        active,
        EditCommand::Graph(GraphCommand::SetLabels {
            cell_id: body.cell_id,
            scope: body.scope,
            add: body.add,
            remove: body.remove,
        }),
    )
    .await
}

/// Session-fenced like every mask mutation; the coordinator commits validation, the link,
/// re-materialization, journal row and version bump in one transaction.
pub async fn link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<LinkBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    let body = json_body(body)?;
    mask::run(
        &state,
        active,
        EditCommand::Graph(GraphCommand::Link {
            parent_id: body.parent_id,
            child_id: body.child_id,
        }),
    )
    .await
}

/// Unlink is addressed by the selected cell; its maximal unbranched chain is derived inside
/// the transaction.
pub async fn unlink(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<UnlinkBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    let body = json_body(body)?;
    mask::run(
        &state,
        active,
        EditCommand::Graph(GraphCommand::Unlink {
            cell_id: body.cell_id,
        }),
    )
    .await
}

/// Cutting one link splits its chain; Unlink deletes a whole one.
pub async fn cut(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<LinkBody>, JsonRejection>,
) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    let body = json_body(body)?;
    mask::run(
        &state,
        active,
        EditCommand::Graph(GraphCommand::Cut {
            parent_id: body.parent_id,
            child_id: body.child_id,
        }),
    )
    .await
}

/// The unified undo: the newest un-undone row whatever its domain, dispatched on it by the
/// coordinator.
pub async fn undo(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    mask::run(&state, active, EditCommand::Undo).await
}

pub async fn redo(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Response> {
    let active = mask::fenced(&state, &headers)?;
    mask::run(&state, active, EditCommand::Redo).await
}

fn parse_bbox(raw: &str) -> ApiResult<Bbox> {
    let parts: Result<Vec<f64>, _> = raw
        .split(',')
        .map(str::trim)
        .map(str::parse::<f64>)
        .collect();
    let parts = parts.map_err(|_| {
        ApiError::BadRequest(format!("bbox must be y0,y1,x0,x1 numbers, got `{raw}`"))
    })?;
    match parts[..] {
        [y0, y1, x0, x1] => Ok(Bbox {
            z: Bbox::UNBOUNDED.z,
            y: [y0.min(y1), y0.max(y1)],
            x: [x0.min(x1), x0.max(x1)],
        }),
        _ => Err(ApiError::BadRequest(format!(
            "bbox must be four numbers y0,y1,x0,x1, got `{raw}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_is_ordered_and_leaves_z_unbounded() {
        let bbox = parse_bbox("10, 20, 30, 40").expect("bbox");
        assert_eq!(bbox.y, [10.0, 20.0]);
        assert_eq!(bbox.x, [30.0, 40.0]);
        assert!(bbox.z[0].is_infinite() && bbox.z[1].is_infinite());
        assert_eq!(parse_bbox("20,10,40,30").expect("bbox").y, [10.0, 20.0]);
        assert!(parse_bbox("1,2,3").is_err());
        assert!(parse_bbox("a,b,c,d").is_err());
    }
}
