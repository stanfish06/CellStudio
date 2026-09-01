mod support;

use std::sync::Arc;

use cellstudio_core::LayerId;
use cellstudio_core::axes::{Axis, Dims, Dtype, PhysicalScale};
use cellstudio_core::bricks::{BrickCache, BrickKey};
use cellstudio_core::dataset::{self, Dataset};
use cellstudio_core::labels::{
    ChunkKey, ContractError, MAX_LABEL_ID, StrokeMode, StrokeSpec, VoxelSet, apply, check_contract,
    clear_label, downsample, ensure_store, regenerate_coarse, restore, scan_inventory, scan_label,
    snapshot, stamp_voxels,
};
use cellstudio_core::reader::{ImageReader, OrthoAxis};
use serde_json::json;
use support::{Format, Spec};
use tempfile::TempDir;
use zarrs::array::codec::ZstdCodec;
use zarrs::array::{ArrayBuilder, ArraySubset, data_type};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::GroupBuilder;

fn image(levels: usize) -> (TempDir, Dataset) {
    image_sized(
        Dims {
            t: 2,
            c: 2,
            z: 6,
            y: 32,
            x: 32,
        },
        levels,
    )
}

fn image_sized(dims: Dims, levels: usize) -> (TempDir, Dataset) {
    let dir = TempDir::new().expect("tempdir");
    let data = support::build(
        Spec::new(Format::V3)
            .dims(dims)
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 8,
                x: 8,
            })
            .levels(levels),
    );
    let ds = dataset::open(&data.root).expect("open image");
    (dir, ds)
}

/// Every value of one level at one timepoint, row-major ZYX.
fn read_level(root: &std::path::Path, level: u32, t: u64) -> (Dims, Vec<u32>) {
    let ds = dataset::open(root).expect("open labels");
    let source = ds.source(level).expect("level");
    let dims = source.dims;
    let subset = ArraySubset::new_with_ranges(&[t..t + 1, 0..1, 0..dims.z, 0..dims.y, 0..dims.x]);
    let values = source
        .array
        .retrieve_array_subset::<Vec<u32>>(&subset)
        .expect("retrieve");
    (dims, values)
}

/// A label store written the way an importer or a conversion script would: our contract,
/// its own chunking, values placed directly.
fn foreign_store(root: &std::path::Path, image: &Dataset, chunks: Dims, values: &[u32]) {
    std::fs::create_dir_all(root).expect("mkdir");
    let store = Arc::new(FilesystemStore::new(root).expect("store"));
    let mut group = GroupBuilder::new()
        .build(store.clone(), "/")
        .expect("group");
    *group.attributes_mut() = json!({
        "ome": {
            "version": "0.5",
            "multiscales": [{
                "axes": [
                    { "name": "t", "type": "time" },
                    { "name": "c", "type": "channel" },
                    { "name": "z", "type": "space" },
                    { "name": "y", "type": "space" },
                    { "name": "x", "type": "space" },
                ],
                "datasets": image.levels.iter().map(|l| json!({ "path": l.index.to_string() })).collect::<Vec<_>>(),
            }],
        }
    })
    .as_object()
    .expect("object")
    .clone();
    group.store_metadata().expect("group metadata");

    for level in &image.levels {
        let dims = Dims { c: 1, ..level.dims };
        let chunks = Dims {
            t: 1,
            c: 1,
            z: chunks.z.clamp(1, dims.z),
            y: chunks.y.clamp(1, dims.y),
            x: chunks.x.clamp(1, dims.x),
        };
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
            .expect("array");
        array.store_metadata().expect("array metadata");
        if level.index == 0 {
            let subset =
                ArraySubset::new_with_ranges(&[0..1, 0..1, 0..dims.z, 0..dims.y, 0..dims.x]);
            array
                .store_array_subset(&subset, values.to_vec())
                .expect("store values");
        }
    }
}

#[test]
fn a_created_store_round_trips_and_passes_the_contract() {
    let (dir, ds) = image(2);
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");
    assert_eq!(store.level_count(), 2);
    assert_eq!(
        store.chunks(0).expect("chunks"),
        Dims {
            t: 1,
            c: 1,
            z: 4,
            y: 32,
            x: 32
        },
        "z clamps to min(4, Z) and y/x to the level extent"
    );

    let reopened = dataset::open(&root).expect("reopen");
    assert_eq!(reopened.dtype, Dtype::U32);
    assert_eq!(reopened.levels.len(), ds.levels.len());
    for (label, image) in reopened.levels.iter().zip(&ds.levels) {
        assert_eq!(label.dims.t, image.dims.t);
        assert_eq!(label.dims.c, 1);
        assert_eq!(
            (label.dims.z, label.dims.y, label.dims.x),
            (image.dims.z, image.dims.y, image.dims.x)
        );
    }
    check_contract(&reopened, &ds).expect("contract");
}

#[test]
fn creation_is_atomic_and_reopening_adopts_rather_than_recreates() {
    let (dir, ds) = image(2);
    let root = dir.path().join("labels.zarr");
    ensure_store(&root, &ds).expect("create");
    assert!(
        !dir.path().join(".labels.zarr.creating").exists(),
        "the temporary create directory is renamed into place, not left behind"
    );

    let store = ensure_store(&root, &ds).expect("adopt");
    let footprint = apply(
        &store,
        0,
        &StrokeSpec {
            mode: StrokeMode::Paint { label: 3 },
            radius: 3.0,
            plane: None,
            centres: vec![[3.0, 16.0, 16.0]],
        },
    )
    .expect("paint");
    assert!(!footprint.chunks.is_empty());

    let adopted = ensure_store(&root, &ds).expect("re-adopt");
    let row = scan_label(&adopted, 0, 3).expect("scan");
    assert_eq!(
        row.area, footprint.deltas[0].area as u64,
        "edits survive a reopen"
    );
}

#[test]
fn the_contract_rejects_the_wrong_dtype_and_geometry_but_not_the_chunking() {
    let (dir, ds) = image(2);

    let u16_store = support::build(Spec::new(Format::V3).levels(2));
    let u16_ds = dataset::open(&u16_store.root).expect("open u16");
    assert!(matches!(
        check_contract(&u16_ds, &ds),
        Err(ContractError::Dtype { found: Dtype::U16 })
    ));

    let short = dir.path().join("short.zarr");
    let (_short_dir, one_level) = image(1);
    foreign_store(
        &short,
        &one_level,
        Dims {
            t: 1,
            c: 1,
            z: 2,
            y: 8,
            x: 8,
        },
        &vec![0_u32; (one_level.dims.z * one_level.dims.y * one_level.dims.x) as usize],
    );
    let short_ds = dataset::open(&short).expect("open short");
    assert!(matches!(
        check_contract(&short_ds, &ds),
        Err(ContractError::LevelCount {
            found: 1,
            expected: 2
        })
    ));

    let odd = dir.path().join("odd-chunks.zarr");
    let level0 = ds.levels[0].dims;
    foreign_store(
        &odd,
        &ds,
        Dims {
            t: 1,
            c: 1,
            z: 1,
            y: 5,
            x: 7,
        },
        &vec![0_u32; (level0.z * level0.y * level0.x) as usize],
    );
    let odd_ds = dataset::open(&odd).expect("open odd");
    check_contract(&odd_ds, &ds).expect("chunking is a performance property, not a contract term");
    ensure_store(&odd, &ds).expect("a differently chunked store is adopted and editable");
}

#[test]
fn scan_label_matches_a_brute_force_count_on_a_foreign_store() {
    let (dir, ds) = image(1);
    let dims = ds.levels[0].dims;
    let mut values = vec![0_u32; (dims.z * dims.y * dims.x) as usize];
    for z in 1..4_u64 {
        for y in 6..11_u64 {
            for x in 3..9_u64 {
                values[((z * dims.y + y) * dims.x + x) as usize] = 42;
            }
        }
    }
    let root = dir.path().join("foreign.zarr");
    foreign_store(
        &root,
        &ds,
        Dims {
            t: 1,
            c: 1,
            z: 1,
            y: 5,
            x: 7,
        },
        &values,
    );

    let store = ensure_store(&root, &ds).expect("adopt");
    let row = scan_label(&store, 0, 42).expect("scan");
    let expected: Vec<[u64; 3]> = (0..dims.z)
        .flat_map(|z| (0..dims.y).flat_map(move |y| (0..dims.x).map(move |x| [z, y, x])))
        .filter(|v| values[((v[0] * dims.y + v[1]) * dims.x + v[2]) as usize] == 42)
        .collect();
    assert_eq!(row.area, expected.len() as u64);
    assert_eq!(row.sum_z, expected.iter().map(|v| v[0] as f64).sum::<f64>());
    assert_eq!(row.sum_x, expected.iter().map(|v| v[2] as f64).sum::<f64>());
    let b = row.bbox.expect("bbox");
    assert_eq!((b.z0, b.z1, b.y0, b.y1, b.x0, b.x1), (1, 3, 6, 10, 3, 8));
    assert_eq!(scan_label(&store, 1, 42).expect("other frame").area, 0);
}

#[test]
fn scan_inventory_collects_every_frame_label_pair_with_exact_extents() {
    let (dir, ds) = image(1);
    let dims = ds.levels[0].dims;
    let mut values = vec![0_u32; (dims.z * dims.y * dims.x) as usize];
    for z in 1..3_u64 {
        for y in 4..9_u64 {
            for x in 2..7_u64 {
                values[((z * dims.y + y) * dims.x + x) as usize] = 42;
            }
        }
    }
    for z in 3..5_u64 {
        for y in 20..25_u64 {
            for x in 10..15_u64 {
                values[((z * dims.y + y) * dims.x + x) as usize] = 7;
            }
        }
    }
    let root = dir.path().join("foreign.zarr");
    foreign_store(
        &root,
        &ds,
        Dims {
            t: 1,
            c: 1,
            z: 1,
            y: 5,
            x: 7,
        },
        &values,
    );
    let store = ensure_store(&root, &ds).expect("adopt");
    // a third label on the other frame, through the app's own write path
    apply(
        &store,
        1,
        &StrokeSpec {
            mode: StrokeMode::Paint { label: 9 },
            radius: 3.0,
            plane: None,
            centres: vec![[3.0, 16.0, 16.0]],
        },
    )
    .expect("paint");

    let progress = std::cell::RefCell::new(Vec::new());
    let inventory =
        scan_inventory(&store, &|fraction| progress.borrow_mut().push(fraction)).expect("scan");
    assert_eq!(
        inventory
            .rows
            .iter()
            .map(|row| (row.t, row.label))
            .collect::<Vec<_>>(),
        vec![(0, 7), (0, 42), (1, 9)],
        "exactly the (t, label) pairs present, sorted"
    );
    for row in &inventory.rows {
        assert_eq!(
            *row,
            scan_label(&store, row.t, row.label).expect("scan_label"),
            "the inventory row equals the bounded per-label scan"
        );
    }
    assert_eq!(inventory.max_id, 42);
    assert!(inventory.oversized.is_empty());
    assert!(inventory.multi_frame.is_empty());
    let progress = progress.into_inner();
    assert_eq!(progress.len() as u64, dims.t, "one callback per frame");
    assert_eq!(progress.last().copied(), Some(1.0));
}

#[test]
fn scan_inventory_flags_oversized_ids_and_an_id_on_two_frames() {
    let (dir, ds) = image(1);
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");
    let paint = |t: u64, label: u32, centre: [f64; 3]| {
        apply(
            &store,
            t,
            &StrokeSpec {
                mode: StrokeMode::Paint { label },
                radius: 3.0,
                plane: None,
                centres: vec![centre],
            },
        )
        .expect("paint");
    };
    let oversized = (MAX_LABEL_ID + 1) as u32;
    paint(0, 42, [3.0, 8.0, 8.0]);
    paint(1, 42, [3.0, 8.0, 8.0]);
    paint(1, oversized, [3.0, 24.0, 24.0]);

    let inventory = scan_inventory(&store, &|_| {}).expect("scan");
    assert_eq!(
        inventory
            .rows
            .iter()
            .map(|row| (row.t, row.label))
            .collect::<Vec<_>>(),
        vec![(0, 42), (1, 42), (1, oversized)],
        "flagged pairs are still reported"
    );
    assert_eq!(inventory.max_id, oversized);
    assert_eq!(inventory.oversized, vec![oversized]);
    assert_eq!(inventory.multi_frame, vec![42]);
}

#[test]
fn a_two_dimensional_stamp_touches_exactly_one_plane() {
    let (dir, ds) = image(1);
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");
    let spec = StrokeSpec {
        mode: StrokeMode::Paint { label: 9 },
        radius: 4.0,
        plane: Some((Axis::Y, 12)),
        centres: vec![[3.0, 12.5, 16.0]],
    };
    let footprint = apply(&store, 0, &spec).expect("paint");
    let bbox = footprint.bbox.expect("bbox");
    assert_eq!((bbox.y0, bbox.y1), (12, 12));

    let (dims, values) = read_level(&root, 0, 0);
    for z in 0..dims.z {
        for y in 0..dims.y {
            for x in 0..dims.x {
                let v = values[((z * dims.y + y) * dims.x + x) as usize];
                assert_eq!(v != 0, y == 12 && v == 9, "painted only y = 12");
            }
        }
    }
}

#[test]
fn erase_and_delete_report_the_voxels_they_took_back() {
    let (dir, ds) = image(1);
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");
    let paint = |label: u32, centre: [f64; 3]| StrokeSpec {
        mode: StrokeMode::Paint { label },
        radius: 3.0,
        plane: None,
        centres: vec![centre],
    };
    let a = apply(&store, 0, &paint(1, [3.0, 12.0, 12.0])).expect("paint 1");
    apply(&store, 0, &paint(2, [3.0, 20.0, 20.0])).expect("paint 2");

    // A scoped eraser over both blobs takes only label 1's voxels.
    let erased = apply(
        &store,
        0,
        &StrokeSpec {
            mode: StrokeMode::Erase { only: Some(1) },
            radius: 20.0,
            plane: None,
            centres: vec![[3.0, 16.0, 16.0]],
        },
    )
    .expect("erase");
    assert_eq!(erased.deltas.len(), 1);
    assert_eq!(erased.deltas[0].label, 1);
    assert_eq!(erased.deltas[0].area, -a.deltas[0].area);

    let row = scan_label(&store, 0, 2).expect("scan");
    let bbox = row.bbox.expect("bbox");
    let cleared = clear_label(&store, 0, 2, bbox).expect("delete");
    assert_eq!(cleared.deltas[0].area, -(row.area as i64));
    assert_eq!(scan_label(&store, 0, 2).expect("rescan").area, 0);
}

#[test]
fn restore_returns_every_chunk_to_its_prior_object_state() {
    // Wide enough that the two strokes below land in different 128x128 label chunks.
    let (dir, ds) = image_sized(
        Dims {
            t: 1,
            c: 1,
            z: 6,
            y: 300,
            x: 300,
        },
        1,
    );
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");

    let first = apply(
        &store,
        0,
        &StrokeSpec {
            mode: StrokeMode::Paint { label: 5 },
            radius: 3.0,
            plane: None,
            centres: vec![[3.0, 6.0, 6.0]],
        },
    )
    .expect("first stroke");

    // A far-away second stroke, so its chunks have no object at all beforehand.
    let second_spec = StrokeSpec {
        mode: StrokeMode::Paint { label: 6 },
        radius: 3.0,
        plane: None,
        centres: vec![[3.0, 280.0, 280.0]],
    };
    let level0 = ds.levels[0].dims;
    let second_set = second_spec.rasterize(ds.scale, [level0.z, level0.y, level0.x]);
    let affected: Vec<ChunkKey> = chunk_keys(&store.chunks(0).expect("chunks"), &second_set, 0);
    let before = snapshot(&store, &affected).expect("snapshot");
    assert!(
        before.iter().any(|s| !s.existed),
        "the first paint in a region has no prior object; that is the case restore must erase"
    );

    let second = apply(&store, 0, &second_spec).expect("second stroke");
    assert!(!second.chunks.is_empty());
    let after = snapshot(&store, &affected).expect("snapshot");
    assert_ne!(before, after);

    restore(&store, &before).expect("restore");
    assert_eq!(
        snapshot(&store, &affected).expect("snapshot"),
        before,
        "byte-identical, and an object that never existed is absent again"
    );
    let row = scan_label(&store, 0, 5).expect("scan");
    assert_eq!(
        row.area, first.deltas[0].area as u64,
        "the other stroke is untouched"
    );
    assert_eq!(scan_label(&store, 0, 6).expect("scan").area, 0);
}

fn chunk_keys(chunks: &Dims, set: &VoxelSet, t: u64) -> Vec<ChunkKey> {
    let mut keys: Vec<ChunkKey> = set
        .iter()
        .map(|v| ChunkKey {
            level: 0,
            t,
            grid: [v[0] / chunks.z, v[1] / chunks.y, v[2] / chunks.x],
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[test]
fn coarse_regeneration_agrees_with_downsampling_the_level_zero_set() {
    let (dir, ds) = image(2);
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");
    let spec = StrokeSpec {
        mode: StrokeMode::Paint { label: 7 },
        radius: 5.0,
        plane: None,
        centres: vec![[2.5, 15.5, 17.5]],
    };
    let footprint = apply(&store, 0, &spec).expect("paint");
    let changed = regenerate_coarse(&store, 0, footprint.bbox.expect("bbox")).expect("coarse");
    assert!(!changed.is_empty());
    assert!(changed.iter().all(|k| k.level == 1));

    let level0 = ds.levels[0].dims;
    let set = spec.rasterize(ds.scale, [level0.z, level0.y, level0.x]);
    let expected = downsample(&set, store.factor(1).expect("factor"));
    let (dims, values) = read_level(&root, 1, 0);
    let painted: Vec<[u64; 3]> = (0..dims.z)
        .flat_map(|z| (0..dims.y).flat_map(move |y| (0..dims.x).map(move |x| [z, y, x])))
        .filter(|v| values[((v[0] * dims.y + v[1]) * dims.x + v[2]) as usize] == 7)
        .collect();
    assert_eq!(painted, expected.iter().collect::<Vec<_>>());
}

#[test]
fn xy_ortho_planes_match_the_volume() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 2,
                c: 2,
                z: 6,
                y: 20,
                x: 12,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .levels(2),
    );
    let ds = dataset::open(&data.root).expect("open");
    let reader = ImageReader::new(Arc::new(ds), 1 << 24);
    for level in 0..2_u32 {
        let volume = reader
            .read_volume(LayerId::Image, level, 1, 1)
            .expect("volume");
        let [dz, dy, dx] = volume.shape.map(u64::from);
        for z in 0..dz {
            let plane = reader
                .read_ortho_plane(LayerId::Image, level, OrthoAxis::XY, 1, 1, z)
                .expect("xy plane");
            assert_eq!(plane.shape, [dy as u32, dx as u32]);
            let from_volume =
                &volume.bytes[((z * dy * dx) as usize * 2)..][..(dy * dx) as usize * 2];
            assert_eq!(&plane.bytes[..], from_volume);
        }
    }
}

#[test]
fn invalidating_a_key_drops_the_resident_brick() {
    let (dir, ds) = image(1);
    let root = dir.path().join("labels.zarr");
    let store = ensure_store(&root, &ds).expect("create");
    let reader = ImageReader::new(Arc::new(ds.clone()), 1 << 24);
    reader.register_layer(
        LayerId::Labels,
        Arc::new(store.open_readable().expect("open")),
    );

    let zyx = [3_u64, 6, 6];
    assert_eq!(
        reader
            .read_pixel(LayerId::Labels, 0, 0, zyx)
            .expect("pixel"),
        0
    );

    let footprint = apply(
        &store,
        0,
        &StrokeSpec {
            mode: StrokeMode::Paint { label: 11 },
            radius: 2.0,
            plane: None,
            centres: vec![[3.5, 6.5, 6.5]],
        },
    )
    .expect("paint");
    let keys: Vec<BrickKey> = footprint
        .chunks
        .iter()
        .map(|k| k.brick(LayerId::Labels))
        .collect();
    assert_eq!(
        reader
            .read_pixel(LayerId::Labels, 0, 0, zyx)
            .expect("pixel"),
        0,
        "the pre-edit brick is still resident until it is invalidated"
    );
    reader.bricks().invalidate(&keys);
    assert_eq!(
        reader
            .read_pixel(LayerId::Labels, 0, 0, zyx)
            .expect("pixel"),
        11
    );
}

/// A decode that read pre-edit bytes must not reach the cache or a waiter that claimed
/// after the write. The chunk is large so the decode is orders of magnitude longer than
/// the claim-to-invalidate window the spins below close.
#[test]
fn an_invalidated_decode_is_neither_published_nor_left_resident() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("slow.zarr");
    std::fs::create_dir_all(&root).expect("mkdir");
    let fs = Arc::new(FilesystemStore::new(&root).expect("store"));
    let shape = vec![1_u64, 1, 4, 2048, 2048];
    let mut builder = ArrayBuilder::new(shape.clone(), shape, data_type::uint32(), 0u32);
    builder.dimension_names(Some(["t", "c", "z", "y", "x"]));
    let array = builder.build(fs.clone(), "/").expect("array");
    array.store_metadata().expect("metadata");
    let values: Vec<u32> = (0..4_u32 * 2048 * 2048)
        .map(|i| i.wrapping_mul(2_654_435_761))
        .collect();
    array.store_chunk(&[0, 0, 0, 0, 0], values).expect("chunk");

    let cache = Arc::new(BrickCache::new(1 << 30));
    cache.register_layer(
        LayerId::Labels,
        Arc::new(dataset::open(&root).expect("open")),
    );
    let key = BrickKey {
        layer: LayerId::Labels,
        level: 0,
        t: 0,
        c: 0,
        grid: [0, 0, 0],
    };

    let owner = std::thread::spawn({
        let cache = cache.clone();
        move || cache.get(key).expect("owner decode")
    });
    spin(|| cache.stats().misses == 1, "the owner claims the key");
    let waiter = std::thread::spawn({
        let cache = cache.clone();
        move || cache.get(key).expect("waiter decode")
    });
    spin(
        || cache.stats().coalesced == 1,
        "the waiter joins the decode",
    );

    cache.invalidate(&[key]);
    let stale = owner.join().expect("owner");
    let fresh = waiter.join().expect("waiter");
    assert!(
        !Arc::ptr_eq(&stale, &fresh),
        "the waiter retried instead of receiving the invalidated decode"
    );
    let resident = cache.get(key).expect("post-commit read");
    assert!(
        !Arc::ptr_eq(&stale, &resident),
        "the stale brick was not left resident"
    );
    assert!(Arc::ptr_eq(&fresh, &resident));
}

fn spin(mut ready: impl FnMut() -> bool, what: &str) {
    let start = std::time::Instant::now();
    while !ready() {
        assert!(
            start.elapsed().as_secs() < 10,
            "timed out waiting until {what}"
        );
        std::hint::spin_loop();
    }
}

// -- the stamp contract -----------------------------------------------------------
//
// The same cases and expectations as `packages/viewer/src/edit/stamp.test.ts`. The hash is
// over the sorted voxel coordinates, so it catches an interior disagreement that a matching
// count and bounding box would hide. Change the stamp formula and both sides must be
// updated together — that is the contract.
/// A coarse level's expectation: the point-sampling factor, then count, bounds and hash.
type Coarse = ([u64; 3], u64, Option<[u64; 6]>, u32);

struct Case {
    name: &'static str,
    dims: [u64; 3],
    centre: [f64; 3],
    radius: f64,
    scale: Option<[f64; 3]>,
    plane: Option<(Axis, u64)>,
    count: u64,
    bounds: Option<[u64; 6]>,
    hash: u32,
    coarse: &'static [Coarse],
}

const CASES: &[Case] = &[
    Case {
        name: "fractional-centre",
        dims: [8, 32, 32],
        centre: [3.5, 12.25, 15.75],
        radius: 3.,
        scale: None,
        plane: None,
        count: 106,
        bounds: Some([1, 5, 9, 14, 13, 18]),
        hash: 737211305,
        coarse: &[([1, 2, 2], 29, Some([1, 5, 5, 7, 7, 9]), 3362287568)],
    },
    Case {
        name: "radius-one",
        dims: [8, 32, 32],
        centre: [4.5, 16.5, 16.5],
        radius: 1.,
        scale: None,
        plane: None,
        count: 7,
        bounds: Some([3, 5, 15, 17, 15, 17]),
        hash: 2679594327,
        coarse: &[([1, 2, 2], 3, Some([3, 5, 8, 8, 8, 8]), 113349655)],
    },
    Case {
        name: "centre-on-voxel-boundary",
        dims: [8, 32, 32],
        centre: [4., 16., 16.],
        radius: 2.5,
        scale: None,
        plane: None,
        count: 56,
        bounds: Some([2, 5, 14, 17, 14, 17]),
        hash: 226855493,
        coarse: &[([2, 2, 2], 7, Some([1, 2, 7, 8, 7, 8]), 2617904996)],
    },
    Case {
        name: "anisotropic",
        dims: [8, 32, 32],
        centre: [4.5, 16.5, 16.5],
        radius: 6.,
        scale: Some([2., 0.6, 0.6]),
        plane: None,
        count: 251,
        bounds: Some([3, 5, 10, 22, 10, 22]),
        hash: 2381416567,
        coarse: &[
            ([1, 3, 3], 29, Some([3, 5, 4, 7, 4, 7]), 3448608705),
            ([1, 2, 2], 71, Some([3, 5, 5, 11, 5, 11]), 1727368983),
        ],
    },
    Case {
        name: "plane-z",
        dims: [8, 32, 32],
        centre: [4.5, 16.5, 16.5],
        radius: 5.,
        scale: Some([2., 0.6, 0.6]),
        plane: Some((Axis::Z, 4)),
        count: 81,
        bounds: Some([4, 4, 11, 21, 11, 21]),
        hash: 2295155377,
        coarse: &[([1, 2, 2], 21, Some([4, 4, 6, 10, 6, 10]), 3553274961)],
    },
    Case {
        name: "plane-y",
        dims: [8, 32, 32],
        centre: [4.5, 16.5, 16.5],
        radius: 5.,
        scale: Some([2., 0.6, 0.6]),
        plane: Some((Axis::Y, 16)),
        count: 25,
        bounds: Some([3, 5, 16, 16, 11, 21]),
        hash: 3253904173,
        coarse: &[([1, 2, 2], 11, Some([3, 5, 8, 8, 6, 10]), 69718805)],
    },
    Case {
        name: "plane-x",
        dims: [8, 32, 32],
        centre: [4.5, 16.5, 16.5],
        radius: 5.,
        scale: Some([2., 0.6, 0.6]),
        plane: Some((Axis::X, 16)),
        count: 25,
        bounds: Some([3, 5, 11, 21, 16, 16]),
        hash: 1557389101,
        coarse: &[([1, 2, 2], 11, Some([3, 5, 6, 10, 8, 8]), 1545221237)],
    },
    Case {
        name: "plane-z-off-slice",
        dims: [8, 32, 32],
        centre: [1.5, 16.5, 16.5],
        radius: 5.,
        scale: Some([2., 0.6, 0.6]),
        plane: Some((Axis::Z, 3)),
        count: 0,
        bounds: None,
        hash: 2166136261,
        coarse: &[],
    },
    Case {
        name: "clip-z-low",
        dims: [6, 10, 10],
        centre: [0.5, 5., 5.],
        radius: 3.,
        scale: None,
        plane: None,
        count: 72,
        bounds: Some([0, 2, 2, 7, 2, 7]),
        hash: 3193864357,
        coarse: &[([2, 2, 2], 12, Some([0, 1, 1, 3, 1, 3]), 3687468773)],
    },
    Case {
        name: "clip-z-high",
        dims: [6, 10, 10],
        centre: [5.5, 5., 5.],
        radius: 3.,
        scale: None,
        plane: None,
        count: 72,
        bounds: Some([3, 5, 2, 7, 2, 7]),
        hash: 16402725,
        coarse: &[([2, 2, 2], 6, Some([2, 2, 1, 3, 1, 3]), 3596338197)],
    },
    Case {
        name: "clip-y-low",
        dims: [6, 10, 10],
        centre: [3., 0.5, 5.],
        radius: 3.,
        scale: None,
        plane: None,
        count: 72,
        bounds: Some([0, 5, 0, 2, 2, 7]),
        hash: 342693957,
        coarse: &[([2, 2, 2], 12, Some([0, 2, 0, 1, 1, 3]), 2936230215)],
    },
    Case {
        name: "clip-y-high",
        dims: [6, 10, 10],
        centre: [3., 9.5, 5.],
        radius: 3.,
        scale: None,
        plane: None,
        count: 72,
        bounds: Some([0, 5, 7, 9, 2, 7]),
        hash: 2383361989,
        coarse: &[([2, 2, 2], 6, Some([0, 2, 4, 4, 1, 3]), 2412400535)],
    },
    Case {
        name: "clip-x-low",
        dims: [6, 10, 10],
        centre: [3., 5., 0.5],
        radius: 3.,
        scale: None,
        plane: None,
        count: 72,
        bounds: Some([0, 5, 2, 7, 0, 2]),
        hash: 2272845989,
        coarse: &[([2, 2, 2], 12, Some([0, 2, 1, 3, 0, 1]), 3929492151)],
    },
    Case {
        name: "clip-x-high",
        dims: [6, 10, 10],
        centre: [3., 5., 9.5],
        radius: 3.,
        scale: None,
        plane: None,
        count: 72,
        bounds: Some([0, 5, 2, 7, 7, 9]),
        hash: 1922683365,
        coarse: &[([2, 2, 2], 6, Some([0, 2, 1, 3, 4, 4]), 3989161991)],
    },
    Case {
        name: "clip-every-face",
        dims: [6, 10, 10],
        centre: [3., 5., 5.],
        radius: 20.,
        scale: None,
        plane: None,
        count: 600,
        bounds: Some([0, 5, 0, 9, 0, 9]),
        hash: 3729080933,
        coarse: &[([3, 3, 3], 32, Some([0, 1, 0, 3, 0, 3]), 1556374725)],
    },
    Case {
        name: "entirely-outside",
        dims: [6, 10, 10],
        centre: [-8., 5., 5.],
        radius: 3.,
        scale: None,
        plane: None,
        count: 0,
        bounds: None,
        hash: 2166136261,
        coarse: &[],
    },
    Case {
        name: "dev-dataset-orb",
        dims: [45, 512, 512],
        centre: [22.5, 256.5, 256.5],
        radius: 40.,
        scale: Some([2., 0.60296875, 0.6029296875]),
        plane: None,
        count: 80671,
        bounds: Some([10, 34, 217, 295, 216, 296]),
        hash: 848347560,
        coarse: &[
            (
                [1, 2, 2],
                20143,
                Some([10, 34, 109, 147, 108, 148]),
                1298913839,
            ),
            ([2, 4, 4], 2495, Some([5, 17, 55, 73, 54, 74]), 1795790636),
            ([1, 3, 3], 8957, Some([10, 34, 73, 98, 73, 98]), 2526211307),
        ],
    },
    Case {
        name: "dev-dataset-disk",
        dims: [45, 512, 512],
        centre: [22.5, 256.25, 255.75],
        radius: 60.,
        scale: Some([2., 0.60296875, 0.6029296875]),
        plane: Some((Axis::Z, 22)),
        count: 11311,
        bounds: Some([22, 22, 196, 315, 196, 315]),
        hash: 4090540587,
        coarse: &[([1, 2, 2], 2825, Some([22, 22, 98, 157, 98, 157]), 886965554)],
    },
];

fn hash(set: &VoxelSet) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for v in set.iter() {
        for value in v {
            for shift in [0, 8, 16, 24] {
                h ^= ((value >> shift) & 0xff) as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
    }
    h
}

fn assert_matches(set: &VoxelSet, count: u64, bounds: Option<[u64; 6]>, want: u32, what: &str) {
    assert_eq!(set.len(), count, "count: {what}");
    let got = set.bounds().map(|b| [b.z0, b.z1, b.y0, b.y1, b.x0, b.x1]);
    assert_eq!(got, bounds, "bounds: {what}");
    assert_eq!(hash(set), want, "hash: {what}");
}

#[test]
fn stamp_and_downsample_match_the_stamp_contract() {
    assert!(CASES.len() >= 15);
    for case in CASES {
        let scale = case.scale.map(|[z, y, x]| PhysicalScale { z, y, x });
        let set = stamp_voxels(case.centre, case.radius, scale, case.plane, case.dims);
        assert_matches(&set, case.count, case.bounds, case.hash, case.name);
        for (factor, count, bounds, want) in case.coarse {
            assert_matches(
                &downsample(&set, *factor),
                *count,
                *bounds,
                *want,
                &format!("{} at {factor:?}", case.name),
            );
        }
    }
}
