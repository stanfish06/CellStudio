//! The project's label store: one u32 array per image pyramid level, the level-0
//! rasterization contract every consumer derives from, and chunk-granular writes with
//! byte-exact snapshots behind them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use zarrs::array::codec::ZstdCodec;
use zarrs::array::{Array, ArrayBuilder, ArraySubset, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::GroupBuilder;
use zarrs::storage::{ReadableStorageTraits, StoreKey, WritableStorageTraits};

use crate::LayerId;
use crate::axes::{Axis, Dims, Dtype, PhysicalScale};
use crate::bricks::{BrickCache, BrickKey};
use crate::dataset::{self, Dataset, OpenError};

/// Chunk shape of every label level: 256 KB decoded at u32, so a stroke rewrites
/// kilobytes rather than a whole ZYX block.
pub const LABEL_CHUNK_Z: u64 = 4;
pub const LABEL_CHUNK_XY: u64 = 128;

/// viv hands the fragment shader a float, so ids stay distinguishable only this far.
pub const MAX_LABEL_ID: u64 = (1 << 24) - 1;

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("label store has sample type {found:?}, but the contract is u32")]
    Dtype { found: Dtype },
    #[error("label store has {found} level(s), the image pyramid has {expected}")]
    LevelCount { found: usize, expected: usize },
    #[error(
        "label level {level} is t={found:?} but must mirror the image level at t={expected:?} with c=1"
    )]
    LevelDims {
        level: u32,
        found: Dims,
        expected: Dims,
    },
    #[error("label level {level} axes are not named t, c, z, y, x in that order")]
    AxisOrder { level: u32 },
    #[error("label id {found} exceeds the {MAX_LABEL_ID} the overlay can distinguish")]
    LabelIdTooLarge { found: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum LabelError {
    #[error(transparent)]
    Dataset(#[from] OpenError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("zarr error on the label store: {0}")]
    Zarr(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("label level {level} does not exist (store has {levels} level(s))")]
    NoSuchLevel { level: u32, levels: u32 },
    #[error("{axis:?} index {index} is out of bounds (extent {extent})")]
    OutOfBounds { axis: Axis, index: u64, extent: u64 },
}

/// One chunk of one label level at one timepoint. `c` is always 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChunkKey {
    pub level: u32,
    pub t: u64,
    /// Chunk indices, `[z, y, x]`.
    pub grid: [u64; 3],
}

impl ChunkKey {
    pub fn brick(&self, layer: LayerId) -> BrickKey {
        BrickKey {
            layer,
            level: self.level,
            t: self.t,
            c: 0,
            grid: self.grid,
        }
    }
}

/// The prior object state of one chunk. `existed = false` is the normal case for the
/// first paint in a region, and its inverse is an erase rather than a zero chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSnapshot {
    pub key: ChunkKey,
    pub existed: bool,
    pub bytes: Option<Vec<u8>>,
}

/// Inclusive level-0 voxel box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoxelBox {
    pub z0: u64,
    pub z1: u64,
    pub y0: u64,
    pub y1: u64,
    pub x0: u64,
    pub x1: u64,
}

impl VoxelBox {
    pub fn point(zyx: [u64; 3]) -> Self {
        Self {
            z0: zyx[0],
            z1: zyx[0],
            y0: zyx[1],
            y1: zyx[1],
            x0: zyx[2],
            x1: zyx[2],
        }
    }

    pub fn grow(&mut self, zyx: [u64; 3]) {
        self.z0 = self.z0.min(zyx[0]);
        self.z1 = self.z1.max(zyx[0]);
        self.y0 = self.y0.min(zyx[1]);
        self.y1 = self.y1.max(zyx[1]);
        self.x0 = self.x0.min(zyx[2]);
        self.x1 = self.x1.max(zyx[2]);
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            z0: self.z0.min(other.z0),
            z1: self.z1.max(other.z1),
            y0: self.y0.min(other.y0),
            y1: self.y1.max(other.y1),
            x0: self.x0.min(other.x0),
            x1: self.x1.max(other.x1),
        }
    }
}

/// A contiguous x span at one `(z, y)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VoxelRun {
    pub z: u64,
    pub y: u64,
    /// Inclusive.
    pub x0: u64,
    pub x1: u64,
}

/// A level-0 voxel set, held as x runs sorted by `(z, y, x0)` and never overlapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoxelSet {
    runs: Vec<VoxelRun>,
}

impl VoxelSet {
    /// Sorts and merges, so a set built from overlapping stamps holds each voxel once.
    pub fn from_runs(mut runs: Vec<VoxelRun>) -> Self {
        runs.retain(|r| r.x0 <= r.x1);
        runs.sort_unstable();
        let mut merged: Vec<VoxelRun> = Vec::with_capacity(runs.len());
        for run in runs {
            match merged.last_mut() {
                Some(last)
                    if last.z == run.z
                        && last.y == run.y
                        && run.x0 <= last.x1.saturating_add(1) =>
                {
                    last.x1 = last.x1.max(run.x1);
                }
                _ => merged.push(run),
            }
        }
        Self { runs: merged }
    }

    pub fn from_box(b: VoxelBox) -> Self {
        let runs = (b.z0..=b.z1)
            .flat_map(|z| {
                (b.y0..=b.y1).map(move |y| VoxelRun {
                    z,
                    y,
                    x0: b.x0,
                    x1: b.x1,
                })
            })
            .collect();
        Self::from_runs(runs)
    }

    pub fn runs(&self) -> &[VoxelRun] {
        &self.runs
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Voxel count, not run count.
    pub fn len(&self) -> u64 {
        self.runs.iter().map(|r| r.x1 - r.x0 + 1).sum()
    }

    /// Sorted by `(z, y, x)`.
    pub fn iter(&self) -> impl Iterator<Item = [u64; 3]> + '_ {
        self.runs
            .iter()
            .flat_map(|r| (r.x0..=r.x1).map(move |x| [r.z, r.y, x]))
    }

    pub fn bounds(&self) -> Option<VoxelBox> {
        let first = self.runs.first()?;
        let mut b = VoxelBox {
            z0: first.z,
            z1: first.z,
            y0: first.y,
            y1: first.y,
            x0: first.x0,
            x1: first.x1,
        };
        for r in &self.runs {
            b.grow([r.z, r.y, r.x0]);
            b.grow([r.z, r.y, r.x1]);
        }
        Some(b)
    }

    pub fn union(sets: impl IntoIterator<Item = VoxelSet>) -> Self {
        Self::from_runs(sets.into_iter().flat_map(|s| s.runs).collect())
    }
}

/// Per-axis voxel radii `[rz, ry, rx]` for a radius stated in level-0 x pixels.
fn radii(r: f64, scale: Option<PhysicalScale>) -> [f64; 3] {
    let s = match scale {
        Some(s)
            if s.z > 0.0
                && s.y > 0.0
                && s.x > 0.0
                && s.z.is_finite()
                && s.y.is_finite()
                && s.x.is_finite() =>
        {
            s
        }
        _ => PhysicalScale::ISOTROPIC,
    };
    [r * s.x / s.z, r * s.x / s.y, r]
}

/// The level-0 rasterization contract: `centre` in fractional level-0 voxel coordinates
/// `[z, y, x]`, membership by voxel centre (voxel `i` spans `[i, i+1)`), inclusive
/// bounds, clipped to `dims`, and `plane` pinning one axis to one exact index so the 2D
/// disk is the ellipsoid intersected with that slice.
pub fn stamp_voxels(
    centre: [f64; 3],
    r: f64,
    scale: Option<PhysicalScale>,
    plane: Option<(Axis, u64)>,
    dims: [u64; 3],
) -> VoxelSet {
    if !r.is_finite() || r <= 0.0 || dims.contains(&0) {
        return VoxelSet::default();
    }
    let rad = radii(r, scale);
    let mut lo = [0_u64; 3];
    let mut hi = [0_u64; 3];
    for i in 0..3 {
        let a = (centre[i] - rad[i] - 0.5).floor();
        let b = (centre[i] + rad[i]).floor();
        if b < 0.0 || a >= dims[i] as f64 {
            return VoxelSet::default();
        }
        lo[i] = a.max(0.0) as u64;
        hi[i] = b.min((dims[i] - 1) as f64) as u64;
    }
    if let Some((axis, index)) = plane {
        let slot = match axis {
            Axis::Z => 0,
            Axis::Y => 1,
            Axis::X => 2,
            _ => return VoxelSet::default(),
        };
        if index < lo[slot] || index > hi[slot] {
            return VoxelSet::default();
        }
        lo[slot] = index;
        hi[slot] = index;
    }

    let mut runs = Vec::new();
    for z in lo[0]..=hi[0] {
        let dz = (z as f64 + 0.5 - centre[0]) / rad[0];
        for y in lo[1]..=hi[1] {
            let dy = (y as f64 + 0.5 - centre[1]) / rad[1];
            let mut open: Option<VoxelRun> = None;
            for x in lo[2]..=hi[2] {
                let dx = (x as f64 + 0.5 - centre[2]) / rad[2];
                if dz * dz + dy * dy + dx * dx <= 1.0 {
                    match open.as_mut() {
                        Some(run) if run.x1 + 1 == x => run.x1 = x,
                        _ => {
                            if let Some(run) = open.take() {
                                runs.push(run);
                            }
                            open = Some(VoxelRun { z, y, x0: x, x1: x });
                        }
                    }
                } else if let Some(run) = open.take() {
                    runs.push(run);
                }
            }
            if let Some(run) = open.take() {
                runs.push(run);
            }
        }
    }
    VoxelSet::from_runs(runs)
}

/// The only way a coarse voxel set is produced: a coarse voxel belongs to the set when
/// the level-0 voxel it point-samples does. Nothing re-rasterizes an ellipsoid at another
/// level, so the echo matches the store at every level.
pub fn downsample(set: &VoxelSet, factor: [u64; 3]) -> VoxelSet {
    let f = [factor[0].max(1), factor[1].max(1), factor[2].max(1)];
    let mut runs = Vec::new();
    for run in &set.runs {
        if !run.z.is_multiple_of(f[0]) || !run.y.is_multiple_of(f[1]) {
            continue;
        }
        let x0 = run.x0.div_ceil(f[2]);
        let x1 = run.x1 / f[2];
        if x0 > x1 {
            continue;
        }
        runs.push(VoxelRun {
            z: run.z / f[0],
            y: run.y / f[1],
            x0,
            x1,
        });
    }
    VoxelSet::from_runs(runs)
}

/// Paint writes `label`; erase writes 0, scoped to `only` when the eraser is following a
/// selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeMode {
    Paint { label: u32 },
    Erase { only: Option<u32> },
}

/// One stroke as it arrives from the client: stamp centres in fractional level-0 voxels.
#[derive(Debug, Clone)]
pub struct StrokeSpec {
    pub mode: StrokeMode,
    pub radius: f64,
    /// `Some` for a slice-view disk, `None` for a 3D orb.
    pub plane: Option<(Axis, u64)>,
    pub centres: Vec<[f64; 3]>,
}

impl StrokeSpec {
    pub fn rasterize(&self, scale: Option<PhysicalScale>, dims: [u64; 3]) -> VoxelSet {
        VoxelSet::union(
            self.centres
                .iter()
                .map(|c| stamp_voxels(*c, self.radius, scale, self.plane, dims)),
        )
    }
}

/// Voxel-count and centroid-sum change for one label, the exact input `mask_extent`
/// folds in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelDelta {
    pub label: u32,
    pub area: i64,
    pub sum_z: f64,
    pub sum_y: f64,
    pub sum_x: f64,
}

/// What one write touched.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditFootprint {
    pub chunks: Vec<ChunkKey>,
    /// Level-0 box of the voxels whose value actually changed.
    pub bbox: Option<VoxelBox>,
    /// Sorted by label; background (0) is not a cell and is not reported.
    pub deltas: Vec<LabelDelta>,
}

/// Exact per-`(t, label)` statistics, for a label this session did not paint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtentRow {
    pub t: u64,
    pub label: u32,
    pub bbox: Option<VoxelBox>,
    pub area: u64,
    pub sum_z: f64,
    pub sum_y: f64,
    pub sum_x: f64,
}

struct LabelLevel {
    index: u32,
    dims: Dims,
    chunks: Dims,
    /// Level-0 voxels per voxel of this level, `[z, y, x]`.
    factor: [u64; 3],
    array: Array<FilesystemStore>,
}

impl LabelLevel {
    fn grid(&self) -> [u64; 3] {
        BrickCache::grid_shape(self.dims, self.chunks)
    }

    fn origin(&self, grid: [u64; 3]) -> [u64; 3] {
        [
            grid[0] * self.chunks.z,
            grid[1] * self.chunks.y,
            grid[2] * self.chunks.x,
        ]
    }

    /// Edge chunks are clipped to the level, which is the shape zarrs encodes.
    fn shape(&self, grid: [u64; 3]) -> [u64; 3] {
        let o = self.origin(grid);
        [
            self.chunks.z.min(self.dims.z.saturating_sub(o[0])),
            self.chunks.y.min(self.dims.y.saturating_sub(o[1])),
            self.chunks.x.min(self.dims.x.saturating_sub(o[2])),
        ]
    }

    fn indices(&self, t: u64, grid: [u64; 3]) -> [u64; 5] {
        [t, 0, grid[0], grid[1], grid[2]]
    }

    fn read_chunk(&self, t: u64, grid: [u64; 3]) -> Result<Vec<u32>, LabelError> {
        self.array
            .retrieve_chunk::<Vec<u32>>(&self.indices(t, grid))
            .map_err(|e| LabelError::Zarr(e.to_string()))
    }

    fn write_chunk(&self, t: u64, grid: [u64; 3], values: Vec<u32>) -> Result<(), LabelError> {
        self.array
            .store_chunk(&self.indices(t, grid), values)
            .map_err(|e| LabelError::Zarr(e.to_string()))
    }

    fn chunk_key(&self, t: u64, grid: [u64; 3]) -> StoreKey {
        self.array.chunk_key(&self.indices(t, grid))
    }
}

/// Writable handles on the label store. The reader gets its own readable [`Dataset`]
/// over the same directory: `dataset::open` yields `ReadableStorage`, on which
/// `store_chunk` is not callable.
pub struct LabelStore {
    root: PathBuf,
    store: Arc<FilesystemStore>,
    scale: Option<PhysicalScale>,
    levels: Vec<LabelLevel>,
}

impl std::fmt::Debug for LabelStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelStore")
            .field("root", &self.root)
            .field("levels", &self.levels.len())
            .finish()
    }
}

impl LabelStore {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn scale(&self) -> Option<PhysicalScale> {
        self.scale
    }

    pub fn level_count(&self) -> u32 {
        self.levels.len() as u32
    }

    pub fn dims(&self, level: u32) -> Result<Dims, LabelError> {
        Ok(self.level(level)?.dims)
    }

    pub fn chunks(&self, level: u32) -> Result<Dims, LabelError> {
        Ok(self.level(level)?.chunks)
    }

    /// Level-0 voxels per voxel of `level`, `[z, y, x]`.
    pub fn factor(&self, level: u32) -> Result<[u64; 3], LabelError> {
        Ok(self.level(level)?.factor)
    }

    /// A readable handle over the same directory, for `ImageReader::register_layer`.
    pub fn open_readable(&self) -> Result<Dataset, OpenError> {
        dataset::open(&self.root)
    }

    fn level(&self, level: u32) -> Result<&LabelLevel, LabelError> {
        self.levels
            .get(level as usize)
            .ok_or(LabelError::NoSuchLevel {
                level,
                levels: self.levels.len() as u32,
            })
    }

    fn check_t(&self, t: u64) -> Result<(), LabelError> {
        let extent = self.level(0)?.dims.t;
        if t >= extent {
            return Err(LabelError::OutOfBounds {
                axis: Axis::T,
                index: t,
                extent,
            });
        }
        Ok(())
    }
}

/// Adopt the store already in the project, or create an empty one that matches `image`.
/// Creation writes into a temporary sibling directory and renames, so an interrupted
/// create leaves no store rather than one that fails [`check_contract`].
pub fn ensure_store(root: &Path, image: &Dataset) -> Result<LabelStore, LabelError> {
    if !root.exists() {
        create_store(root, image)?;
    }
    open_store(root, image)
}

/// Open an existing store, refusing one that does not satisfy the contract.
pub fn open_store(root: &Path, image: &Dataset) -> Result<LabelStore, LabelError> {
    let readable = dataset::open(root)?;
    check_contract(&readable, image)?;
    let store = Arc::new(FilesystemStore::new(root).map_err(|e| LabelError::Zarr(e.to_string()))?);
    let level0 = readable
        .levels
        .first()
        .map(|l| l.dims)
        .unwrap_or(image.dims);
    let mut levels = Vec::with_capacity(readable.levels.len());
    for level in &readable.levels {
        let array = Array::open(
            store.clone(),
            &format!("/{}", level.path.trim_start_matches('/')),
        )
        .map_err(|e| LabelError::Zarr(e.to_string()))?;
        levels.push(LabelLevel {
            index: level.index,
            dims: level.dims,
            chunks: level.chunks,
            factor: [
                ratio(level0.z, level.dims.z),
                ratio(level0.y, level.dims.y),
                ratio(level0.x, level.dims.x),
            ],
            array,
        });
    }
    Ok(LabelStore {
        root: root.to_path_buf(),
        store,
        scale: image.scale,
        levels,
    })
}

fn ratio(base: u64, level: u64) -> u64 {
    ((base as f64 / level.max(1) as f64).round() as u64).max(1)
}

/// u32 values, one array per image level mirroring that level's z/y/x and t with `c = 1`,
/// and TCZYX dimension names. Chunking is deliberately not checked: an adopted store
/// chunked another way is editable, only slower.
pub fn check_contract(store: &Dataset, image: &Dataset) -> Result<(), ContractError> {
    if store.dtype != Dtype::U32 {
        return Err(ContractError::Dtype { found: store.dtype });
    }
    if store.levels.len() != image.levels.len() {
        return Err(ContractError::LevelCount {
            found: store.levels.len(),
            expected: image.levels.len(),
        });
    }
    for (level, image_level) in store.levels.iter().zip(&image.levels) {
        let expected = Dims {
            c: 1,
            ..image_level.dims
        };
        if level.dims != expected {
            return Err(ContractError::LevelDims {
                level: level.index,
                found: level.dims,
                expected,
            });
        }
        let map = store
            .source(level.index)
            .map_err(|_| ContractError::AxisOrder { level: level.index })?
            .map;
        let ordered = map.ndim() == 5 && Axis::ALL.iter().all(|a| map.slot(*a) == Some(a.slot()));
        if !ordered {
            return Err(ContractError::AxisOrder { level: level.index });
        }
    }
    Ok(())
}

/// The database's largest label, against the id the overlay can still distinguish.
pub fn check_max_label(max: u64) -> Result<(), ContractError> {
    if max > MAX_LABEL_ID {
        return Err(ContractError::LabelIdTooLarge { found: max });
    }
    Ok(())
}

fn create_store(root: &Path, image: &Dataset) -> Result<(), LabelError> {
    let parent = root.parent().unwrap_or(Path::new("."));
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "labels.zarr".into());
    let tmp = parent.join(format!(".{name}.creating"));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| LabelError::Io {
            path: tmp.clone(),
            source: e,
        })?;
    }
    std::fs::create_dir_all(&tmp).map_err(|e| LabelError::Io {
        path: tmp.clone(),
        source: e,
    })?;

    let result = write_empty_store(&tmp, image);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        return result;
    }
    std::fs::rename(&tmp, root).map_err(|e| LabelError::Io {
        path: root.to_path_buf(),
        source: e,
    })
}

fn write_empty_store(root: &Path, image: &Dataset) -> Result<(), LabelError> {
    let store = Arc::new(FilesystemStore::new(root).map_err(|e| LabelError::Zarr(e.to_string()))?);
    let mut group = GroupBuilder::new()
        .build(store.clone(), "/")
        .map_err(|e| LabelError::Zarr(e.to_string()))?;
    *group.attributes_mut() = label_attributes(image);
    group
        .store_metadata()
        .map_err(|e| LabelError::Zarr(e.to_string()))?;

    for level in &image.levels {
        let dims = Dims { c: 1, ..level.dims };
        let chunks = label_chunks(dims);
        let mut builder = ArrayBuilder::new(
            vec![dims.t, dims.c, dims.z, dims.y, dims.x],
            vec![chunks.t, chunks.c, chunks.z, chunks.y, chunks.x],
            data_type::uint32(),
            0u32,
        );
        builder
            .bytes_to_bytes_codecs(vec![Arc::new(ZstdCodec::new(3, false))])
            .dimension_names(Some(["t", "c", "z", "y", "x"]));
        let array = builder
            .build(store.clone(), &format!("/{}", level.index))
            .map_err(|e| LabelError::Zarr(e.to_string()))?;
        array
            .store_metadata()
            .map_err(|e| LabelError::Zarr(e.to_string()))?;
    }
    Ok(())
}

/// `(t=1, c=1, z=min(4, Z), y=128, x=128)`, never exceeding the level it describes.
pub fn label_chunks(dims: Dims) -> Dims {
    Dims {
        t: 1,
        c: 1,
        z: LABEL_CHUNK_Z.clamp(1, dims.z.max(1)),
        y: LABEL_CHUNK_XY.clamp(1, dims.y.max(1)),
        x: LABEL_CHUNK_XY.clamp(1, dims.x.max(1)),
    }
}

fn label_attributes(image: &Dataset) -> Map<String, Value> {
    let base = image.scale.unwrap_or(PhysicalScale::ISOTROPIC);
    let datasets: Vec<Value> = image
        .levels
        .iter()
        .map(|level| {
            json!({
                "path": level.index.to_string(),
                "coordinateTransformations": [{
                    "type": "scale",
                    "scale": [
                        1.0,
                        1.0,
                        base.z * level.factor[0],
                        base.y * level.factor[1],
                        base.x * level.factor[2],
                    ],
                }],
            })
        })
        .collect();
    let mut attributes = Map::new();
    attributes.insert(
        "ome".into(),
        json!({
            "version": "0.5",
            "multiscales": [{
                "name": "cellstudio-labels",
                "axes": [
                    { "name": "t", "type": "time", "unit": "second" },
                    { "name": "c", "type": "channel" },
                    { "name": "z", "type": "space", "unit": "micrometer" },
                    { "name": "y", "type": "space", "unit": "micrometer" },
                    { "name": "x", "type": "space", "unit": "micrometer" },
                ],
                "datasets": datasets,
            }],
        }),
    );
    attributes.insert("cellstudio_labels".into(), json!(image.root));
    attributes
}

/// Encoded bytes of each chunk as it stands, or the record that it has no object. zarrs
/// offers no `retrieve_encoded_chunk`, so this is a raw store-key read.
pub fn snapshot(store: &LabelStore, chunks: &[ChunkKey]) -> Result<Vec<ChunkSnapshot>, LabelError> {
    chunks
        .iter()
        .map(|key| {
            let level = store.level(key.level)?;
            let raw = store
                .store
                .get(&level.chunk_key(key.t, key.grid))
                .map_err(|e| LabelError::Zarr(e.to_string()))?;
            Ok(ChunkSnapshot {
                key: *key,
                existed: raw.is_some(),
                bytes: raw.map(|b| b.to_vec()),
            })
        })
        .collect()
}

/// The inverse of a snapshot. An absent object is erased, not written back as an encoded
/// zero chunk: the two are value-equivalent but the second leaves an object the store
/// never had.
pub fn restore(store: &LabelStore, snaps: &[ChunkSnapshot]) -> Result<(), LabelError> {
    for snap in snaps {
        let level = store.level(snap.key.level)?;
        let key = level.chunk_key(snap.key.t, snap.key.grid);
        match (snap.existed, &snap.bytes) {
            (true, Some(bytes)) => store
                .store
                .set(&key, Bytes::from(bytes.clone()))
                .map_err(|e| LabelError::Zarr(e.to_string()))?,
            _ => store
                .store
                .erase(&key)
                .map_err(|e| LabelError::Zarr(e.to_string()))?,
        }
    }
    Ok(())
}

/// Rasterize the stroke at level 0 and write it, chunk by chunk.
pub fn apply(store: &LabelStore, t: u64, spec: &StrokeSpec) -> Result<EditFootprint, LabelError> {
    store.check_t(t)?;
    let level = store.level(0)?;
    let dims = [level.dims.z, level.dims.y, level.dims.x];
    let set = spec.rasterize(store.scale, dims);
    write_set(store, t, &set, spec.mode)
}

/// Delete: the recorded bbox is the only region scanned.
pub fn clear_label(
    store: &LabelStore,
    t: u64,
    label: u32,
    bbox: VoxelBox,
) -> Result<EditFootprint, LabelError> {
    store.check_t(t)?;
    let level = store.level(0)?;
    let clipped = VoxelBox {
        z0: bbox.z0,
        z1: bbox.z1.min(level.dims.z.saturating_sub(1)),
        y0: bbox.y0,
        y1: bbox.y1.min(level.dims.y.saturating_sub(1)),
        x0: bbox.x0,
        x1: bbox.x1.min(level.dims.x.saturating_sub(1)),
    };
    if clipped.z0 > clipped.z1 || clipped.y0 > clipped.y1 || clipped.x0 > clipped.x1 {
        return Ok(EditFootprint::default());
    }
    write_set(
        store,
        t,
        &VoxelSet::from_box(clipped),
        StrokeMode::Erase { only: Some(label) },
    )
}

fn write_set(
    store: &LabelStore,
    t: u64,
    set: &VoxelSet,
    mode: StrokeMode,
) -> Result<EditFootprint, LabelError> {
    let level = store.level(0)?;
    let mut by_chunk: BTreeMap<[u64; 3], Vec<VoxelRun>> = BTreeMap::new();
    for run in set.runs() {
        let gz = run.z / level.chunks.z;
        let gy = run.y / level.chunks.y;
        let mut x = run.x0;
        while x <= run.x1 {
            let gx = x / level.chunks.x;
            let end = run.x1.min((gx + 1) * level.chunks.x - 1);
            by_chunk.entry([gz, gy, gx]).or_default().push(VoxelRun {
                z: run.z,
                y: run.y,
                x0: x,
                x1: end,
            });
            x = end + 1;
        }
    }

    let mut footprint = EditFootprint::default();
    let mut deltas: HashMap<u32, LabelDelta> = HashMap::new();
    for (grid, runs) in by_chunk {
        let origin = level.origin(grid);
        let shape = level.shape(grid);
        let mut values = level.read_chunk(t, grid)?;
        let mut changed = false;
        for run in runs {
            for x in run.x0..=run.x1 {
                let local = [run.z - origin[0], run.y - origin[1], x - origin[2]];
                let index = ((local[0] * shape[1] + local[1]) * shape[2] + local[2]) as usize;
                let Some(slot) = values.get_mut(index) else {
                    continue;
                };
                let old = *slot;
                let new = match mode {
                    StrokeMode::Paint { label } => label,
                    StrokeMode::Erase { only: None } => 0,
                    StrokeMode::Erase { only: Some(l) } if old == l => 0,
                    StrokeMode::Erase { only: Some(_) } => old,
                };
                if new == old {
                    continue;
                }
                *slot = new;
                changed = true;
                let zyx = [run.z, run.y, x];
                accumulate(&mut deltas, old, zyx, -1);
                accumulate(&mut deltas, new, zyx, 1);
                footprint.bbox = Some(match footprint.bbox {
                    Some(mut b) => {
                        b.grow(zyx);
                        b
                    }
                    None => VoxelBox::point(zyx),
                });
            }
        }
        if changed {
            level.write_chunk(t, grid, values)?;
            footprint.chunks.push(ChunkKey { level: 0, t, grid });
        }
    }
    let mut deltas: Vec<LabelDelta> = deltas
        .into_values()
        .filter(|d| d.area != 0 || d.sum_z != 0.0)
        .collect();
    deltas.sort_unstable_by_key(|d| d.label);
    footprint.deltas = deltas;
    Ok(footprint)
}

fn accumulate(deltas: &mut HashMap<u32, LabelDelta>, label: u32, zyx: [u64; 3], sign: i64) {
    if label == 0 {
        return;
    }
    let entry = deltas.entry(label).or_insert(LabelDelta {
        label,
        area: 0,
        sum_z: 0.0,
        sum_y: 0.0,
        sum_x: 0.0,
    });
    let s = sign as f64;
    entry.area += sign;
    entry.sum_z += s * zyx[0] as f64;
    entry.sum_y += s * zyx[1] as f64;
    entry.sum_x += s * zyx[2] as f64;
}

/// Point-sample level 0 into every coarser level's covering chunks. Never averaging: the
/// mean of two label ids is a third cell.
pub fn regenerate_coarse(
    store: &LabelStore,
    t: u64,
    bbox: VoxelBox,
) -> Result<Vec<ChunkKey>, LabelError> {
    store.check_t(t)?;
    let level0 = store.level(0)?;
    let mut changed = Vec::new();
    for level in store.levels.iter().skip(1) {
        let f = level.factor;
        let lo = [
            bbox.z0.div_ceil(f[0]),
            bbox.y0.div_ceil(f[1]),
            bbox.x0.div_ceil(f[2]),
        ];
        let hi = [
            (bbox.z1 / f[0]).min(level.dims.z.saturating_sub(1)),
            (bbox.y1 / f[1]).min(level.dims.y.saturating_sub(1)),
            (bbox.x1 / f[2]).min(level.dims.x.saturating_sub(1)),
        ];
        if (0..3).any(|i| lo[i] > hi[i]) {
            continue;
        }
        let source = sample_region(level0, t, lo, hi, f)?;
        let extent = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];

        let grid_lo = [
            lo[0] / level.chunks.z,
            lo[1] / level.chunks.y,
            lo[2] / level.chunks.x,
        ];
        let grid_hi = [
            hi[0] / level.chunks.z,
            hi[1] / level.chunks.y,
            hi[2] / level.chunks.x,
        ];
        for gz in grid_lo[0]..=grid_hi[0] {
            for gy in grid_lo[1]..=grid_hi[1] {
                for gx in grid_lo[2]..=grid_hi[2] {
                    let grid = [gz, gy, gx];
                    let origin = level.origin(grid);
                    let shape = level.shape(grid);
                    let mut values = level.read_chunk(t, grid)?;
                    let mut dirty = false;
                    for cz in lo[0].max(origin[0])..=hi[0].min(origin[0] + shape[0] - 1) {
                        for cy in lo[1].max(origin[1])..=hi[1].min(origin[1] + shape[1] - 1) {
                            for cx in lo[2].max(origin[2])..=hi[2].min(origin[2] + shape[2] - 1) {
                                let src = (((cz - lo[0]) * extent[1] + (cy - lo[1])) * extent[2]
                                    + (cx - lo[2]))
                                    as usize;
                                let dst = (((cz - origin[0]) * shape[1] + (cy - origin[1]))
                                    * shape[2]
                                    + (cx - origin[2]))
                                    as usize;
                                let (Some(value), Some(slot)) =
                                    (source.get(src).copied(), values.get_mut(dst))
                                else {
                                    continue;
                                };
                                if *slot != value {
                                    *slot = value;
                                    dirty = true;
                                }
                            }
                        }
                    }
                    if dirty {
                        level.write_chunk(t, grid, values)?;
                        changed.push(ChunkKey {
                            level: level.index,
                            t,
                            grid,
                        });
                    }
                }
            }
        }
    }
    Ok(changed)
}

/// Level-0 values at the sample points of the coarse box `lo..=hi`, row-major over the
/// coarse extent.
fn sample_region(
    level0: &LabelLevel,
    t: u64,
    lo: [u64; 3],
    hi: [u64; 3],
    f: [u64; 3],
) -> Result<Vec<u32>, LabelError> {
    let start = [lo[0] * f[0], lo[1] * f[1], lo[2] * f[2]];
    let end = [
        (hi[0] * f[0] + 1).min(level0.dims.z),
        (hi[1] * f[1] + 1).min(level0.dims.y),
        (hi[2] * f[2] + 1).min(level0.dims.x),
    ];
    let span = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let subset = ArraySubset::new_with_ranges(&[
        t..t + 1,
        0..1,
        start[0]..end[0],
        start[1]..end[1],
        start[2]..end[2],
    ]);
    let dense = level0
        .array
        .retrieve_array_subset::<Vec<u32>>(&subset)
        .map_err(|e| LabelError::Zarr(e.to_string()))?;
    let extent = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    let mut out = vec![0_u32; (extent[0] * extent[1] * extent[2]) as usize];
    for cz in 0..extent[0] {
        for cy in 0..extent[1] {
            for cx in 0..extent[2] {
                let src = ((cz * f[0] * span[1] + cy * f[1]) * span[2] + cx * f[2]) as usize;
                let dst = ((cz * extent[1] + cy) * extent[2] + cx) as usize;
                if let (Some(value), Some(slot)) = (dense.get(src).copied(), out.get_mut(dst)) {
                    *slot = value;
                }
            }
        }
    }
    Ok(out)
}

/// Per-chunk visitor: `(origin, shape, values)`.
type ChunkVisitor<'a> = dyn FnMut([u64; 3], [u64; 3], &[u32]) + 'a;

/// One full chunk walk of `level` at frame `t`: `f(origin, shape, values)` once per chunk,
/// in grid order. The one raster loop [`scan_label`] and [`scan_inventory`] share.
fn for_each_frame_chunk(
    level: &LabelLevel,
    t: u64,
    f: &mut ChunkVisitor<'_>,
) -> Result<(), LabelError> {
    let grid = level.grid();
    for gz in 0..grid[0] {
        for gy in 0..grid[1] {
            for gx in 0..grid[2] {
                let cell = [gz, gy, gx];
                let values = level.read_chunk(t, cell)?;
                f(level.origin(cell), level.shape(cell), &values);
            }
        }
    }
    Ok(())
}

fn fold_voxel(row: &mut ExtentRow, zyx: [u64; 3]) {
    row.area += 1;
    row.sum_z += zyx[0] as f64;
    row.sum_y += zyx[1] as f64;
    row.sum_x += zyx[2] as f64;
    row.bbox = Some(match row.bbox {
        Some(mut b) => {
            b.grow(zyx);
            b
        }
        None => VoxelBox::point(zyx),
    });
}

/// One bounded frame scan producing the exact bbox, area and centroid sums for a label
/// this session did not paint — what `ensure_extent` calls once per `(t, label)`.
pub fn scan_label(store: &LabelStore, t: u64, label: u32) -> Result<ExtentRow, LabelError> {
    store.check_t(t)?;
    let level = store.level(0)?;
    let mut row = ExtentRow {
        t,
        label,
        bbox: None,
        area: 0,
        sum_z: 0.0,
        sum_y: 0.0,
        sum_x: 0.0,
    };
    if label == 0 {
        return Ok(row);
    }
    for_each_frame_chunk(level, t, &mut |origin, shape, values| {
        for (index, value) in values.iter().enumerate() {
            if *value != label {
                continue;
            }
            let i = index as u64;
            fold_voxel(
                &mut row,
                [
                    origin[0] + i / (shape[1] * shape[2]),
                    origin[1] + (i / shape[2]) % shape[1],
                    origin[2] + i % shape[2],
                ],
            );
        }
    })?;
    Ok(row)
}

/// Everything one full level-0 scan of an adopted store learns: one exact [`ExtentRow`] per
/// `(frame, label)` occurrence, the highest id present, and the violations found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    /// Sorted by `(t, label)`.
    pub rows: Vec<ExtentRow>,
    pub max_id: u32,
    /// Ids past [`MAX_LABEL_ID`], sorted.
    pub oversized: Vec<u32>,
    /// Ids present on more than one frame, sorted; the store contract is one id per frame.
    pub multi_frame: Vec<u32>,
}

/// One pass over every level-0 chunk, frame by frame. `progress` is called once per
/// finished frame with the fraction complete.
pub fn scan_inventory(store: &LabelStore, progress: &dyn Fn(f32)) -> Result<Inventory, LabelError> {
    let level = store.level(0)?;
    let frames = level.dims.t;
    let mut inventory = Inventory::default();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut oversized: BTreeSet<u32> = BTreeSet::new();
    let mut multi_frame: BTreeSet<u32> = BTreeSet::new();
    for t in 0..frames {
        let mut frame: HashMap<u32, ExtentRow> = HashMap::new();
        for_each_frame_chunk(level, t, &mut |origin, shape, values| {
            for (index, value) in values.iter().enumerate() {
                if *value == 0 {
                    continue;
                }
                let i = index as u64;
                let row = frame.entry(*value).or_insert(ExtentRow {
                    t,
                    label: *value,
                    bbox: None,
                    area: 0,
                    sum_z: 0.0,
                    sum_y: 0.0,
                    sum_x: 0.0,
                });
                fold_voxel(
                    row,
                    [
                        origin[0] + i / (shape[1] * shape[2]),
                        origin[1] + (i / shape[2]) % shape[1],
                        origin[2] + i % shape[2],
                    ],
                );
            }
        })?;
        let mut rows: Vec<ExtentRow> = frame.into_values().collect();
        rows.sort_unstable_by_key(|row| row.label);
        for row in rows {
            inventory.max_id = inventory.max_id.max(row.label);
            if u64::from(row.label) > MAX_LABEL_ID {
                oversized.insert(row.label);
            }
            if !seen.insert(row.label) {
                multi_frame.insert(row.label);
            }
            inventory.rows.push(row);
        }
        progress((t + 1) as f32 / frames.max(1) as f32);
    }
    inventory.oversized = oversized.into_iter().collect();
    inventory.multi_frame = multi_frame.into_iter().collect();
    Ok(inventory)
}
