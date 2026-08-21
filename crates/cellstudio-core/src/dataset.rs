use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use zarrs::array::{Array, DataType, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::Group;
use zarrs::storage::{ReadableStorage, ReadableStorageTraits};

use crate::axes::{Axis, AxisMap, Dims, Dtype, Orientation, PhysicalScale};

pub const MAX_XY_AMPLIFICATION: f64 = 4.0;

/// Chunk layers along the out-of-plane axis above which XZ/YZ assembly is hostile.
pub const MAX_ORTHO_COLUMN_CHUNKS: u64 = 8;

pub const DEFAULT_CHANNEL_COLORS: [&str; 6] =
    ["FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ZarrFormat {
    V2,
    V3,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Level {
    pub index: u32,
    /// Store-relative path of the level array (`"0"`, or `""` for a bare array store).
    pub path: String,
    pub dims: Dims,
    pub chunks: Dims,
    pub factor: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelMeta {
    pub index: u32,
    pub name: String,
    /// `RRGGBB`
    pub color: String,
    /// Display window `[start, end]`.
    pub window: [f64; 2],
    /// Slider bounds `[min, max]`.
    pub limits: [f64; 2],
    pub active: bool,
    pub defaulted: bool,
}

/// Read cost of one plane in one orientation, from chunk-shape arithmetic only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewAmplification {
    pub orientation: Orientation,
    /// Bytes of the plane itself.
    pub bytes_needed: u64,
    /// Bytes decoded to produce it (whole chunks).
    pub bytes_decoded: u64,
    pub chunks_decoded: u64,
    /// Chunk layers the brick column spans along the out-of-plane axis.
    pub column_chunks: u64,
    /// `bytes_decoded / bytes_needed`.
    pub amplification: f64,
    pub hostile: bool,
}

/// Per-orientation read amplification for one level.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutReport {
    pub level: u32,
    pub views: Vec<ViewAmplification>,
    pub hostile: bool,
    pub hostile_views: Vec<Orientation>,
}

impl LayoutReport {
    pub fn view(&self, orientation: Orientation) -> Option<&ViewAmplification> {
        self.views.iter().find(|v| v.orientation == orientation)
    }
}

pub struct LevelSource {
    pub array: Array<dyn ReadableStorageTraits>,
    pub map: AxisMap,
    pub dims: Dims,
    pub chunks: Dims,
}

impl std::fmt::Debug for LevelSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LevelSource")
            .field("path", &self.array.path().as_str())
            .field("map", &self.map)
            .field("dims", &self.dims)
            .field("chunks", &self.chunks)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Dataset {
    pub root: PathBuf,
    pub format: ZarrFormat,
    /// Level-0 extent, TCZYX.
    pub dims: Dims,
    pub dtype: Dtype,
    /// Micrometres per voxel at level 0; `None` when the store carries no scale.
    pub scale: Option<PhysicalScale>,
    pub levels: Vec<Level>,
    pub channels: Vec<ChannelMeta>,
    #[serde(skip)]
    sources: Arc<Vec<LevelSource>>,
}

impl Dataset {
    pub fn level(&self, level: u32) -> Result<&Level, OpenError> {
        self.levels
            .get(level as usize)
            .ok_or(OpenError::NoSuchLevel {
                level,
                levels: self.levels.len() as u32,
            })
    }

    pub fn source(&self, level: u32) -> Result<&LevelSource, OpenError> {
        self.sources
            .get(level as usize)
            .ok_or(OpenError::NoSuchLevel {
                level,
                levels: self.levels.len() as u32,
            })
    }

    pub fn coarsest_level(&self) -> u32 {
        self.levels.len().saturating_sub(1) as u32
    }

    pub fn layout(&self) -> Vec<LayoutReport> {
        self.levels
            .iter()
            .map(|l| analyze_layout(l, self.dtype))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("dataset path does not exist: {0}")]
    NotFound(PathBuf),
    #[error("dataset path is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("cannot read store at {path}: {reason}")]
    Store { path: PathBuf, reason: String },
    #[error(
        "{0} is neither a zarr group nor a zarr array (no zarr.json, .zgroup or .zarray found)"
    )]
    NotAZarrStore(PathBuf),
    #[error("store has no `multiscales` metadata, so its pyramid levels cannot be resolved")]
    MissingMultiscales,
    #[error("`multiscales` metadata is present but lists no datasets")]
    EmptyMultiscales,
    #[error("malformed `{field}` metadata: {reason}")]
    MalformedMetadata { field: String, reason: String },
    #[error("level {level} array `{path}` cannot be opened: {reason}")]
    MissingLevel {
        level: u32,
        path: String,
        reason: String,
    },
    #[error(
        "axis names {names:?} are not a TCZYX subset containing Y and X (axes must be named t, c, z, y, x)"
    )]
    UnsupportedAxes { names: Vec<String> },
    #[error("axes {names:?} are not in TCZYX order, which OME-Zarr requires")]
    UnsupportedAxisOrder { names: Vec<String> },
    #[error("axis count {ndim} does not match the {axes} axes declared in metadata")]
    AxisCountMismatch { ndim: usize, axes: usize },
    #[error("unsupported sample type `{dtype}`: only uint8, uint16 and uint32 are supported")]
    UnsupportedDtype { dtype: String },
    #[error("level {level} has sample type `{found}` but level 0 has `{expected}`")]
    InconsistentDtype {
        level: u32,
        found: String,
        expected: String,
    },
    #[error(
        "level {level} has {found} timepoints/channels (t={t}, c={c}), level 0 has t={t0}, c={c0}"
    )]
    InconsistentLevelDims {
        level: u32,
        found: String,
        t: u64,
        c: u64,
        t0: u64,
        c0: u64,
    },
    #[error("level {level} does not exist (dataset has {levels} level(s))")]
    NoSuchLevel { level: u32, levels: u32 },
}

/// Open a store read-only.
pub fn open(path: &Path) -> Result<Dataset, OpenError> {
    if !path.exists() {
        return Err(OpenError::NotFound(path.to_path_buf()));
    }
    if !path.is_dir() {
        return Err(OpenError::NotADirectory(path.to_path_buf()));
    }
    let store: ReadableStorage =
        Arc::new(FilesystemStore::new(path).map_err(|e| OpenError::Store {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?);

    match Group::open(store.clone(), "/") {
        Ok(group) => open_multiscale(path, store, group.attributes()),
        Err(_) => match Array::open(store.clone(), "/") {
            Ok(array) => open_bare_array(path, array),
            Err(_) => Err(OpenError::NotAZarrStore(path.to_path_buf())),
        },
    }
}

fn ngff_root(attributes: &Map<String, Value>) -> &Map<String, Value> {
    match attributes.get("ome").and_then(Value::as_object) {
        Some(ome) if ome.contains_key("multiscales") => ome,
        _ => attributes,
    }
}

fn open_multiscale(
    root: &Path,
    store: ReadableStorage,
    attributes: &Map<String, Value>,
) -> Result<Dataset, OpenError> {
    let ngff = ngff_root(attributes);
    let multiscale = ngff
        .get("multiscales")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_object)
        .ok_or(OpenError::MissingMultiscales)?;

    let axes = multiscale_axes(multiscale)?;
    let axis_names: Option<Vec<String>> = axes
        .as_ref()
        .map(|a| a.iter().map(|axis| axis.name.clone()).collect());
    let datasets = multiscale
        .get("datasets")
        .and_then(Value::as_array)
        .ok_or(OpenError::MissingMultiscales)?;
    if datasets.is_empty() {
        return Err(OpenError::EmptyMultiscales);
    }

    let outer_scale = scale_transform(multiscale.get("coordinateTransformations"));

    let mut sources = Vec::with_capacity(datasets.len());
    let mut levels = Vec::with_capacity(datasets.len());
    let mut level_scales: Vec<Option<Vec<f64>>> = Vec::with_capacity(datasets.len());

    for (index, entry) in datasets.iter().enumerate() {
        let entry = entry
            .as_object()
            .ok_or_else(|| OpenError::MalformedMetadata {
                field: "multiscales[0].datasets".into(),
                reason: format!("entry {index} is not an object"),
            })?;
        let path = entry.get("path").and_then(Value::as_str).ok_or_else(|| {
            OpenError::MalformedMetadata {
                field: "multiscales[0].datasets".into(),
                reason: format!("entry {index} has no `path`"),
            }
        })?;
        let array = Array::open(store.clone(), &format!("/{}", path.trim_start_matches('/')))
            .map_err(|e| OpenError::MissingLevel {
                level: index as u32,
                path: path.to_string(),
                reason: e.to_string(),
            })?;
        let map = axis_map(&array, axis_names.as_deref())?;
        let source = level_source(array, map);
        levels.push(Level {
            index: index as u32,
            path: path.to_string(),
            dims: source.dims,
            chunks: source.chunks,
            factor: [1.0, 1.0, 1.0],
        });
        level_scales.push(
            scale_transform(entry.get("coordinateTransformations"))
                .map(|s| combine_scales(&s, outer_scale.as_deref())),
        );
        sources.push(source);
    }

    let dtype = validate_dtype(&sources)?;
    validate_level_consistency(&sources)?;
    let map = sources[0].map;
    let scale = physical_scale(level_scales[0].as_deref(), map, axes.as_deref());
    fill_factors(&mut levels, &level_scales, map);

    let dims = sources[0].dims;
    let channels = parse_channels(ngff.get("omero"), dims.c, dtype);

    Ok(Dataset {
        root: root.to_path_buf(),
        format: zarr_format(&sources[0].array),
        dims,
        dtype,
        scale,
        levels,
        channels,
        sources: Arc::new(sources),
    })
}

fn open_bare_array(
    root: &Path,
    array: Array<dyn ReadableStorageTraits>,
) -> Result<Dataset, OpenError> {
    let format = zarr_format(&array);
    let map = axis_map(&array, None)?;
    let source = level_source(array, map);
    let dims = source.dims;
    let level = Level {
        index: 0,
        path: String::new(),
        dims,
        chunks: source.chunks,
        factor: [1.0, 1.0, 1.0],
    };
    let sources = vec![source];
    let dtype = validate_dtype(&sources)?;
    Ok(Dataset {
        root: root.to_path_buf(),
        format,
        dims,
        dtype,
        scale: None,
        levels: vec![level],
        channels: parse_channels(None, dims.c, dtype),
        sources: Arc::new(sources),
    })
}

fn zarr_format(array: &Array<dyn ReadableStorageTraits>) -> ZarrFormat {
    match array.metadata() {
        zarrs::array::ArrayMetadata::V2(_) => ZarrFormat::V2,
        zarrs::array::ArrayMetadata::V3(_) => ZarrFormat::V3,
    }
}

fn level_source(array: Array<dyn ReadableStorageTraits>, map: AxisMap) -> LevelSource {
    let dims = map.normalize(array.shape());
    let chunk_indices = vec![0_u64; array.dimensionality()];
    let chunks = match array.chunk_shape(&chunk_indices) {
        Ok(shape) => {
            let extents: Vec<u64> = shape.iter().map(|n| n.get()).collect();
            map.normalize(&extents)
        }
        Err(_) => dims,
    };
    LevelSource {
        array,
        map,
        dims,
        chunks,
    }
}

struct AxisSpec {
    name: String,
    unit: Option<String>,
}

fn multiscale_axes(multiscale: &Map<String, Value>) -> Result<Option<Vec<AxisSpec>>, OpenError> {
    let Some(axes) = multiscale.get("axes") else {
        return Ok(None);
    };
    let axes = axes
        .as_array()
        .ok_or_else(|| OpenError::MalformedMetadata {
            field: "multiscales[0].axes".into(),
            reason: "not an array".into(),
        })?;
    let mut specs = Vec::with_capacity(axes.len());
    for (i, axis) in axes.iter().enumerate() {
        let spec = match axis {
            Value::String(s) => AxisSpec {
                name: s.clone(),
                unit: None,
            },
            Value::Object(o) => AxisSpec {
                name: o
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpenError::MalformedMetadata {
                        field: "multiscales[0].axes".into(),
                        reason: format!("axis {i} has no `name`"),
                    })?
                    .to_string(),
                unit: o.get("unit").and_then(Value::as_str).map(str::to_string),
            },
            _ => {
                return Err(OpenError::MalformedMetadata {
                    field: "multiscales[0].axes".into(),
                    reason: format!("axis {i} is neither a string nor an object"),
                });
            }
        };
        specs.push(spec);
    }
    Ok(Some(specs))
}

fn axis_map(
    array: &Array<dyn ReadableStorageTraits>,
    ngff_names: Option<&[String]>,
) -> Result<AxisMap, OpenError> {
    let ndim = array.dimensionality();
    let names: Vec<String> = match ngff_names {
        Some(names) => {
            if names.len() != ndim {
                return Err(OpenError::AxisCountMismatch {
                    ndim,
                    axes: names.len(),
                });
            }
            names.to_vec()
        }
        None => match array.dimension_names() {
            Some(dim_names)
                if dim_names.len() == ndim
                    && dim_names
                        .iter()
                        .all(|n| n.as_deref().and_then(Axis::from_name).is_some()) =>
            {
                dim_names.iter().flatten().cloned().collect()
            }
            _ => conventional_axes(ndim)?,
        },
    };

    let mut slots: [Option<u8>; 5] = [None; 5];
    for (i, name) in names.iter().enumerate() {
        let axis = Axis::from_name(name).ok_or_else(|| OpenError::UnsupportedAxes {
            names: names.clone(),
        })?;
        if slots[axis.slot()].is_some() {
            return Err(OpenError::MalformedMetadata {
                field: "axes".into(),
                reason: format!("axis `{}` appears twice", axis.as_str()),
            });
        }
        slots[axis.slot()] = Some(i as u8);
    }
    if slots[Axis::Y.slot()].is_none() || slots[Axis::X.slot()].is_none() {
        return Err(OpenError::UnsupportedAxes { names });
    }
    let present: Vec<u8> = slots.iter().flatten().copied().collect();
    if present.windows(2).any(|w| w[0] >= w[1]) {
        return Err(OpenError::UnsupportedAxisOrder { names });
    }
    Ok(AxisMap::new(slots, ndim))
}

fn conventional_axes(ndim: usize) -> Result<Vec<String>, OpenError> {
    let names: &[&str] = match ndim {
        2 => &["y", "x"],
        3 => &["z", "y", "x"],
        4 => &["c", "z", "y", "x"],
        5 => &["t", "c", "z", "y", "x"],
        _ => {
            return Err(OpenError::UnsupportedAxes {
                names: (0..ndim).map(|i| format!("dim{i}")).collect(),
            });
        }
    };
    Ok(names.iter().map(|s| (*s).to_string()).collect())
}

fn validate_dtype(sources: &[LevelSource]) -> Result<Dtype, OpenError> {
    let dtype = map_dtype(sources[0].array.data_type())?;
    for (index, source) in sources.iter().enumerate().skip(1) {
        let level_dtype =
            map_dtype(source.array.data_type()).map_err(|_| OpenError::InconsistentDtype {
                level: index as u32,
                found: source.array.data_type().to_string(),
                expected: sources[0].array.data_type().to_string(),
            })?;
        if level_dtype != dtype {
            return Err(OpenError::InconsistentDtype {
                level: index as u32,
                found: source.array.data_type().to_string(),
                expected: sources[0].array.data_type().to_string(),
            });
        }
    }
    Ok(dtype)
}

fn map_dtype(dtype: &DataType) -> Result<Dtype, OpenError> {
    if *dtype == data_type::uint8() {
        Ok(Dtype::U8)
    } else if *dtype == data_type::uint16() {
        Ok(Dtype::U16)
    } else if *dtype == data_type::uint32() {
        Ok(Dtype::U32)
    } else {
        Err(OpenError::UnsupportedDtype {
            dtype: dtype.to_string(),
        })
    }
}

fn validate_level_consistency(sources: &[LevelSource]) -> Result<(), OpenError> {
    let base = sources[0].dims;
    for (index, source) in sources.iter().enumerate().skip(1) {
        let dims = source.dims;
        if dims.t != base.t || dims.c != base.c {
            return Err(OpenError::InconsistentLevelDims {
                level: index as u32,
                found: format!("t={}, c={}", dims.t, dims.c),
                t: dims.t,
                c: dims.c,
                t0: base.t,
                c0: base.c,
            });
        }
    }
    Ok(())
}

fn scale_transform(transforms: Option<&Value>) -> Option<Vec<f64>> {
    transforms?.as_array()?.iter().find_map(|t| {
        let obj = t.as_object()?;
        if obj.get("type").and_then(Value::as_str)? != "scale" {
            return None;
        }
        Some(
            obj.get("scale")?
                .as_array()?
                .iter()
                .map(|v| v.as_f64().unwrap_or(1.0))
                .collect::<Vec<f64>>(),
        )
    })
}

fn combine_scales(inner: &[f64], outer: Option<&[f64]>) -> Vec<f64> {
    match outer {
        Some(outer) => inner
            .iter()
            .enumerate()
            .map(|(i, v)| v * outer.get(i).copied().unwrap_or(1.0))
            .collect(),
        None => inner.to_vec(),
    }
}

fn space_unit_to_micrometre(unit: Option<&str>) -> f64 {
    match unit.map(|u| u.trim().to_ascii_lowercase()).as_deref() {
        Some("nanometer" | "nanometre" | "nm") => 1e-3,
        Some("millimeter" | "millimetre" | "mm") => 1e3,
        Some("centimeter" | "centimetre" | "cm") => 1e4,
        Some("meter" | "metre" | "m") => 1e6,
        Some("angstrom") => 1e-4,
        // micrometer, um, µm, unset, or unknown: assume µm.
        _ => 1.0,
    }
}

fn physical_scale(
    scale: Option<&[f64]>,
    map: AxisMap,
    axes: Option<&[AxisSpec]>,
) -> Option<PhysicalScale> {
    let scale = scale?;
    let pick = |axis: Axis| -> Option<f64> {
        let i = map.slot(axis)?;
        let unit = axes.and_then(|a| a.get(i)).and_then(|a| a.unit.as_deref());
        scale
            .get(i)
            .copied()
            .map(|v| v * space_unit_to_micrometre(unit))
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    let y = pick(Axis::Y)?;
    let x = pick(Axis::X)?;
    let z = pick(Axis::Z).unwrap_or(1.0);
    Some(PhysicalScale { z, y, x })
}

fn fill_factors(levels: &mut [Level], scales: &[Option<Vec<f64>>], map: AxisMap) {
    let base_dims = levels.first().map(|l| l.dims);
    let base_scale = scales.first().and_then(|s| s.clone());
    for (index, level) in levels.iter_mut().enumerate() {
        let stored = scales
            .get(index)
            .and_then(|s| s.as_ref())
            .zip(base_scale.as_ref())
            .and_then(|(level_scale, base)| {
                let ratio = |axis: Axis| -> Option<f64> {
                    let i = map.slot(axis)?;
                    let (a, b) = (level_scale.get(i)?, base.get(i)?);
                    if !a.is_finite() || !b.is_finite() || *b == 0.0 {
                        return None;
                    }
                    Some(a / b)
                };
                Some([
                    ratio(Axis::Z).unwrap_or(1.0),
                    ratio(Axis::Y)?,
                    ratio(Axis::X)?,
                ])
            });
        level.factor = stored.unwrap_or_else(|| match base_dims {
            Some(base) => [
                ratio_of(base.z, level.dims.z),
                ratio_of(base.y, level.dims.y),
                ratio_of(base.x, level.dims.x),
            ],
            None => [1.0, 1.0, 1.0],
        });
    }
}

fn ratio_of(base: u64, level: u64) -> f64 {
    if level == 0 {
        1.0
    } else {
        base as f64 / level as f64
    }
}

fn parse_channels(omero: Option<&Value>, count: u64, dtype: Dtype) -> Vec<ChannelMeta> {
    let full_max = match dtype {
        Dtype::U8 => u8::MAX as f64,
        Dtype::U16 => u16::MAX as f64,
        Dtype::U32 => u32::MAX as f64,
    };
    let stored = omero
        .and_then(Value::as_object)
        .and_then(|o| o.get("channels"))
        .and_then(Value::as_array);

    (0..count)
        .map(|index| {
            let entry = stored
                .and_then(|c| c.get(index as usize))
                .and_then(Value::as_object);
            let default_color =
                DEFAULT_CHANNEL_COLORS[(index as usize) % DEFAULT_CHANNEL_COLORS.len()];
            let Some(entry) = entry else {
                return ChannelMeta {
                    index: index as u32,
                    name: format!("Channel {}", index + 1),
                    color: default_color.to_string(),
                    window: [0.0, full_max],
                    limits: [0.0, full_max],
                    active: true,
                    defaulted: true,
                };
            };
            let window = entry.get("window").and_then(Value::as_object);
            let number = |key: &str| -> Option<f64> {
                window
                    .and_then(|w| w.get(key))
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
            };
            let limits = [
                number("min").unwrap_or(0.0),
                number("max").unwrap_or(full_max),
            ];
            ChannelMeta {
                index: index as u32,
                name: entry
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Channel {}", index + 1)),
                color: entry
                    .get("color")
                    .and_then(Value::as_str)
                    .map(|c| c.trim_start_matches('#').to_ascii_uppercase())
                    .unwrap_or_else(|| default_color.to_string()),
                window: [
                    number("start").unwrap_or(limits[0]),
                    number("end").unwrap_or(limits[1]),
                ],
                limits,
                active: entry.get("active").and_then(Value::as_bool).unwrap_or(true),
                defaulted: false,
            }
        })
        .collect()
}

/// Read amplification per orientation from chunk-shape arithmetic, no pixel reads.
pub fn analyze_layout(level: &Level, dtype: Dtype) -> LayoutReport {
    let element = dtype.size_bytes() as u64;
    let dims = level.dims;
    let chunks = clamp_chunks(level.chunks, dims);
    let chunk_voxels = chunks.t * chunks.c * chunks.z * chunks.y * chunks.x;

    let views: Vec<ViewAmplification> = [Orientation::XY, Orientation::XZ, Orientation::YZ]
        .into_iter()
        .map(|orientation| {
            let (needed_voxels, column_chunks, tiles) = match orientation {
                Orientation::XY => (
                    dims.y * dims.x,
                    1,
                    tiles_of(dims.y, chunks.y) * tiles_of(dims.x, chunks.x),
                ),
                Orientation::XZ => (
                    dims.z * dims.x,
                    tiles_of(dims.z, chunks.z),
                    tiles_of(dims.x, chunks.x),
                ),
                Orientation::YZ => (
                    dims.z * dims.y,
                    tiles_of(dims.z, chunks.z),
                    tiles_of(dims.y, chunks.y),
                ),
            };
            let chunks_decoded = column_chunks * tiles;
            let bytes_needed = needed_voxels * element;
            let bytes_decoded = chunks_decoded * chunk_voxels * element;
            let amplification = if bytes_needed == 0 {
                1.0
            } else {
                bytes_decoded as f64 / bytes_needed as f64
            };
            let hostile = match orientation {
                Orientation::XY => amplification > MAX_XY_AMPLIFICATION,
                Orientation::XZ | Orientation::YZ => column_chunks > MAX_ORTHO_COLUMN_CHUNKS,
            };
            ViewAmplification {
                orientation,
                bytes_needed,
                bytes_decoded,
                chunks_decoded,
                column_chunks,
                amplification,
                hostile,
            }
        })
        .collect();

    let hostile_views: Vec<Orientation> = views
        .iter()
        .filter(|v| v.hostile)
        .map(|v| v.orientation)
        .collect();
    LayoutReport {
        level: level.index,
        hostile: !hostile_views.is_empty(),
        hostile_views,
        views,
    }
}

fn clamp_chunks(chunks: Dims, dims: Dims) -> Dims {
    Dims {
        t: chunks.t.clamp(1, dims.t.max(1)),
        c: chunks.c.clamp(1, dims.c.max(1)),
        z: chunks.z.clamp(1, dims.z.max(1)),
        y: chunks.y.clamp(1, dims.y.max(1)),
        x: chunks.x.clamp(1, dims.x.max(1)),
    }
}

fn tiles_of(extent: u64, chunk: u64) -> u64 {
    if chunk == 0 {
        1
    } else {
        extent.div_ceil(chunk).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_units_map_to_micrometres() {
        assert_eq!(space_unit_to_micrometre(Some("micrometer")), 1.0);
        assert_eq!(space_unit_to_micrometre(Some("nanometer")), 1e-3);
        assert_eq!(space_unit_to_micrometre(Some("MILLIMETRE")), 1e3);
        assert_eq!(space_unit_to_micrometre(None), 1.0);
        assert_eq!(space_unit_to_micrometre(Some("furlong")), 1.0);
    }
}
