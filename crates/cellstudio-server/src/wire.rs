use cellstudio_core::axes::{Dims, Dtype, Orientation, PhysicalScale};
use cellstudio_core::dataset::{ChannelMeta, LayoutReport, Level};
use cellstudio_core::reader::Histogram;
use cellstudio_db::queries::{CellRow, EditEntry, LineageTree, LinkRow, Versions};
use serde::Serialize;

use cellstudio_db::{GraphCommit, LabelDefinition, LabelState, TrackCoverage};

use crate::edit::{EditOutcome, MaskCommit};

pub const SESSION_HEADER: &str = "x-cellstudio-session";
pub const SHAPE_HEADER: &str = "x-cellstudio-shape";
pub const DTYPE_HEADER: &str = "x-cellstudio-dtype";
pub const LEVEL_HEADER: &str = "x-cellstudio-level";
pub const VOLUME_SOURCE_HEADER: &str = "x-cellstudio-volume-source";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthInfo {
    pub status: &'static str,
    pub version: &'static str,
    pub session: Option<String>,
    pub reads: ReadStats,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadStats {
    pub inflight: u64,
    pub peak: u64,
    pub permits: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionsWire {
    pub session_id: String,
    pub image: u64,
    pub labels: u64,
    pub graph: u64,
    pub settings: u64,
}

impl VersionsWire {
    pub fn new(session_id: &str, versions: Versions) -> Self {
        Self {
            session_id: session_id.to_owned(),
            image: versions.image,
            labels: versions.labels,
            graph: versions.graph,
            settings: versions.settings,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelWire {
    pub index: u32,
    pub dims: Dims,
    pub chunks: Dims,
    pub factor: [f64; 3],
}

impl From<&Level> for LevelWire {
    fn from(level: &Level) -> Self {
        Self {
            index: level.index,
            dims: level.dims,
            chunks: level.chunks,
            factor: level.factor,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelWire {
    pub name: String,
    pub color: Option<String>,
    pub window: Option<[f64; 2]>,
}

impl From<&ChannelMeta> for ChannelWire {
    fn from(channel: &ChannelMeta) -> Self {
        Self {
            name: channel.name.clone(),
            color: Some(channel.color.clone()),
            window: Some(channel.window),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Amplification {
    pub xy: f64,
    pub xz: f64,
    pub yz: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutAdvisory {
    pub hostile: bool,
    pub amplification: Amplification,
    pub affected_views: Vec<&'static str>,
}

impl From<&LayoutReport> for LayoutAdvisory {
    fn from(report: &LayoutReport) -> Self {
        let of = |orientation: Orientation| {
            report
                .view(orientation)
                .map(|v| v.amplification)
                .unwrap_or(1.0)
        };
        Self {
            hostile: report.hostile,
            amplification: Amplification {
                xy: of(Orientation::XY),
                xz: of(Orientation::XZ),
                yz: of(Orientation::YZ),
            },
            affected_views: report
                .hostile_views
                .iter()
                .copied()
                .map(orientation_name)
                .collect(),
        }
    }
}

pub fn orientation_name(orientation: Orientation) -> &'static str {
    match orientation {
        Orientation::XY => "xy",
        Orientation::XZ => "xz",
        Orientation::YZ => "yz",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub session_id: String,
    pub source_path: String,
    pub project_path: String,
    pub dims: Dims,
    pub dtype: Dtype,
    pub scale: Option<PhysicalScale>,
    pub levels: Vec<LevelWire>,
    pub channels: Vec<ChannelWire>,
    pub versions: VersionsWire,
    pub layout: LayoutAdvisory,
    pub has_labels: bool,
    /// Any `links` row exists — a snapshot at open/refetch, like the version counters.
    pub has_graph: bool,
    /// Stored definitions ∪ names on cells, with per-name use counts.
    pub label_definitions: Vec<LabelDefinitionWire>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelDefinitionWire {
    pub name: String,
    pub uses: u64,
    pub color: Option<String>,
}

impl From<&LabelDefinition> for LabelDefinitionWire {
    fn from(def: &LabelDefinition) -> Self {
        Self {
            name: def.name.clone(),
            uses: def.uses,
            color: def.color.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelStateWire {
    pub name: String,
    pub cell: bool,
    pub track: &'static str,
}

impl From<&LabelState> for LabelStateWire {
    fn from(state: &LabelState) -> Self {
        Self {
            name: state.name.clone(),
            cell: state.cell,
            track: match state.track {
                TrackCoverage::All => "all",
                TrackCoverage::None => "none",
                TrackCoverage::Some => "some",
            },
        }
    }
}

/// `DELETE /project/label-definitions/{name}`: the strip edit when any cell carried the
/// name, and the list afterwards.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelDefinitionsWire {
    pub session_id: String,
    pub definitions: Vec<LabelDefinitionWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditResultWire>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellRowWire {
    pub id: u32,
    pub t: u64,
    /// `[z, y, x]` in pixel units.
    pub centroid: Option<[f64; 3]>,
    pub area: Option<u64>,
    pub confidence: Option<f64>,
    pub state: Option<&'static str>,
    pub track_id: Option<u32>,
    pub parent_id: Option<u32>,
    pub reviewed: bool,
    pub labels: Vec<String>,
    pub track_labels: Vec<String>,
}

impl From<&CellRow> for CellRowWire {
    fn from(row: &CellRow) -> Self {
        Self {
            id: row.id,
            t: row.t,
            centroid: row.centroid,
            area: row.area,
            confidence: row.detection_confidence,
            state: row.state.map(|s| s.as_str()),
            track_id: row.track_id,
            parent_id: row.parent_id,
            reviewed: row.reviewed,
            labels: row.labels.clone(),
            track_labels: row.track_labels.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkRowWire {
    pub parent: u32,
    pub child: u32,
    pub confidence: Option<f64>,
    pub reviewed: bool,
}

impl From<&LinkRow> for LinkRowWire {
    fn from(row: &LinkRow) -> Self {
        Self {
            parent: row.parent,
            child: row.child,
            confidence: row.confidence,
            reviewed: row.reviewed,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineageTreeWire {
    pub root_cell_id: u32,
    pub focus_cell_id: u32,
    pub cells: Vec<CellRowWire>,
    pub links: Vec<LinkRowWire>,
}

impl From<&LineageTree> for LineageTreeWire {
    fn from(tree: &LineageTree) -> Self {
        Self {
            root_cell_id: tree.root,
            focus_cell_id: tree.focus,
            cells: tree.cells.iter().map(CellRowWire::from).collect(),
            links: tree.links.iter().map(LinkRowWire::from).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistogramWire {
    pub counts: Vec<u32>,
    pub min: u64,
    pub max: u64,
    /// True when the bins come from a coarse level or a strided sample rather than
    /// every full-resolution voxel.
    pub sampled: bool,
    /// Additive: the pyramid level the sample came from, and how many voxels it covered.
    pub level: u32,
    pub samples: u64,
}

impl HistogramWire {
    /// `level_voxels` is the ZYX voxel count of the level the sample came from.
    pub fn new(histogram: &Histogram, level_voxels: u64) -> Self {
        Self {
            counts: histogram.bins.clone(),
            min: histogram.min,
            max: histogram.max,
            sampled: histogram.level > 0 || histogram.samples < level_voxels,
            level: histogram.level,
            samples: histogram.samples,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEntryWire {
    pub seq: i64,
    pub ts: String,
    pub domain: &'static str,
    pub scope: Option<String>,
    pub undone: bool,
    /// False once the entry's chunk snapshots have been pruned past the retained window.
    pub undoable: bool,
}

impl From<&EditEntry> for EditEntryWire {
    fn from(entry: &EditEntry) -> Self {
        Self {
            seq: entry.seq,
            ts: entry.ts.clone(),
            domain: entry.domain.as_str(),
            scope: entry.scope.clone(),
            undone: entry.undone,
            undoable: entry.undoable,
        }
    }
}

/// A block of label ids a session may paint with.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelLeaseWire {
    pub first: u32,
    pub count: u32,
}

/// The discriminated result of any mutation through the coordinator. routes and
/// the api-client dispatch on `domain`.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum EditResultWire {
    Mask(MaskEditWire),
    Graph(GraphEditWire),
}

impl EditResultWire {
    pub fn new(session: &str, outcome: EditOutcome) -> Self {
        match outcome {
            EditOutcome::Mask(commit) => Self::Mask(MaskEditWire::new(session, commit)),
            EditOutcome::Graph(commit) => Self::Graph(GraphEditWire::new(session, commit)),
        }
    }
}

/// What one committed mask edit tells the renderer: the version to advance to, the cells
/// whose voxels changed, the ids that no longer exist, and the chunks to drop.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskEditWire {
    pub domain: &'static str,
    pub seq: i64,
    pub version: u64,
    pub session_id: String,
    /// The store exists from here on, so the renderer flips its own flag without re-opening
    /// the project.
    pub has_labels: bool,
    pub cells: Vec<CellRowWire>,
    pub removed: Vec<u32>,
    pub chunks: Vec<String>,
    /// `version.graph` after the commit, when the mask edit removed cells or links.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_tracks: Option<Vec<u32>>,
}

impl MaskEditWire {
    pub fn new(session: &str, commit: MaskCommit) -> Self {
        Self {
            domain: "mask",
            seq: commit.seq,
            version: commit.version,
            session_id: session.to_owned(),
            has_labels: commit.has_labels,
            cells: commit.cells.iter().map(CellRowWire::from).collect(),
            removed: commit.removed,
            chunks: commit.chunks,
            graph_version: commit.graph_version,
            affected_tracks: commit.affected_tracks,
        }
    }
}

/// What one committed graph edit tells the renderer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEditWire {
    pub domain: &'static str,
    pub session_id: String,
    pub seq: i64,
    pub graph_version: u64,
    /// Rows of every cell whose track assignment the edit touched, as committed.
    pub affected_cells: Vec<CellRowWire>,
    pub affected_tracks: Vec<u32>,
}

impl GraphEditWire {
    pub fn new(session: &str, commit: GraphCommit) -> Self {
        Self {
            domain: "graph",
            session_id: session.to_owned(),
            seq: commit.seq,
            graph_version: commit.graph_version,
            affected_cells: commit.cells.iter().map(CellRowWire::from).collect(),
            affected_tracks: commit.affected_tracks,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PixelValue {
    pub value: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRef {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub ticket: String,
}
