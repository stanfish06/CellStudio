use cellstudio_core::axes::{Dims, Dtype, Orientation, PhysicalScale};
use cellstudio_core::dataset::{ChannelMeta, LayoutReport, Level};
use cellstudio_core::reader::Histogram;
use cellstudio_db::queries::{CellRow, EditEntry, LineageTree, LinkRow, Versions};
use serde::Serialize;

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
    pub reviewed: bool,
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
            reviewed: row.reviewed,
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
}

impl From<&EditEntry> for EditEntryWire {
    fn from(entry: &EditEntry) -> Self {
        Self {
            seq: entry.seq,
            ts: entry.ts.clone(),
            domain: entry.domain.as_str(),
            scope: entry.scope.clone(),
            undone: entry.undone,
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
