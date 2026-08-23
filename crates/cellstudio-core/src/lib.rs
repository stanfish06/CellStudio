pub mod axes;
pub mod bricks;
pub mod dataset;
pub mod labels;
pub mod reader;
pub mod rechunk;
pub mod tracks;
pub mod volume;

pub use axes::{Axis, AxisMap, Dims, Dtype, Orientation, PhysicalScale};
pub use bricks::{Brick, BrickCache, BrickKey, BrickStats};
pub use dataset::{
    ChannelMeta, Dataset, LayoutReport, Level, OpenError, ViewAmplification, ZarrFormat,
    analyze_layout, open,
};
pub use labels::{
    ChunkKey, ChunkSnapshot, ContractError, EditFootprint, ExtentRow, LabelDelta, LabelError,
    LabelStore, StrokeMode, StrokeSpec, VoxelBox, VoxelRun, VoxelSet, apply, check_contract,
    clear_label, downsample, ensure_store, regenerate_coarse, restore, scan_label, snapshot,
    stamp_voxels,
};
pub use reader::{Histogram, ImageReader, OrthoAxis, Plane, ReadError, Volume};
pub use rechunk::{RechunkError, rechunk};
pub use tracks::{CellRecord, CellState, LinkRef};
pub use volume::{ProxyError, ProxyStore, build_proxy, choose_proxy_level};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerId {
    Image,  // image
    Labels, // segmentation
}
