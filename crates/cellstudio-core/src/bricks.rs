use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use lru::LruCache;
use parking_lot::{Condvar, Mutex, RwLock};
use rayon::prelude::*;
use zarrs::array::{ArrayBytes, ArraySubset, CodecOptions};

use crate::LayerId;
use crate::axes::{Axis, Dims, Dtype};
use crate::dataset::Dataset;
use crate::reader::ReadError;

pub const DEFAULT_CAPACITY_BYTES: usize = 2 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct BrickKey {
    pub layer: LayerId,
    pub level: u32,
    pub t: u64,
    pub c: u64,
    pub grid: [u64; 3],
}

#[derive(Debug)]
pub struct Brick {
    pub key: BrickKey,
    /// First voxel of the brick in level coordinates, `[z, y, x]`.
    pub origin: [u64; 3],
    /// Extent clipped to the level's dims, `[z, y, x]`.
    pub shape: [u64; 3],
    pub dtype: Dtype,
    pub bytes: Bytes,
}

impl Brick {
    pub fn len_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn contains(&self, zyx: [u64; 3]) -> bool {
        (0..3).all(|i| zyx[i] >= self.origin[i] && zyx[i] < self.origin[i] + self.shape[i])
    }

    /// Byte offset of the run starting at absolute `[z, y, x_from]`.
    pub fn offset(&self, zyx: [u64; 3]) -> Option<usize> {
        if !self.contains(zyx) {
            return None;
        }
        let local = [
            zyx[0] - self.origin[0],
            zyx[1] - self.origin[1],
            zyx[2] - self.origin[2],
        ];
        let index = (local[0] * self.shape[1] + local[1]) * self.shape[2] + local[2];
        Some(index as usize * self.dtype.size_bytes())
    }

    /// Sample at absolute level coordinates, widened to u64.
    pub fn value(&self, zyx: [u64; 3]) -> Option<u64> {
        let offset = self.offset(zyx)?;
        read_sample(&self.bytes, offset, self.dtype)
    }
}

pub fn read_sample(bytes: &[u8], offset: usize, dtype: Dtype) -> Option<u64> {
    match dtype {
        Dtype::U8 => bytes.get(offset).map(|v| u64::from(*v)),
        Dtype::U16 => bytes
            .get(offset..offset + 2)
            .and_then(|b| <[u8; 2]>::try_from(b).ok())
            .map(|b| u64::from(u16::from_ne_bytes(b))),
        Dtype::U32 => bytes
            .get(offset..offset + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(|b| u64::from(u32::from_ne_bytes(b))),
    }
}

#[derive(Debug, Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
    decodes: AtomicU64,
    coalesced: AtomicU64,
    evictions: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct BrickStats {
    pub hits: u64,
    pub misses: u64,
    pub decodes: u64,
    pub coalesced: u64,
    pub evictions: u64,
    pub entries: u64,
    pub bytes: u64,
    pub capacity_bytes: u64,
}

struct Resident {
    lru: LruCache<BrickKey, Arc<Brick>>,
    bytes: usize,
}

/// A decode another thread is running; waiters block on `ready`.
#[derive(Default)]
struct Pending {
    ready: Condvar,
    slot: Mutex<Option<Result<Arc<Brick>, String>>>,
}

impl Pending {
    fn wait(&self) -> Result<Arc<Brick>, ReadError> {
        let mut slot = self.slot.lock();
        let result = loop {
            if let Some(result) = slot.as_ref() {
                break result.clone();
            }
            self.ready.wait(&mut slot);
        };
        result.map_err(ReadError::Decode)
    }

    fn publish(&self, result: Result<Arc<Brick>, String>) {
        *self.slot.lock() = Some(result);
        self.ready.notify_all();
    }
}

pub struct BrickCache {
    capacity_bytes: usize,
    resident: Mutex<Resident>,
    inflight: Mutex<HashMap<BrickKey, Arc<Pending>>>,
    layers: RwLock<HashMap<LayerId, Arc<Dataset>>>,
    counters: Counters,
    codec_options: CodecOptions,
}

impl std::fmt::Debug for BrickCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrickCache")
            .field("stats", &self.stats())
            .finish()
    }
}

impl BrickCache {
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes: capacity_bytes.max(1),
            resident: Mutex::new(Resident {
                lru: LruCache::unbounded(),
                bytes: 0,
            }),
            inflight: Mutex::new(HashMap::new()),
            layers: RwLock::new(HashMap::new()),
            counters: Counters::default(),
            codec_options: CodecOptions::default().with_concurrent_target(1),
        }
    }

    pub fn register_layer(&self, layer: LayerId, dataset: Arc<Dataset>) {
        self.layers.write().insert(layer, dataset);
    }

    pub fn layer(&self, layer: LayerId) -> Option<Arc<Dataset>> {
        self.layers.read().get(&layer).cloned()
    }

    pub fn stats(&self) -> BrickStats {
        let resident = self.resident.lock();
        BrickStats {
            hits: self.counters.hits.load(Ordering::Relaxed),
            misses: self.counters.misses.load(Ordering::Relaxed),
            decodes: self.counters.decodes.load(Ordering::Relaxed),
            coalesced: self.counters.coalesced.load(Ordering::Relaxed),
            evictions: self.counters.evictions.load(Ordering::Relaxed),
            entries: resident.lru.len() as u64,
            bytes: resident.bytes as u64,
            capacity_bytes: self.capacity_bytes as u64,
        }
    }

    pub fn clear(&self) {
        let mut resident = self.resident.lock();
        resident.lru.clear();
        resident.bytes = 0;
    }

    /// Brick-grid extent of a level, `[z, y, x]`.
    pub fn grid_shape(dims: Dims, chunks: Dims) -> [u64; 3] {
        [
            dims.z.div_ceil(chunks.z.max(1)).max(1),
            dims.y.div_ceil(chunks.y.max(1)).max(1),
            dims.x.div_ceil(chunks.x.max(1)).max(1),
        ]
    }

    pub fn get(&self, key: BrickKey) -> Result<Arc<Brick>, ReadError> {
        match self.claim(&key) {
            Claim::Resident(brick) => {
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
                Ok(brick)
            }
            Claim::Waiting(pending) => {
                self.counters.coalesced.fetch_add(1, Ordering::Relaxed);
                pending.wait()
            }
            Claim::Owner(pending) => {
                self.counters.misses.fetch_add(1, Ordering::Relaxed);
                let mut guard = Publisher {
                    cache: self,
                    key,
                    pending,
                    done: false,
                };
                let decoded = self.decode(&key);
                guard.settle(decoded)
            }
        }
    }

    pub fn get_many(&self, keys: &[BrickKey]) -> Result<Vec<Arc<Brick>>, ReadError> {
        let mut unique: Vec<BrickKey> = keys.to_vec();
        unique.sort_unstable_by_key(|k| (k.level, k.t, k.c, k.grid));
        unique.dedup();
        let decoded: Vec<(BrickKey, Arc<Brick>)> = unique
            .par_iter()
            .map(|k| self.get(*k).map(|b| (*k, b)))
            .collect::<Result<Vec<_>, ReadError>>()?;
        let by_key: HashMap<BrickKey, Arc<Brick>> = decoded.into_iter().collect();
        keys.iter()
            .map(|k| {
                by_key
                    .get(k)
                    .cloned()
                    .ok_or(ReadError::Internal("brick lost from batch"))
            })
            .collect()
    }

    fn claim(&self, key: &BrickKey) -> Claim {
        let mut inflight = self.inflight.lock();
        if let Some(brick) = self.resident.lock().lru.get(key).cloned() {
            return Claim::Resident(brick);
        }
        match inflight.get(key) {
            Some(pending) => Claim::Waiting(pending.clone()),
            None => {
                let pending = Arc::new(Pending::default());
                inflight.insert(*key, pending.clone());
                Claim::Owner(pending)
            }
        }
    }

    fn insert(&self, brick: Arc<Brick>) {
        let size = brick.len_bytes();
        let mut resident = self.resident.lock();
        if let Some(previous) = resident.lru.put(brick.key, brick) {
            resident.bytes = resident.bytes.saturating_sub(previous.len_bytes());
        }
        resident.bytes += size;
        while resident.bytes > self.capacity_bytes && resident.lru.len() > 1 {
            match resident.lru.pop_lru() {
                Some((_, evicted)) => {
                    resident.bytes = resident.bytes.saturating_sub(evicted.len_bytes());
                    self.counters.evictions.fetch_add(1, Ordering::Relaxed);
                }
                None => break,
            }
        }
    }

    fn decode(&self, key: &BrickKey) -> Result<Arc<Brick>, ReadError> {
        let dataset = self
            .layers
            .read()
            .get(&key.layer)
            .cloned()
            .ok_or(ReadError::UnknownLayer(key.layer))?;
        let source = dataset.source(key.level)?;
        let dims = source.dims;
        let chunks = source.chunks;
        bounds_check(Axis::T, key.t, dims.t)?;
        bounds_check(Axis::C, key.c, dims.c)?;

        let grid = Self::grid_shape(dims, chunks);
        for (i, axis) in [Axis::Z, Axis::Y, Axis::X].into_iter().enumerate() {
            bounds_check(axis, key.grid[i], grid[i])?;
        }
        let origin = [
            key.grid[0] * chunks.z,
            key.grid[1] * chunks.y,
            key.grid[2] * chunks.x,
        ];
        let shape = [
            chunks.z.min(dims.z - origin[0]),
            chunks.y.min(dims.y - origin[1]),
            chunks.x.min(dims.x - origin[2]),
        ];
        let region = [
            key.t..key.t + 1,
            key.c..key.c + 1,
            origin[0]..origin[0] + shape[0],
            origin[1]..origin[1] + shape[1],
            origin[2]..origin[2] + shape[2],
        ];
        let subset = ArraySubset::new_with_ranges(&source.map.project(&region));
        let bytes: ArrayBytes<'static> = source
            .array
            .retrieve_array_subset_opt(&subset, &self.codec_options)
            .map_err(|e| ReadError::Zarr(e.to_string()))?;
        let bytes = bytes
            .into_fixed()
            .map_err(|e| ReadError::Decode(e.to_string()))?
            .into_owned();
        self.counters.decodes.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(Brick {
            key: *key,
            origin,
            shape,
            dtype: dataset.dtype,
            bytes: Bytes::from(bytes),
        }))
    }
}

fn bounds_check(axis: Axis, index: u64, extent: u64) -> Result<(), ReadError> {
    if index >= extent {
        return Err(ReadError::OutOfBounds {
            axis,
            index,
            extent,
        });
    }
    Ok(())
}

enum Claim {
    Resident(Arc<Brick>),
    Waiting(Arc<Pending>),
    Owner(Arc<Pending>),
}

struct Publisher<'a> {
    cache: &'a BrickCache,
    key: BrickKey,
    pending: Arc<Pending>,
    done: bool,
}

impl Publisher<'_> {
    fn settle(&mut self, result: Result<Arc<Brick>, ReadError>) -> Result<Arc<Brick>, ReadError> {
        match &result {
            Ok(brick) => {
                self.cache.insert(brick.clone());
                self.release(Ok(brick.clone()));
            }
            Err(e) => self.release(Err(e.to_string())),
        }
        result
    }

    fn release(&mut self, result: Result<Arc<Brick>, String>) {
        self.done = true;
        self.cache.inflight.lock().remove(&self.key);
        self.pending.publish(result);
    }
}

impl Drop for Publisher<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.release(Err("brick decode panicked".to_string()));
        }
    }
}
