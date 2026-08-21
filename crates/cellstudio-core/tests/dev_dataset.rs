//! Real-data checks against the development dataset (skipped unless CELLSTUDIO_DEV_DATASET is set).

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use cellstudio_core::axes::{Dims, Dtype, Orientation};
use cellstudio_core::dataset::{self, ZarrFormat};
use cellstudio_core::reader::{ImageReader, OrthoAxis};
use cellstudio_core::{LayerId, bricks};

fn dev_dataset() -> Option<PathBuf> {
    let raw = std::env::var_os("CELLSTUDIO_DEV_DATASET")?;
    let path = PathBuf::from(raw);
    if !path.is_dir() {
        eprintln!(
            "skipping: CELLSTUDIO_DEV_DATASET is not a directory: {}",
            path.display()
        );
        return None;
    }
    Some(path)
}

macro_rules! dev {
    () => {
        match dev_dataset() {
            Some(path) => path,
            None => {
                eprintln!("skipping: CELLSTUDIO_DEV_DATASET is unset");
                return;
            }
        }
    };
}

#[test]
fn opens_with_the_expected_geometry() {
    let root = dev!();
    let dataset = dataset::open(&root).expect("development dataset opens");

    assert_eq!(dataset.format, ZarrFormat::V2);
    assert_eq!(
        dataset.dims,
        Dims {
            t: 277,
            c: 3,
            z: 3,
            y: 1024,
            x: 1024
        }
    );
    assert_eq!(dataset.dtype, Dtype::U16);
    assert_eq!(dataset.levels.len(), 3);

    let extents: Vec<(u64, u64, u64)> = dataset
        .levels
        .iter()
        .map(|l| (l.dims.z, l.dims.y, l.dims.x))
        .collect();
    assert_eq!(extents, vec![(3, 1024, 1024), (3, 512, 512), (3, 256, 256)]);
    let factors: Vec<[f64; 3]> = dataset.levels.iter().map(|l| l.factor).collect();
    assert_eq!(
        factors,
        vec![[1.0, 1.0, 1.0], [1.0, 2.0, 2.0], [1.0, 4.0, 4.0]]
    );

    // One chunk per (t, c) holding the whole ZYX block, at every level.
    for level in &dataset.levels {
        assert_eq!(
            level.chunks,
            Dims {
                t: 1,
                c: 1,
                z: 3,
                y: level.dims.y,
                x: level.dims.x
            }
        );
    }
}

#[test]
fn anisotropic_voxel_size_is_surfaced() {
    let root = dev!();
    let dataset = dataset::open(&root).expect("opens");
    let scale = dataset.scale.expect("voxel size");

    assert_eq!(scale.z, 2.0);
    approx::assert_abs_diff_eq!(scale.y, 0.60296875, epsilon = 1e-12);
    approx::assert_abs_diff_eq!(scale.x, 0.6029296875, epsilon = 1e-12);
    assert_ne!(
        scale.y, scale.x,
        "Y and X spacing differ in the fourth decimal"
    );
    approx::assert_abs_diff_eq!(scale.ratio(scale.z, scale.x), 3.3171, epsilon = 1e-4);
}

#[test]
fn omero_channel_metadata_initializes_the_controls() {
    let root = dev!();
    let dataset = dataset::open(&root).expect("opens");

    assert_eq!(dataset.channels.len(), 3);
    let colors: Vec<&str> = dataset.channels.iter().map(|c| c.color.as_str()).collect();
    assert_eq!(colors, vec!["FF0000", "FFB100", "37FF00"]);
    let windows: Vec<[f64; 2]> = dataset.channels.iter().map(|c| c.window).collect();
    assert_eq!(
        windows,
        vec![[480.0, 5716.0], [480.0, 2440.0], [480.0, 4198.0]]
    );
    for (channel, prefix) in dataset
        .channels
        .iter()
        .zip(["637_Cy5", "561_RFP", "488_GFP"])
    {
        assert!(channel.name.starts_with(prefix), "{}", channel.name);
        assert_eq!(channel.limits, [0.0, 65535.0]);
        assert!(!channel.defaulted);
    }
}

#[test]
fn the_layout_advisory_does_not_fire() {
    let root = dev!();
    let dataset = dataset::open(&root).expect("opens");

    for report in dataset.layout() {
        let xy = report.view(Orientation::XY).expect("xy view");
        assert_eq!(
            xy.amplification, 3.0,
            "one 3-plane chunk per XY plane at level {}",
            report.level
        );
        for orientation in [Orientation::XZ, Orientation::YZ] {
            let view = report.view(orientation).expect("ortho view");
            assert_eq!(view.chunks_decoded, 1);
            assert_eq!(view.column_chunks, 1);
        }
        assert!(
            !report.hostile,
            "level {} must not raise the advisory",
            report.level
        );
    }
}

#[test]
fn ortho_planes_agree_with_the_pixel_readout() {
    let root = dev!();
    let dataset = Arc::new(dataset::open(&root).expect("opens"));
    let reader = ImageReader::new(dataset, 64 << 20);

    let (t, c, y) = (130_u64, 1_u64, 512_u64);
    let plane = reader
        .read_ortho_plane(LayerId::Image, 0, OrthoAxis::XZ, t, c, y)
        .expect("xz plane");
    assert_eq!(
        plane.shape,
        [3, 1024],
        "a 3-plane stack makes the ortho view 3 px tall"
    );
    let samples = support::as_u16(&plane.bytes);
    assert_eq!(samples.len(), 3 * 1024);

    for (z, x) in [(0_u64, 0_u64), (1, 513), (2, 1023)] {
        let expected = reader
            .read_pixel(LayerId::Image, t, c, [z, y, x])
            .expect("pixel");
        assert_eq!(
            u64::from(samples[(z * 1024 + x) as usize]),
            expected,
            "z{z} x{x}"
        );
    }

    let yz = reader
        .read_ortho_plane(LayerId::Image, 0, OrthoAxis::YZ, t, c, 600)
        .expect("yz plane");
    assert_eq!(yz.shape, [3, 1024]);
    let yz_samples = support::as_u16(&yz.bytes);
    let expected = reader
        .read_pixel(LayerId::Image, t, c, [1, 700, 600])
        .expect("pixel");
    assert_eq!(u64::from(yz_samples[1024 + 700]), expected);

    // Both planes came out of the one chunk that holds this (t, c) block.
    assert_eq!(reader.bricks().stats().decodes, 1);
}

#[test]
fn coarse_volume_and_histogram_stay_off_the_full_resolution_level() {
    let root = dev!();
    let dataset = Arc::new(dataset::open(&root).expect("opens"));
    let reader = ImageReader::new(dataset, 64 << 20);

    let volume = reader
        .read_volume(LayerId::Image, 2, 0, 0)
        .expect("coarse volume");
    assert_eq!(volume.shape, [3, 256, 256]);
    assert_eq!(volume.bytes.len(), 3 * 256 * 256 * 2);
    assert!(!volume.from_proxy);

    let histogram = reader.histogram(LayerId::Image, 0, 0).expect("histogram");
    assert_eq!(
        histogram.level, 2,
        "the histogram samples the coarsest level"
    );
    assert_eq!(histogram.samples, 3 * 256 * 256);
    assert_eq!(histogram.bins.len(), 256);
    assert_eq!(histogram.range, [0, u64::from(u16::MAX)]);
    assert!(histogram.max > histogram.min);
    assert_eq!(
        histogram.bins.iter().map(|b| u64::from(*b)).sum::<u64>(),
        histogram.samples
    );

    // Level 0 was never touched: every decode above was a level-2 chunk.
    assert!(reader.bricks().stats().bytes <= 2 * 3 * 256 * 256 * 2);
}

#[test]
fn reading_never_writes_to_the_source() {
    let root = dev!();
    let before = support::store_digest(&root);

    let dataset = Arc::new(dataset::open(&root).expect("opens"));
    let reader = ImageReader::new(dataset, bricks::DEFAULT_CAPACITY_BYTES.min(64 << 20));
    reader
        .read_ortho_plane(LayerId::Image, 2, OrthoAxis::XZ, 5, 0, 128)
        .expect("xz plane");
    reader.read_volume(LayerId::Image, 2, 5, 0).expect("volume");
    reader
        .read_pixel(LayerId::Image, 5, 0, [1, 500, 500])
        .expect("pixel");
    reader.histogram(LayerId::Image, 5, 0).expect("histogram");

    assert_eq!(
        support::store_digest(&root),
        before,
        "the source store must be untouched"
    );
}
