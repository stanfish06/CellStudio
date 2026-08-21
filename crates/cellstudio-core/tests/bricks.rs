//! Brick cache: byte-bounded LRU, brick geometry, and single-decode dedup for concurrent gets.

mod support;

use std::sync::{Arc, Barrier};

use cellstudio_core::axes::{Axis, Dims};
use cellstudio_core::bricks::{BrickCache, BrickKey};
use cellstudio_core::dataset;
use cellstudio_core::reader::ReadError;
use cellstudio_core::{Dtype, LayerId};
use support::{Format, Spec};

fn cache(data: &support::Data, capacity: usize) -> Arc<BrickCache> {
    let dataset = Arc::new(dataset::open(&data.root).expect("opens"));
    let cache = Arc::new(BrickCache::new(capacity));
    cache.register_layer(LayerId::Image, dataset);
    cache
}

fn key(t: u64, c: u64, grid: [u64; 3]) -> BrickKey {
    BrickKey {
        layer: LayerId::Image,
        level: 0,
        t,
        c,
        grid,
    }
}

#[test]
fn concurrent_gets_for_one_key_decode_once() {
    // Compressed 1 MB chunks give the decode real work, so the threads genuinely
    // overlap; the assertion holds either way.
    let data = support::build(
        Spec::new(Format::V3)
            .compress()
            .dims(Dims {
                t: 2,
                c: 1,
                z: 32,
                y: 128,
                x: 128,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 32,
                y: 128,
                x: 128,
            })
            .levels(1),
    );
    let cache = cache(&data, 64 << 20);
    let threads = 16;
    let barrier = Arc::new(Barrier::new(threads));

    let bricks: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (cache, barrier) = (cache.clone(), barrier.clone());
                scope.spawn(move || {
                    barrier.wait();
                    cache.get(key(0, 0, [0, 0, 0])).expect("brick")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("thread"))
            .collect()
    });

    let stats = cache.stats();
    assert_eq!(
        stats.decodes, 1,
        "the wait-map must coalesce {threads} gets into one decode"
    );
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.hits + stats.coalesced, (threads - 1) as u64);
    for brick in &bricks {
        assert!(
            Arc::ptr_eq(brick, &bricks[0]),
            "every caller shares the one decoded brick"
        );
    }
    assert_eq!(bricks[0].shape, [32, 128, 128]);
}

#[test]
fn distinct_keys_decode_independently() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 2,
                c: 2,
                z: 4,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 4,
                x: 4,
            })
            .levels(1),
    );
    let cache = cache(&data, 64 << 20);

    let keys: Vec<BrickKey> = (0..2)
        .flat_map(|gz| (0..2).flat_map(move |gy| (0..2).map(move |gx| key(0, 0, [gz, gy, gx]))))
        .collect();
    let bricks = cache.get_many(&keys).expect("bricks");
    assert_eq!(bricks.len(), 8);
    assert_eq!(cache.stats().decodes, 8);

    // A second pass is served entirely from the LRU.
    let again = cache.get_many(&keys).expect("bricks");
    assert_eq!(cache.stats().decodes, 8);
    for (a, b) in bricks.iter().zip(&again) {
        assert!(Arc::ptr_eq(a, b));
    }
}

#[test]
fn repeated_keys_in_one_batch_decode_once() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .levels(1),
    );
    let cache = cache(&data, 64 << 20);
    let keys = vec![key(0, 0, [0, 0, 0]); 5];
    let bricks = cache.get_many(&keys).expect("bricks");
    assert_eq!(bricks.len(), 5);
    assert_eq!(cache.stats().decodes, 1);
}

#[test]
fn cache_stays_inside_its_byte_budget() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 4,
                c: 1,
                z: 4,
                y: 16,
                x: 16,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .levels(1),
    );
    // One brick is 4*8*8*2 = 512 bytes; the budget holds three.
    let cache = cache(&data, 1536);
    for t in 0..4 {
        for gy in 0..2 {
            for gx in 0..2 {
                cache.get(key(t, 0, [0, gy, gx])).expect("brick");
            }
        }
    }
    let stats = cache.stats();
    assert!(stats.bytes <= stats.capacity_bytes, "{stats:?}");
    assert_eq!(stats.entries, 3);
    assert_eq!(stats.evictions, 13);
    assert_eq!(stats.decodes, 16);
}

#[test]
fn a_single_oversized_brick_stays_resident() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .levels(1),
    );
    let cache = cache(&data, 16);
    let brick = cache.get(key(0, 0, [0, 0, 0])).expect("brick");
    assert!(brick.len_bytes() > 16);
    assert_eq!(
        cache.stats().entries,
        1,
        "evicting the only entry would guarantee a re-decode"
    );
}

#[test]
fn edge_bricks_are_clipped_to_the_level() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 1,
                c: 1,
                z: 3,
                y: 10,
                x: 10,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 4,
                x: 4,
            })
            .levels(1),
    );
    let cache = cache(&data, 1 << 20);

    let last = cache.get(key(0, 0, [1, 2, 2])).expect("brick");
    assert_eq!(last.origin, [2, 8, 8]);
    assert_eq!(
        last.shape,
        [1, 2, 2],
        "the trailing brick covers only what exists"
    );
    assert_eq!(last.len_bytes(), 4 * 2);
    assert_eq!(last.dtype, Dtype::U16);
    assert_eq!(
        last.value([2, 9, 9]),
        Some(u64::from(data.at(0, 0, 0, 2, 9, 9)))
    );
    assert_eq!(last.value([0, 0, 0]), None, "outside the brick");
}

#[test]
fn out_of_range_keys_name_the_axis() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 2,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 8,
                x: 8,
            })
            .levels(1),
    );
    let cache = cache(&data, 1 << 20);

    match cache.get(key(9, 0, [0, 0, 0])) {
        Err(ReadError::OutOfBounds {
            axis,
            index,
            extent,
        }) => {
            assert_eq!((axis, index, extent), (Axis::T, 9, 2));
        }
        other => panic!("expected an OutOfBounds error, got {other:?}"),
    }
    match cache.get(key(0, 0, [7, 0, 0])) {
        Err(ReadError::OutOfBounds { axis, .. }) => assert_eq!(axis, Axis::Z),
        other => panic!("expected an OutOfBounds error, got {other:?}"),
    }
}

#[test]
fn unregistered_layers_are_rejected() {
    let data = support::build(Spec::new(Format::V3).levels(1));
    let cache = cache(&data, 1 << 20);
    let key = BrickKey {
        layer: LayerId::Labels,
        level: 0,
        t: 0,
        c: 0,
        grid: [0, 0, 0],
    };
    assert!(matches!(
        cache.get(key),
        Err(ReadError::UnknownLayer(LayerId::Labels))
    ));
}
