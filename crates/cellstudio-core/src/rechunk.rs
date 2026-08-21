use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::{Map, Value, json};
use zarrs::array::codec::ZstdCodec;
use zarrs::array::{ArrayBuilder, ArrayBytes, ArraySubset, CodecOptions, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::GroupBuilder;

use crate::axes::{Dims, Dtype};
use crate::dataset::{Dataset, Level, OpenError};

/// Starting-point brick shape; tuned per project by Spike 2.
pub const DEFAULT_BRICK: Dims = Dims {
    t: 1,
    c: 1,
    z: 16,
    y: 256,
    x: 256,
};

#[derive(Debug, thiserror::Error)]
pub enum RechunkError {
    #[error(transparent)]
    Dataset(#[from] OpenError),
    #[error("brick targets must cover one timepoint and one channel, got t={t}, c={c}")]
    UnsupportedTarget { t: u64, c: u64 },
    #[error("zarr error: {0}")]
    Zarr(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Write a brick working copy of every level of `ds` into `out`, returning `out`.
/// `progress` is called on the calling thread with a 0.0–1.0 fraction.
pub fn rechunk(
    ds: &Dataset,
    out: &Path,
    target: Dims,
    progress: &dyn Fn(f32),
) -> Result<PathBuf, RechunkError> {
    if target.t != 1 || target.c != 1 {
        return Err(RechunkError::UnsupportedTarget {
            t: target.t,
            c: target.c,
        });
    }
    std::fs::create_dir_all(out).map_err(|e| RechunkError::Io {
        path: out.to_path_buf(),
        source: e,
    })?;
    let store = Arc::new(FilesystemStore::new(out).map_err(|e| RechunkError::Zarr(e.to_string()))?);

    let mut group = GroupBuilder::new()
        .build(store.clone(), "/")
        .map_err(|e| RechunkError::Zarr(e.to_string()))?;
    *group.attributes_mut() = working_copy_attributes(ds);
    group
        .store_metadata()
        .map_err(|e| RechunkError::Zarr(e.to_string()))?;

    let dtype = match ds.dtype {
        Dtype::U8 => data_type::uint8(),
        Dtype::U16 => data_type::uint16(),
        Dtype::U32 => data_type::uint32(),
    };
    let total_steps: u64 = ds
        .levels
        .iter()
        .map(|l| l.dims.t * l.dims.c)
        .sum::<u64>()
        .max(1);
    let mut done = 0_u64;
    progress(0.0);

    for level in &ds.levels {
        let source = ds.source(level.index)?;
        let dims = level.dims;
        let chunks = brick_shape(target, dims);
        let mut builder = ArrayBuilder::new(
            vec![dims.t, dims.c, dims.z, dims.y, dims.x],
            vec![chunks.t, chunks.c, chunks.z, chunks.y, chunks.x],
            dtype.clone(),
            0u64,
        );
        builder
            .bytes_to_bytes_codecs(vec![Arc::new(ZstdCodec::new(3, false))])
            .dimension_names(Some(["t", "c", "z", "y", "x"]));
        let array = builder
            .build(store.clone(), &format!("/{}", level.index))
            .map_err(|e| RechunkError::Zarr(e.to_string()))?;
        array
            .store_metadata()
            .map_err(|e| RechunkError::Zarr(e.to_string()))?;

        let grid = [
            dims.z.div_ceil(chunks.z).max(1),
            dims.y.div_ceil(chunks.y).max(1),
            dims.x.div_ceil(chunks.x).max(1),
        ];
        let options = CodecOptions::default().with_concurrent_target(1);
        for t in 0..dims.t {
            for c in 0..dims.c {
                let cells: Vec<[u64; 3]> = (0..grid[0])
                    .flat_map(|gz| {
                        (0..grid[1]).flat_map(move |gy| (0..grid[2]).map(move |gx| [gz, gy, gx]))
                    })
                    .collect();
                cells
                    .par_iter()
                    .try_for_each(|cell| -> Result<(), RechunkError> {
                        let origin = [cell[0] * chunks.z, cell[1] * chunks.y, cell[2] * chunks.x];
                        let extent = [
                            chunks.z.min(dims.z - origin[0]),
                            chunks.y.min(dims.y - origin[1]),
                            chunks.x.min(dims.x - origin[2]),
                        ];
                        let region = [
                            t..t + 1,
                            c..c + 1,
                            origin[0]..origin[0] + extent[0],
                            origin[1]..origin[1] + extent[1],
                            origin[2]..origin[2] + extent[2],
                        ];
                        let subset = ArraySubset::new_with_ranges(&source.map.project(&region));
                        let bytes: ArrayBytes<'static> = source
                            .array
                            .retrieve_array_subset_opt(&subset, &options)
                            .map_err(|e| RechunkError::Zarr(e.to_string()))?;
                        array
                            .store_chunk(&[t, c, cell[0], cell[1], cell[2]], bytes)
                            .map_err(|e| RechunkError::Zarr(e.to_string()))
                    })?;
                done += 1;
                progress(done as f32 / total_steps as f32);
            }
        }
    }
    Ok(out.to_path_buf())
}

/// Bricks never exceed the level they describe: a 3-plane stack gets 3-plane bricks.
pub fn brick_shape(target: Dims, dims: Dims) -> Dims {
    Dims {
        t: target.t.clamp(1, dims.t.max(1)),
        c: target.c.clamp(1, dims.c.max(1)),
        z: target.z.clamp(1, dims.z.max(1)),
        y: target.y.clamp(1, dims.y.max(1)),
        x: target.x.clamp(1, dims.x.max(1)),
    }
}

/// NGFF 0.5 attributes mirroring the source: same axes, same per-level scales, same
/// channel display metadata, so re-opening the working copy reports what the source did.
fn working_copy_attributes(ds: &Dataset) -> Map<String, Value> {
    let scale = ds.scale;
    let datasets: Vec<Value> = ds
        .levels
        .iter()
        .map(|level| {
            json!({
                "path": level.index.to_string(),
                "coordinateTransformations": [{
                    "type": "scale",
                    "scale": level_scale(level, scale, 1.0),
                }],
            })
        })
        .collect();
    let channels: Vec<Value> = ds
        .channels
        .iter()
        .map(|channel| {
            json!({
                "active": channel.active,
                "color": channel.color,
                "label": channel.name,
                "window": {
                    "min": channel.limits[0],
                    "max": channel.limits[1],
                    "start": channel.window[0],
                    "end": channel.window[1],
                },
            })
        })
        .collect();

    let mut attributes = Map::new();
    attributes.insert(
        "ome".into(),
        json!({
            "version": "0.5",
            "multiscales": [{
                "name": "cellstudio-working-copy",
                "axes": [
                    { "name": "t", "type": "time", "unit": "second" },
                    { "name": "c", "type": "channel" },
                    { "name": "z", "type": "space", "unit": "micrometer" },
                    { "name": "y", "type": "space", "unit": "micrometer" },
                    { "name": "x", "type": "space", "unit": "micrometer" },
                ],
                "datasets": datasets,
            }],
            "omero": { "channels": channels },
        }),
    );
    attributes.insert("cellstudio_source".into(), json!(ds.root));
    attributes
}

fn level_scale(
    level: &Level,
    scale: Option<crate::axes::PhysicalScale>,
    interval: f64,
) -> Vec<f64> {
    let base = scale.unwrap_or(crate::axes::PhysicalScale::ISOTROPIC);
    vec![
        interval,
        1.0,
        base.z * level.factor[0],
        base.y * level.factor[1],
        base.x * level.factor[2],
    ]
}
