use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::LayerId;
use crate::axes::{Axis, Dims, Dtype, Orientation};
use crate::bricks::{Brick, BrickCache, BrickKey, read_sample};
use crate::dataset::{Dataset, OpenError};
use crate::volume::ProxyStore;

pub const MAX_VOLUME_BYTES: u64 = 512 << 20;

pub const HISTOGRAM_BINS: usize = 256;

pub const HISTOGRAM_MAX_SAMPLES: u64 = 1 << 22;

pub const HISTOGRAM_MAX_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrthoAxis {
    XZ,
    YZ,
}

impl OrthoAxis {
    pub fn orientation(self) -> Orientation {
        match self {
            OrthoAxis::XZ => Orientation::XZ,
            OrthoAxis::YZ => Orientation::YZ,
        }
    }

    pub fn index_axis(self) -> Axis {
        match self {
            OrthoAxis::XZ => Axis::Y,
            OrthoAxis::YZ => Axis::X,
        }
    }
}

/// One assembled plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub shape: [u32; 2],
    pub dtype: Dtype,
    pub level: u32,
    pub bytes: Bytes,
}

/// One volume at a single (t, c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub shape: [u32; 3],
    pub dtype: Dtype,
    pub level: u32,
    /// True when served from a prebuilt proxy rather than assembled from the pyramid.
    pub from_proxy: bool,
    pub bytes: Bytes,
}

/// Intensity distribution for the channel settings popover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Histogram {
    pub level: u32,
    pub range: [u64; 2],
    pub bins: Vec<u32>,
    pub min: u64,
    pub max: u64,
    pub samples: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Dataset(#[from] OpenError),
    #[error("layer {0:?} is not registered")]
    UnknownLayer(LayerId),
    #[error("{axis:?} index {index} is out of bounds (extent {extent})")]
    OutOfBounds { axis: Axis, index: u64, extent: u64 },
    #[error("zarr read failed: {0}")]
    Zarr(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("volume of {bytes} bytes exceeds the {limit} byte limit; request a coarser level")]
    VolumeTooLarge { bytes: u64, limit: u64 },
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),
}

/// Reads pixels for every registered layer through one shared brick cache.
pub struct ImageReader {
    dataset: Arc<Dataset>,
    bricks: Arc<BrickCache>,
    proxies: RwLock<Option<ProxyStore>>,
    histograms: Mutex<HashMap<(LayerId, u64, u64), Histogram>>,
}

impl std::fmt::Debug for ImageReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageReader")
            .field("root", &self.dataset.root)
            .field("bricks", &self.bricks.stats())
            .field("proxy", &self.proxy_level())
            .finish()
    }
}

impl ImageReader {
    pub fn new(dataset: Arc<Dataset>, capacity_bytes: usize) -> Self {
        let bricks = Arc::new(BrickCache::new(capacity_bytes));
        bricks.register_layer(LayerId::Image, dataset.clone());
        Self {
            dataset,
            bricks,
            proxies: RwLock::new(None),
            histograms: Mutex::new(HashMap::new()),
        }
    }

    pub fn dataset(&self) -> &Dataset {
        &self.dataset
    }

    pub fn bricks(&self) -> &Arc<BrickCache> {
        &self.bricks
    }

    pub fn register_layer(&self, layer: LayerId, dataset: Arc<Dataset>) {
        self.bricks.register_layer(layer, dataset);
    }

    pub fn attach_proxy(&self, proxy: ProxyStore) {
        *self.proxies.write() = Some(proxy);
    }

    pub fn proxy_level(&self) -> Option<u32> {
        self.proxies.read().as_ref().map(|p| p.level)
    }

    fn layer(&self, layer: LayerId) -> Result<Arc<Dataset>, ReadError> {
        self.bricks
            .layer(layer)
            .ok_or(ReadError::UnknownLayer(layer))
    }

    pub fn read_ortho_plane(
        &self,
        layer: LayerId,
        level: u32,
        axis: OrthoAxis,
        t: u64,
        c: u64,
        index: u64,
    ) -> Result<Plane, ReadError> {
        let dataset = self.layer(layer)?;
        let source = dataset.source(level)?;
        let (dims, chunks) = (source.dims, source.chunks);
        check(Axis::T, t, dims.t)?;
        check(Axis::C, c, dims.c)?;
        let index_axis = axis.index_axis();
        check(index_axis, index, dims.get(index_axis))?;

        let grid = BrickCache::grid_shape(dims, chunks);
        let cols = match axis {
            OrthoAxis::XZ => dims.x,
            OrthoAxis::YZ => dims.y,
        };
        let element = dataset.dtype.size_bytes();
        let mut out = vec![0_u8; (dims.z * cols) as usize * element];

        let (fixed, spans) = match axis {
            OrthoAxis::XZ => (index / chunks.y.max(1), grid[2]),
            OrthoAxis::YZ => (index / chunks.x.max(1), grid[1]),
        };
        let keys: Vec<BrickKey> = (0..grid[0])
            .flat_map(|gz| {
                (0..spans).map(move |gs| match axis {
                    OrthoAxis::XZ => [gz, fixed, gs],
                    OrthoAxis::YZ => [gz, gs, fixed],
                })
            })
            .map(|grid| BrickKey {
                layer,
                level,
                t,
                c,
                grid,
            })
            .collect();

        for brick in self.bricks.get_many(&keys)? {
            copy_ortho_rows(&brick, axis, index, cols, element, &mut out)?;
        }
        Ok(Plane {
            shape: [dims.z as u32, cols as u32],
            dtype: dataset.dtype,
            level,
            bytes: Bytes::from(out),
        })
    }

    pub fn read_volume(
        &self,
        layer: LayerId,
        level: u32,
        t: u64,
        c: u64,
    ) -> Result<Volume, ReadError> {
        if layer == LayerId::Image
            && let Some(proxy) = self.proxies.read().as_ref()
            && proxy.level == level
        {
            return proxy.read_volume(t, c);
        }

        let dataset = self.layer(layer)?;
        let source = dataset.source(level)?;
        let (dims, chunks) = (source.dims, source.chunks);
        check(Axis::T, t, dims.t)?;
        check(Axis::C, c, dims.c)?;

        let element = dataset.dtype.size_bytes() as u64;
        let bytes_total = dims.zyx_voxels() * element;
        if bytes_total > MAX_VOLUME_BYTES {
            return Err(ReadError::VolumeTooLarge {
                bytes: bytes_total,
                limit: MAX_VOLUME_BYTES,
            });
        }

        let grid = BrickCache::grid_shape(dims, chunks);
        let keys: Vec<BrickKey> = (0..grid[0])
            .flat_map(|gz| {
                (0..grid[1]).flat_map(move |gy| (0..grid[2]).map(move |gx| [gz, gy, gx]))
            })
            .map(|grid| BrickKey {
                layer,
                level,
                t,
                c,
                grid,
            })
            .collect();

        let mut out = vec![0_u8; bytes_total as usize];
        for brick in self.bricks.get_many(&keys)? {
            copy_volume_rows(&brick, dims, element as usize, &mut out)?;
        }
        Ok(Volume {
            shape: [dims.z as u32, dims.y as u32, dims.x as u32],
            dtype: dataset.dtype,
            level,
            from_proxy: false,
            bytes: Bytes::from(out),
        })
    }

    /// Full-resolution sample under the cursor.
    pub fn read_pixel(
        &self,
        layer: LayerId,
        t: u64,
        c: u64,
        zyx: [u64; 3],
    ) -> Result<u64, ReadError> {
        let dataset = self.layer(layer)?;
        let source = dataset.source(0)?;
        let (dims, chunks) = (source.dims, source.chunks);
        check(Axis::T, t, dims.t)?;
        check(Axis::C, c, dims.c)?;
        for (axis, (index, extent)) in [Axis::Z, Axis::Y, Axis::X]
            .into_iter()
            .zip(zyx.into_iter().zip([dims.z, dims.y, dims.x]))
        {
            check(axis, index, extent)?;
        }
        let key = BrickKey {
            layer,
            level: 0,
            t,
            c,
            grid: [
                zyx[0] / chunks.z.max(1),
                zyx[1] / chunks.y.max(1),
                zyx[2] / chunks.x.max(1),
            ],
        };
        let brick = self.bricks.get(key)?;
        brick
            .value(zyx)
            .ok_or(ReadError::Internal("pixel outside the brick it maps to"))
    }

    /// Sampled from the coarsest pyramid level and memoized per (layer, t, channel);
    /// never reads full-resolution data when a pyramid exists.
    pub fn histogram(&self, layer: LayerId, t: u64, c: u64) -> Result<Histogram, ReadError> {
        if let Some(hit) = self.histograms.lock().get(&(layer, t, c)) {
            return Ok(hit.clone());
        }
        let dataset = self.layer(layer)?;
        let level = dataset.coarsest_level();
        let source = dataset.source(level)?;
        let (dims, chunks) = (source.dims, source.chunks);
        check(Axis::T, t, dims.t)?;
        check(Axis::C, c, dims.c)?;

        let dtype = dataset.dtype;
        let range: [u64; 2] = match dtype {
            Dtype::U8 => [0, u64::from(u8::MAX)],
            Dtype::U16 => [0, u64::from(u16::MAX)],
            Dtype::U32 => [0, u64::from(u32::MAX)],
        };
        let element = dtype.size_bytes();
        let grid = BrickCache::grid_shape(dims, chunks);
        let bricks_total = grid[0] * grid[1] * grid[2];
        // Two strides keep the cost bounded even on a store with no pyramid: skip whole
        // bricks so the decode stays inside the byte budget, then skip voxels inside the
        // bricks that are read so the sample count stays inside its own bound.
        let brick_bytes = (chunks.zyx_voxels() * element as u64).max(1);
        let brick_stride = (bricks_total * brick_bytes)
            .div_ceil(HISTOGRAM_MAX_BYTES)
            .max(1);
        let sampled_bricks = bricks_total.div_ceil(brick_stride);
        let voxel_stride = (sampled_bricks * chunks.zyx_voxels())
            .div_ceil(HISTOGRAM_MAX_SAMPLES)
            .max(1);

        let mut bins = vec![0_u32; HISTOGRAM_BINS];
        let mut samples = 0_u64;
        let mut min = u64::MAX;
        let mut max = 0_u64;
        let mut index = 0;
        while index < bricks_total {
            let grid_index = [
                index / (grid[1] * grid[2]),
                (index / grid[2]) % grid[1],
                index % grid[2],
            ];
            let brick = self.bricks.get(BrickKey {
                layer,
                level,
                t,
                c,
                grid: grid_index,
            })?;
            let count = brick.bytes.len() / element;
            let mut offset = 0;
            while offset < count {
                if let Some(value) = read_sample(&brick.bytes, offset * element, dtype) {
                    let bin = (u128::from(value) * HISTOGRAM_BINS as u128
                        / (u128::from(range[1]) + 1)) as usize;
                    if let Some(slot) = bins.get_mut(bin.min(HISTOGRAM_BINS - 1)) {
                        *slot += 1;
                    }
                    min = min.min(value);
                    max = max.max(value);
                    samples += 1;
                }
                offset += voxel_stride as usize;
            }
            index += brick_stride;
        }
        let histogram = Histogram {
            level,
            range,
            bins,
            min: if samples == 0 { 0 } else { min },
            max,
            samples,
        };
        let mut memo = self.histograms.lock();
        // The popover only ever needs the recent (t, channel) pairs; a full flush is
        // cheaper than tracking per-entry recency.
        if memo.len() > 512 {
            memo.clear();
        }
        memo.insert((layer, t, c), histogram.clone());
        Ok(histogram)
    }
}

fn check(axis: Axis, index: u64, extent: u64) -> Result<(), ReadError> {
    if index >= extent {
        return Err(ReadError::OutOfBounds {
            axis,
            index,
            extent,
        });
    }
    Ok(())
}

fn copy_ortho_rows(
    brick: &Brick,
    axis: OrthoAxis,
    index: u64,
    cols: u64,
    element: usize,
    out: &mut [u8],
) -> Result<(), ReadError> {
    for zi in 0..brick.shape[0] {
        let z = brick.origin[0] + zi;
        match axis {
            OrthoAxis::XZ => {
                let start = [z, index, brick.origin[2]];
                let src = brick
                    .offset(start)
                    .ok_or(ReadError::Internal("brick row missing"))?;
                let run = brick.shape[2] as usize * element;
                let dst = (z * cols + brick.origin[2]) as usize * element;
                copy_run(out, dst, &brick.bytes, src, run)?;
            }
            OrthoAxis::YZ => {
                for yi in 0..brick.shape[1] {
                    let y = brick.origin[1] + yi;
                    let src = brick
                        .offset([z, y, index])
                        .ok_or(ReadError::Internal("brick column missing"))?;
                    let dst = (z * cols + y) as usize * element;
                    copy_run(out, dst, &brick.bytes, src, element)?;
                }
            }
        }
    }
    Ok(())
}

fn copy_volume_rows(
    brick: &Brick,
    dims: Dims,
    element: usize,
    out: &mut [u8],
) -> Result<(), ReadError> {
    let run = brick.shape[2] as usize * element;
    for zi in 0..brick.shape[0] {
        for yi in 0..brick.shape[1] {
            let (z, y) = (brick.origin[0] + zi, brick.origin[1] + yi);
            let src = brick
                .offset([z, y, brick.origin[2]])
                .ok_or(ReadError::Internal("brick row missing"))?;
            let dst = ((z * dims.y + y) * dims.x + brick.origin[2]) as usize * element;
            copy_run(out, dst, &brick.bytes, src, run)?;
        }
    }
    Ok(())
}

fn copy_run(
    out: &mut [u8],
    dst: usize,
    src_bytes: &[u8],
    src: usize,
    run: usize,
) -> Result<(), ReadError> {
    let target = out
        .get_mut(dst..dst + run)
        .ok_or(ReadError::Internal("assembled plane offset out of range"))?;
    let source = src_bytes
        .get(src..src + run)
        .ok_or(ReadError::Internal("brick offset out of range"))?;
    target.copy_from_slice(source);
    Ok(())
}
