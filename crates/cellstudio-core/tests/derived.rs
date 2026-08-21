//! Volume-proxy build and the re-chunked brick working copy, leaving the source store byte-identical.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cellstudio_core::axes::{Dims, Dtype, Orientation};
use cellstudio_core::reader::{ImageReader, OrthoAxis};
use cellstudio_core::{LayerId, dataset, rechunk, volume};
use parking_lot::Mutex;
use support::{Format, Spec, as_u16};

/// Collects progress values so a test can assert they are monotonic and reach 1.0.
#[derive(Default)]
struct Progress {
    seen: Mutex<Vec<f32>>,
    calls: AtomicU32,
}

impl Progress {
    fn record(&self) -> impl Fn(f32) + '_ {
        move |fraction| {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.seen.lock().push(fraction);
        }
    }

    fn assert_monotonic_to_completion(&self) {
        let seen = self.seen.lock();
        assert!(
            self.calls.load(Ordering::Relaxed) > 1,
            "progress must be reported: {seen:?}"
        );
        assert_eq!(seen.first().copied(), Some(0.0));
        assert_eq!(seen.last().copied(), Some(1.0));
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]),
            "progress went backwards: {seen:?}"
        );
    }
}

fn reader(data: &support::Data) -> ImageReader {
    let dataset = Arc::new(dataset::open(&data.root).expect("opens"));
    ImageReader::new(dataset, 8 << 20)
}

#[test]
fn proxy_build_serves_volumes_and_leaves_the_source_untouched() {
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 3,
                c: 2,
                z: 4,
                y: 16,
                x: 16,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 8,
                x: 8,
            })
            .levels(2),
    );
    let reader = reader(&data);
    let out = tempfile::TempDir::new().expect("tempdir");
    let proxy_path = out.path().join(volume::PROXY_STORE_NAME);
    let before = support::store_digest(&data.root);

    // Before the job runs, volumes still come back: assembled from the pyramid.
    let pyramid = reader
        .read_volume(LayerId::Image, 1, 2, 1)
        .expect("pyramid volume");
    assert!(!pyramid.from_proxy);
    assert_eq!(reader.proxy_level(), None);

    let progress = Progress::default();
    let proxy = volume::build_proxy(&reader, 1, &proxy_path, &progress.record()).expect("proxy");
    progress.assert_monotonic_to_completion();
    assert_eq!(
        progress.calls.load(Ordering::Relaxed),
        1 + 3 * 2,
        "one report per (t, c)"
    );

    assert_eq!(proxy.level, 1);
    assert_eq!(proxy.dtype, Dtype::U16);
    assert_eq!(
        proxy.dims,
        Dims {
            t: 3,
            c: 2,
            z: 4,
            y: 8,
            x: 8
        }
    );
    assert_eq!(proxy.path, proxy_path);

    reader.attach_proxy(proxy);
    assert_eq!(reader.proxy_level(), Some(1));
    let served = reader
        .read_volume(LayerId::Image, 1, 2, 1)
        .expect("proxy volume");
    assert!(
        served.from_proxy,
        "requests for the proxy level are served from the proxy"
    );
    assert_eq!(served.shape, pyramid.shape);
    assert_eq!(served.bytes, pyramid.bytes);
    assert_eq!(as_u16(&served.bytes), data.volume(1, 2, 1));

    // Other levels keep coming from the pyramid.
    let other = reader
        .read_volume(LayerId::Image, 0, 2, 1)
        .expect("level 0 volume");
    assert!(!other.from_proxy);
    assert_eq!(as_u16(&other.bytes), data.volume(0, 2, 1));

    assert_eq!(
        support::store_digest(&data.root),
        before,
        "the source store must be untouched"
    );
}

#[test]
fn a_reopened_proxy_reports_its_level() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 2,
                c: 1,
                z: 2,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 8,
                x: 8,
            })
            .levels(2),
    );
    let reader = reader(&data);
    let out = tempfile::TempDir::new().expect("tempdir");
    let path = out.path().join(volume::PROXY_STORE_NAME);
    volume::build_proxy(&reader, 1, &path, &|_| {}).expect("proxy");

    let reopened = volume::ProxyStore::open(&path).expect("reopens");
    assert_eq!(reopened.level, 1);
    assert_eq!(
        reopened.dims,
        Dims {
            t: 2,
            c: 1,
            z: 2,
            y: 4,
            x: 4
        }
    );
    let volume = reopened.read_volume(1, 0).expect("volume");
    assert_eq!(as_u16(&volume.bytes), data.volume(1, 1, 0));
}

#[test]
fn proxy_level_fits_the_gpu_budget() {
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 64,
                x: 64,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 64,
                x: 64,
            })
            .levels(3),
    );
    let dataset = dataset::open(&data.root).expect("opens");

    // Level 0 is 4*64*64*2 = 32 KiB, level 1 is 8 KiB, level 2 is 2 KiB.
    assert_eq!(volume::choose_proxy_level(&dataset, 64 << 10), 0);
    assert_eq!(volume::choose_proxy_level(&dataset, 16 << 10), 1);
    assert_eq!(volume::choose_proxy_level(&dataset, 4 << 10), 2);
    assert_eq!(
        volume::choose_proxy_level(&dataset, 1),
        2,
        "the coarsest level is the floor"
    );
}

#[test]
fn rechunk_writes_a_working_copy_with_identical_pixels() {
    // Plane-chunked source: hostile for ortho views, which is what re-chunking fixes.
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 2,
                c: 2,
                z: 16,
                y: 16,
                x: 16,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 1,
                y: 16,
                x: 16,
            })
            .levels(2)
            .omero(true),
    );
    let source = dataset::open(&data.root).expect("opens");
    let source_report = &source.layout()[0];
    assert_eq!(
        source_report.hostile_views,
        vec![Orientation::XZ, Orientation::YZ]
    );
    let before = support::store_digest(&data.root);

    let out = tempfile::TempDir::new().expect("tempdir");
    let target = Dims {
        t: 1,
        c: 1,
        z: 4,
        y: 256,
        x: 256,
    };
    let progress = Progress::default();
    let copy_root = rechunk::rechunk(
        &source,
        &out.path().join("image.zarr"),
        target,
        &progress.record(),
    )
    .expect("rechunk");
    progress.assert_monotonic_to_completion();

    let copy = dataset::open(&copy_root).expect("working copy opens");
    assert_eq!(copy.dims, source.dims);
    assert_eq!(copy.dtype, source.dtype);
    assert_eq!(copy.levels.len(), source.levels.len());
    for (a, b) in copy.levels.iter().zip(&source.levels) {
        assert_eq!(a.dims, b.dims);
        assert_eq!(
            a.factor, b.factor,
            "per-level scale factors survive the copy"
        );
    }
    assert_eq!(copy.scale, source.scale);
    assert_eq!(
        copy.channels
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>(),
        source
            .channels
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        copy.levels[0].chunks,
        Dims {
            t: 1,
            c: 1,
            z: 4,
            y: 16,
            x: 16
        }
    );

    // The advisory clears for the orientations the working copy was built for.
    let report = &copy.layout()[0];
    assert!(!report.hostile, "{report:?}");

    // Every pixel path reads the same values off the working copy.
    let reader = ImageReader::new(Arc::new(copy), 8 << 20);
    for level in 0..2 {
        let dims = data.level_dims[level];
        for t in 0..dims.t {
            for c in 0..dims.c {
                let volume = reader
                    .read_volume(LayerId::Image, level as u32, t, c)
                    .expect("volume");
                assert_eq!(as_u16(&volume.bytes), data.volume(level, t, c));
            }
        }
        let plane = reader
            .read_ortho_plane(LayerId::Image, level as u32, OrthoAxis::XZ, 1, 1, 3)
            .expect("xz plane");
        assert_eq!(as_u16(&plane.bytes), data.xz_plane(level, 1, 1, 3));
    }

    assert_eq!(
        support::store_digest(&data.root),
        before,
        "the source store must be untouched"
    );
}

#[test]
fn rechunk_clamps_bricks_to_thin_stacks() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 1,
                c: 1,
                z: 3,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 1,
                y: 8,
                x: 8,
            })
            .levels(1),
    );
    let source = dataset::open(&data.root).expect("opens");
    let out = tempfile::TempDir::new().expect("tempdir");
    let root = rechunk::rechunk(
        &source,
        &out.path().join("copy.zarr"),
        rechunk::DEFAULT_BRICK,
        &|_| {},
    )
    .expect("rechunk");

    let copy = dataset::open(&root).expect("opens");
    assert_eq!(
        copy.levels[0].chunks,
        Dims {
            t: 1,
            c: 1,
            z: 3,
            y: 8,
            x: 8
        },
        "a 3-plane stack gets 3-plane bricks, not 16"
    );
    let reader = ImageReader::new(Arc::new(copy), 1 << 20);
    let volume = reader.read_volume(LayerId::Image, 0, 0, 0).expect("volume");
    assert_eq!(as_u16(&volume.bytes), data.volume(0, 0, 0));
}

#[test]
fn rechunk_rejects_multi_frame_bricks() {
    let data = support::build(Spec::new(Format::V3).levels(1));
    let source = dataset::open(&data.root).expect("opens");
    let out = tempfile::TempDir::new().expect("tempdir");
    let err = rechunk::rechunk(
        &source,
        &out.path().join("copy.zarr"),
        Dims {
            t: 2,
            c: 1,
            z: 4,
            y: 8,
            x: 8,
        },
        &|_| {},
    )
    .expect_err("rejected");
    assert!(
        matches!(err, rechunk::RechunkError::UnsupportedTarget { t: 2, c: 1 }),
        "{err}"
    );
}
