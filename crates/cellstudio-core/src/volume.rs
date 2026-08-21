use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use serde::Serialize;
use zarrs::array::codec::ZstdCodec;
use zarrs::array::{Array, ArrayBuilder, ArrayBytes, ArraySubset, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::storage::{ReadableStorage, ReadableStorageTraits};

use crate::LayerId;
use crate::axes::{Dims, Dtype};
use crate::dataset::{Dataset, OpenError};
use crate::reader::{ImageReader, ReadError, Volume};

/// Directory name of the proxy store inside the project cache.
pub const PROXY_STORE_NAME: &str = "volume_proxy.zarr";

/// Attribute key carrying the pyramid level the proxy was built from.
const PROXY_ATTRIBUTE: &str = "cellstudio_proxy";

/// Per-timepoint GPU budget for the 3D view.
pub const DEFAULT_VOLUME_BUDGET_BYTES: u64 = 128 << 20;

#[derive(Serialize)]
pub struct ProxyStore {
    pub path: PathBuf,
    /// TCZYX of the proxy series; ZYX is the level's extent.
    pub dims: Dims,
    pub dtype: Dtype,
    pub level: u32,
    #[serde(skip)]
    array: Arc<Array<dyn ReadableStorageTraits>>,
}

impl std::fmt::Debug for ProxyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyStore")
            .field("path", &self.path)
            .field("dims", &self.dims)
            .field("dtype", &self.dtype)
            .field("level", &self.level)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error(transparent)]
    Dataset(#[from] OpenError),
    #[error("volume proxies require uint8 or uint16 data, found {0:?}")]
    UnsupportedDtype(Dtype),
    #[error("proxy store at {path} is not a uint16 TCZYX array: {reason}")]
    Malformed { path: PathBuf, reason: String },
    #[error("zarr write failed: {0}")]
    Zarr(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl ProxyStore {
    pub fn open(path: &Path) -> Result<Self, ProxyError> {
        let store: ReadableStorage =
            Arc::new(
                FilesystemStore::new(path).map_err(|e| ProxyError::Malformed {
                    path: path.to_path_buf(),
                    reason: e.to_string(),
                })?,
            );
        let array = Array::open(store, "/").map_err(|e| ProxyError::Malformed {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        if array.data_type() != &data_type::uint16() {
            return Err(ProxyError::Malformed {
                path: path.to_path_buf(),
                reason: format!("sample type is {}", array.data_type()),
            });
        }
        let shape = array.shape();
        let dims = match shape {
            [t, c, z, y, x] => Dims {
                t: *t,
                c: *c,
                z: *z,
                y: *y,
                x: *x,
            },
            other => {
                return Err(ProxyError::Malformed {
                    path: path.to_path_buf(),
                    reason: format!("shape has {} axes, expected 5", other.len()),
                });
            }
        };
        let level = array
            .attributes()
            .get(PROXY_ATTRIBUTE)
            .and_then(|v| v.get("level"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        Ok(Self {
            path: path.to_path_buf(),
            dims,
            dtype: Dtype::U16,
            level,
            array: Arc::new(array),
        })
    }

    /// One (t, c) volume, straight out of the proxy chunk.
    pub fn read_volume(&self, t: u64, c: u64) -> Result<Volume, ReadError> {
        let subset = ArraySubset::new_with_ranges(&[
            t..t + 1,
            c..c + 1,
            0..self.dims.z,
            0..self.dims.y,
            0..self.dims.x,
        ]);
        let bytes: ArrayBytes<'static> = self
            .array
            .retrieve_array_subset(&subset)
            .map_err(|e| ReadError::Zarr(e.to_string()))?;
        let bytes = bytes
            .into_fixed()
            .map_err(|e| ReadError::Decode(e.to_string()))?
            .into_owned();
        Ok(Volume {
            shape: [self.dims.z as u32, self.dims.y as u32, self.dims.x as u32],
            dtype: Dtype::U16,
            level: self.level,
            from_proxy: true,
            bytes: Bytes::from(bytes),
        })
    }
}

/// Finest level whose single-timepoint uint16 volume fits `budget_bytes`.
pub fn choose_proxy_level(dataset: &Dataset, budget_bytes: u64) -> u32 {
    let coarsest = dataset.coarsest_level();
    dataset
        .levels
        .iter()
        .find(|level| level.dims.zyx_voxels() * 2 <= budget_bytes)
        .map(|level| level.index)
        .unwrap_or(coarsest)
}

/// Build the proxy series for every (t, c) at `level`, writing into `out`.
/// `progress` is called on the calling thread with a 0.0–1.0 fraction.
pub fn build_proxy(
    reader: &ImageReader,
    level: u32,
    out: &Path,
    progress: &dyn Fn(f32),
) -> Result<ProxyStore, ProxyError> {
    let dataset = reader.dataset();
    let source_dtype = dataset.dtype;
    if source_dtype == Dtype::U32 {
        return Err(ProxyError::UnsupportedDtype(source_dtype));
    }
    let dims = dataset.level(level)?.dims;

    std::fs::create_dir_all(out).map_err(|e| ProxyError::Io {
        path: out.to_path_buf(),
        source: e,
    })?;
    let store = Arc::new(
        FilesystemStore::new(out).map_err(|e| ProxyError::Malformed {
            path: out.to_path_buf(),
            reason: e.to_string(),
        })?,
    );

    let mut builder = ArrayBuilder::new(
        vec![dims.t, dims.c, dims.z, dims.y, dims.x],
        vec![1, 1, dims.z, dims.y, dims.x],
        data_type::uint16(),
        0u16,
    );
    builder
        .bytes_to_bytes_codecs(vec![Arc::new(ZstdCodec::new(3, false))])
        .dimension_names(Some(["t", "c", "z", "y", "x"]))
        .attributes(
            serde_json::json!({
                PROXY_ATTRIBUTE: { "level": level, "source": dataset.root }
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        );
    let array = builder
        .build(store, "/")
        .map_err(|e| ProxyError::Zarr(e.to_string()))?;
    array
        .store_metadata()
        .map_err(|e| ProxyError::Zarr(e.to_string()))?;

    let total = (dims.t * dims.c).max(1);
    let mut done = 0_u64;
    progress(0.0);
    for t in 0..dims.t {
        for c in 0..dims.c {
            let volume = reader.read_volume(LayerId::Image, level, t, c)?;
            let elements = widen_to_u16(&volume.bytes, source_dtype);
            array
                .store_chunk(&[t, c, 0, 0, 0], elements)
                .map_err(|e| ProxyError::Zarr(e.to_string()))?;
            done += 1;
            progress(done as f32 / total as f32);
        }
    }
    ProxyStore::open(out)
}

/// uint8 samples widen without rescaling so display windows keep their meaning.
fn widen_to_u16(bytes: &[u8], dtype: Dtype) -> Vec<u16> {
    match dtype {
        Dtype::U8 => bytes.iter().map(|v| u16::from(*v)).collect(),
        _ => bytes
            .chunks_exact(2)
            .map(|b| u16::from_ne_bytes([b[0], b[1]]))
            .collect(),
    }
}
