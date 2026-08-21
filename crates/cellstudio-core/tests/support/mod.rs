#![allow(dead_code)]
//! Programmatic in-memory OME-Zarr builder for tests.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cellstudio_core::axes::{Axis, Dims};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use zarrs::array::codec::GzipCodec;
use zarrs::array::{Array, ArrayBuilder, ArrayMetadataV2, FillValueMetadata, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::{Group, GroupBuilder};
use zarrs::metadata::v2::GroupMetadataV2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    V2,
    V3,
}

/// What to write. Defaults are a 2-level XY-only pyramid over a full TCZYX store.
#[derive(Clone, Debug)]
pub struct Spec {
    pub format: Format,
    pub dims: Dims,
    pub chunks: Dims,
    pub levels: usize,
    /// Pyramid halves y and x only; z stays constant (the development dataset's shape).
    pub xy_only: bool,
    pub axes: Vec<Axis>,
    /// Level-0 scale in `axes` order; `None` writes no `coordinateTransformations`.
    pub scale: Option<Vec<f64>>,
    pub units: bool,
    pub space_unit: &'static str,
    pub omero: bool,
    /// v3 only: compress chunks with gzip, so a data with real decode cost still
    /// occupies a few KB on disk.
    pub compress: bool,
    pub timestamps: Option<Vec<Value>>,
    /// False writes a bare array at the store root instead of a multiscale group.
    pub multiscales: bool,
}

impl Spec {
    pub fn new(format: Format) -> Self {
        Self {
            format,
            dims: Dims {
                t: 2,
                c: 2,
                z: 4,
                y: 8,
                x: 8,
            },
            chunks: Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 4,
                x: 4,
            },
            levels: 2,
            xy_only: true,
            axes: vec![Axis::T, Axis::C, Axis::Z, Axis::Y, Axis::X],
            scale: Some(vec![600.0, 1.0, 2.0, 0.5, 0.5]),
            units: true,
            space_unit: "micrometer",
            omero: false,
            compress: false,
            timestamps: None,
            multiscales: true,
        }
    }

    pub fn dims(mut self, dims: Dims) -> Self {
        self.dims = dims;
        self
    }

    pub fn chunks(mut self, chunks: Dims) -> Self {
        self.chunks = chunks;
        self
    }

    pub fn levels(mut self, levels: usize) -> Self {
        self.levels = levels;
        self
    }

    pub fn axes(mut self, axes: &[Axis]) -> Self {
        self.axes = axes.to_vec();
        self
    }

    pub fn scale(mut self, scale: Option<Vec<f64>>) -> Self {
        self.scale = scale;
        self
    }

    pub fn space_unit(mut self, unit: &'static str) -> Self {
        self.space_unit = unit;
        self
    }

    pub fn omero(mut self, omero: bool) -> Self {
        self.omero = omero;
        self
    }

    pub fn compress(mut self) -> Self {
        self.compress = true;
        self
    }

    pub fn timestamps(mut self, timestamps: Vec<Value>) -> Self {
        self.timestamps = Some(timestamps);
        self
    }

    pub fn isotropic_pyramid(mut self) -> Self {
        self.xy_only = false;
        self
    }

    pub fn bare_array(mut self) -> Self {
        self.multiscales = false;
        self.levels = 1;
        self
    }

    fn level_dims(&self, level: usize) -> Dims {
        let f = 1_u64 << level;
        Dims {
            t: self.dims.t,
            c: self.dims.c,
            z: if self.xy_only {
                self.dims.z
            } else {
                (self.dims.z / f).max(1)
            },
            y: (self.dims.y / f).max(1),
            x: (self.dims.x / f).max(1),
        }
    }

    fn level_scale(&self, level: usize) -> Option<Vec<f64>> {
        let base = self.scale.as_ref()?;
        let f = (1_u64 << level) as f64;
        let zf = if self.xy_only { 1.0 } else { f };
        Some(
            self.axes
                .iter()
                .enumerate()
                .map(|(i, axis)| {
                    let v = base.get(i).copied().unwrap_or(1.0);
                    match axis {
                        Axis::Z => v * zf,
                        Axis::Y | Axis::X => v * f,
                        _ => v,
                    }
                })
                .collect(),
        )
    }
}

/// A written store plus the exact values it holds, so tests compare reads against the
/// source data rather than against a second implementation.
pub struct Data {
    _dir: TempDir,
    pub root: PathBuf,
    pub spec: Spec,
    pub level_dims: Vec<Dims>,
    /// Per level, TCZYX row-major.
    pub data: Vec<Vec<u16>>,
}

impl Data {
    pub fn at(&self, level: usize, t: u64, c: u64, z: u64, y: u64, x: u64) -> u16 {
        let dims = self.level_dims[level];
        let index = (((t * dims.c + c) * dims.z + z) * dims.y + y) * dims.x + x;
        self.data[level][index as usize]
    }

    /// XZ plane at `y`: rows along z, columns along x.
    pub fn xz_plane(&self, level: usize, t: u64, c: u64, y: u64) -> Vec<u16> {
        let dims = self.level_dims[level];
        (0..dims.z)
            .flat_map(|z| (0..dims.x).map(move |x| (z, x)))
            .map(|(z, x)| self.at(level, t, c, z, y, x))
            .collect()
    }

    /// YZ plane at `x`: rows along z, columns along y.
    pub fn yz_plane(&self, level: usize, t: u64, c: u64, x: u64) -> Vec<u16> {
        let dims = self.level_dims[level];
        (0..dims.z)
            .flat_map(|z| (0..dims.y).map(move |y| (z, y)))
            .map(|(z, y)| self.at(level, t, c, z, y, x))
            .collect()
    }

    pub fn volume(&self, level: usize, t: u64, c: u64) -> Vec<u16> {
        let dims = self.level_dims[level];
        (0..dims.z)
            .flat_map(|z| (0..dims.y).flat_map(move |y| (0..dims.x).map(move |x| (z, y, x))))
            .map(|(z, y, x)| self.at(level, t, c, z, y, x))
            .collect()
    }
}

/// Deterministic content: every axis moves the value, so a transposed read cannot pass.
fn sample(level: usize, t: u64, c: u64, z: u64, y: u64, x: u64) -> u16 {
    let v = level as u64 * 30_011 + t * 7_919 + c * 1_301 + z * 211 + y * 17 + x * 3 + 1;
    (v % 65_521) as u16
}

pub fn build(spec: Spec) -> Data {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("data.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(&root).expect("store"));

    let level_dims: Vec<Dims> = (0..spec.levels).map(|l| spec.level_dims(l)).collect();
    let mut data = Vec::with_capacity(spec.levels);

    if spec.multiscales {
        write_group(&store, &spec, &level_dims);
    }
    for (level, dims) in level_dims.iter().enumerate() {
        let values = generate(level, *dims);
        let path = if spec.multiscales {
            format!("/{level}")
        } else {
            "/".to_string()
        };
        write_level(&store, &path, &spec, *dims, &values);
        data.push(values);
    }

    Data {
        _dir: dir,
        root,
        spec,
        level_dims,
        data,
    }
}

fn generate(level: usize, dims: Dims) -> Vec<u16> {
    let mut out = Vec::with_capacity(dims.voxels() as usize);
    for t in 0..dims.t {
        for c in 0..dims.c {
            for z in 0..dims.z {
                for y in 0..dims.y {
                    for x in 0..dims.x {
                        out.push(sample(level, t, c, z, y, x));
                    }
                }
            }
        }
    }
    out
}

fn store_shape(spec: &Spec, dims: Dims) -> Vec<u64> {
    spec.axes.iter().map(|axis| dims.get(*axis)).collect()
}

fn store_chunks(spec: &Spec, dims: Dims) -> Vec<u64> {
    spec.axes
        .iter()
        .map(|axis| spec.chunks.get(*axis).clamp(1, dims.get(*axis).max(1)))
        .collect()
}

fn write_group(store: &Arc<FilesystemStore>, spec: &Spec, level_dims: &[Dims]) {
    let mut ngff = Map::new();
    ngff.insert("multiscales".into(), json!([multiscale(spec, level_dims)]));
    if spec.omero {
        ngff.insert("omero".into(), omero(spec));
    }

    let mut attributes = Map::new();
    match spec.format {
        Format::V2 => {
            for (k, v) in ngff {
                attributes.insert(k, v);
            }
        }
        Format::V3 => {
            let mut ome = ngff;
            ome.insert("version".into(), json!("0.5"));
            attributes.insert("ome".into(), Value::Object(ome));
        }
    }
    if let Some(stamps) = &spec.timestamps {
        attributes.insert("time_stamps".into(), json!(stamps));
    }

    match spec.format {
        Format::V2 => {
            let metadata = GroupMetadataV2::new().with_attributes(attributes);
            let group =
                Group::new_with_metadata(store.clone(), "/", metadata.into()).expect("v2 group");
            group.store_metadata().expect("store v2 group metadata");
        }
        Format::V3 => {
            let mut group = GroupBuilder::new()
                .build(store.clone(), "/")
                .expect("v3 group");
            *group.attributes_mut() = attributes;
            group.store_metadata().expect("store v3 group metadata");
        }
    }
}

fn multiscale(spec: &Spec, level_dims: &[Dims]) -> Value {
    let axes: Vec<Value> = spec
        .axes
        .iter()
        .map(|axis| match axis {
            Axis::T => {
                if spec.units {
                    json!({ "name": "t", "type": "time", "unit": "second" })
                } else {
                    json!({ "name": "t", "type": "time" })
                }
            }
            Axis::C => json!({ "name": "c", "type": "channel" }),
            other => {
                if spec.units {
                    json!({ "name": other.as_str(), "type": "space", "unit": spec.space_unit })
                } else {
                    json!({ "name": other.as_str(), "type": "space" })
                }
            }
        })
        .collect();
    let datasets: Vec<Value> = (0..level_dims.len())
        .map(|level| match spec.level_scale(level) {
            Some(scale) => json!({
                "path": level.to_string(),
                "coordinateTransformations": [{ "type": "scale", "scale": scale }],
            }),
            None => json!({ "path": level.to_string() }),
        })
        .collect();
    json!({ "version": "0.4", "name": "data", "axes": axes, "datasets": datasets })
}

fn omero(spec: &Spec) -> Value {
    let palette = ["FF0000", "FFB100", "37FF00", "0066FF"];
    let channels: Vec<Value> = (0..spec.dims.c)
        .map(|c| {
            json!({
                "active": true,
                "color": palette[(c as usize) % palette.len()],
                "label": format!("probe-{c}"),
                "window": { "min": 0.0, "max": 65535.0, "start": 100.0 * (c + 1) as f64, "end": 4000.0 + c as f64 },
            })
        })
        .collect();
    json!({ "version": "0.4", "channels": channels, "rdefs": { "defaultT": 0, "defaultZ": 1 } })
}

fn write_level(store: &Arc<FilesystemStore>, path: &str, spec: &Spec, dims: Dims, values: &[u16]) {
    let shape = store_shape(spec, dims);
    let chunks = store_chunks(spec, dims);
    let array = match spec.format {
        Format::V2 => {
            let chunk_shape: Vec<NonZeroU64> = chunks
                .iter()
                .map(|c| NonZeroU64::new(*c).expect("nonzero chunk"))
                .collect();
            let metadata = ArrayMetadataV2::new(
                shape.clone(),
                chunk_shape,
                "<u2".into(),
                FillValueMetadata::from(0u16),
                None,
                None,
            );
            let array =
                Array::new_with_metadata(store.clone(), path, metadata.into()).expect("v2 array");
            array.store_metadata().expect("store v2 array metadata");
            array
        }
        Format::V3 => {
            let mut builder =
                ArrayBuilder::new(shape.clone(), chunks.clone(), data_type::uint16(), 0u16);
            builder.dimension_names(Some(
                spec.axes.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
            ));
            if spec.compress {
                builder
                    .bytes_to_bytes_codecs(vec![Arc::new(GzipCodec::new(5).expect("gzip level"))]);
            }
            let array = builder.build(store.clone(), path).expect("v3 array");
            array.store_metadata().expect("store v3 array metadata");
            array
        }
    };

    // TCZYX strides for indexing the generated data.
    let extents = dims.as_array();
    let mut strides = [1_u64; 5];
    for i in (0..4).rev() {
        strides[i] = strides[i + 1] * extents[i + 1];
    }

    let grid: Vec<u64> = shape
        .iter()
        .zip(&chunks)
        .map(|(s, c)| s.div_ceil(*c).max(1))
        .collect();
    let mut cell = vec![0_u64; grid.len()];
    loop {
        // Chunks are stored whole; positions past the array edge take the fill value.
        let ranges: Vec<std::ops::Range<u64>> = cell
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let start = g * chunks[i];
                start..start + chunks[i]
            })
            .collect();
        let mut elements = Vec::new();
        let walk = ChunkWalk {
            spec,
            ranges: &ranges,
            extents,
            strides,
            values,
        };
        walk.emit(0, &mut [0; 5], &mut elements);
        array.store_chunk(&cell, elements).expect("store chunk");

        // Odometer over the chunk grid.
        let mut axis = cell.len();
        loop {
            if axis == 0 {
                return;
            }
            axis -= 1;
            cell[axis] += 1;
            if cell[axis] < grid[axis] {
                break;
            }
            cell[axis] = 0;
        }
    }
}

/// Walks one chunk in store order, reading from the TCZYX-indexed source data.
struct ChunkWalk<'a> {
    spec: &'a Spec,
    ranges: &'a [std::ops::Range<u64>],
    extents: [u64; 5],
    strides: [u64; 5],
    values: &'a [u16],
}

impl ChunkWalk<'_> {
    fn emit(&self, axis: usize, coords: &mut [u64; 5], out: &mut Vec<u16>) {
        if axis == self.ranges.len() {
            if (0..5).any(|i| coords[i] >= self.extents[i]) {
                out.push(0);
                return;
            }
            let index: u64 = (0..5).map(|i| coords[i] * self.strides[i]).sum();
            out.push(self.values[index as usize]);
            return;
        }
        let slot = self.spec.axes[axis].slot();
        for value in self.ranges[axis].clone() {
            coords[slot] = value;
            self.emit(axis + 1, coords, out);
        }
        coords[slot] = 0;
    }
}

/// A v3 store whose sample type is float32, for dtype rejection.
pub fn float_store() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("float.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(&root).expect("store"));
    let mut group = GroupBuilder::new()
        .build(store.clone(), "/")
        .expect("group");
    *group.attributes_mut() = json!({
        "multiscales": [{
            "version": "0.4",
            "axes": [{ "name": "y", "type": "space" }, { "name": "x", "type": "space" }],
            "datasets": [{ "path": "0" }],
        }]
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    group.store_metadata().expect("group metadata");
    let array = ArrayBuilder::new(vec![4, 4], vec![4, 4], data_type::float32(), 0.0f32)
        .build(store, "/0")
        .expect("array");
    array.store_metadata().expect("array metadata");
    (dir, root)
}

/// A store whose group metadata carries attributes but no `multiscales`.
pub fn group_without_multiscales() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("plain.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(&root).expect("store"));
    let mut group = GroupBuilder::new().build(store, "/").expect("group");
    *group.attributes_mut() = json!({ "note": "no multiscales here" })
        .as_object()
        .cloned()
        .unwrap_or_default();
    group.store_metadata().expect("group metadata");
    (dir, root)
}

/// A multiscale group whose level array was never written.
pub fn multiscales_without_level() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("dangling.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(&root).expect("store"));
    let mut group = GroupBuilder::new().build(store, "/").expect("group");
    *group.attributes_mut() = json!({
        "multiscales": [{
            "version": "0.4",
            "axes": [{ "name": "y" }, { "name": "x" }],
            "datasets": [{ "path": "0" }],
        }]
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    group.store_metadata().expect("group metadata");
    (dir, root)
}

/// Axes named outside TCZYX.
pub fn unsupported_axes() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("weird-axes.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(&root).expect("store"));
    let mut group = GroupBuilder::new()
        .build(store.clone(), "/")
        .expect("group");
    *group.attributes_mut() = json!({
        "multiscales": [{
            "version": "0.4",
            "axes": [{ "name": "wavelength" }, { "name": "y" }, { "name": "x" }],
            "datasets": [{ "path": "0" }],
        }]
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    group.store_metadata().expect("group metadata");
    let array = ArrayBuilder::new(vec![2, 4, 4], vec![2, 4, 4], data_type::uint16(), 0u16)
        .build(store, "/0")
        .expect("array");
    array.store_metadata().expect("array metadata");
    (dir, root)
}

/// Space axes before the channel axis, which OME-Zarr forbids.
pub fn out_of_order_axes() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("order.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(&root).expect("store"));
    let mut group = GroupBuilder::new()
        .build(store.clone(), "/")
        .expect("group");
    *group.attributes_mut() = json!({
        "multiscales": [{
            "version": "0.4",
            "axes": [{ "name": "z" }, { "name": "y" }, { "name": "x" }, { "name": "c" }],
            "datasets": [{ "path": "0" }],
        }]
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    group.store_metadata().expect("group metadata");
    let array = ArrayBuilder::new(
        vec![2, 4, 4, 2],
        vec![2, 4, 4, 2],
        data_type::uint16(),
        0u16,
    )
    .build(store, "/0")
    .expect("array");
    array.store_metadata().expect("array metadata");
    (dir, root)
}

/// Sum of every file's length under `path`, with each file's relative name, enough to
/// prove a store was not written to.
pub fn store_digest(path: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    collect(path, path, &mut out);
    out.sort();
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, u64, std::time::SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            collect(root, &path, out);
        } else {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            out.push((name, meta.len(), modified));
        }
    }
}

/// Reinterpret little-endian bytes as u16 samples.
pub fn as_u16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect()
}
