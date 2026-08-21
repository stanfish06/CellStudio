//! Per-orientation read amplification and the chunk-layout advisory (chunk arithmetic only).

mod support;

use cellstudio_core::axes::{Dims, Dtype, Orientation};
use cellstudio_core::dataset::{self, Level};

fn level(dims: Dims, chunks: Dims) -> Level {
    Level {
        index: 0,
        path: "0".into(),
        dims,
        chunks,
        factor: [1.0, 1.0, 1.0],
    }
}

/// The development dataset: one chunk per (t, c) holding the whole ZYX block, at every
/// level. XY reads decode 3× the plane; ortho reads decode exactly one chunk.
#[test]
fn development_layout_is_not_hostile() {
    for (y, x) in [(1024_u64, 1024_u64), (512, 512), (256, 256)] {
        let dims = Dims {
            t: 277,
            c: 3,
            z: 3,
            y,
            x,
        };
        let report = dataset::analyze_layout(
            &level(
                dims,
                Dims {
                    t: 1,
                    c: 1,
                    z: 3,
                    y,
                    x,
                },
            ),
            Dtype::U16,
        );

        let xy = report.view(Orientation::XY).expect("xy view");
        assert_eq!(
            xy.amplification, 3.0,
            "one 3-plane chunk per XY plane at {y}x{x}"
        );
        assert_eq!(xy.chunks_decoded, 1);
        for orientation in [Orientation::XZ, Orientation::YZ] {
            let view = report.view(orientation).expect("ortho view");
            assert_eq!(
                view.chunks_decoded, 1,
                "{orientation:?} assembles from one chunk"
            );
            assert_eq!(view.column_chunks, 1);
        }
        assert!(
            !report.hostile,
            "the development layout must not raise the advisory"
        );
        assert!(report.hostile_views.is_empty());
    }
}

/// Plane-chunked (z-extent 1): XY is cheap, ortho assembly walks the whole z stack.
#[test]
fn plane_chunked_layout_is_hostile_for_ortho_views() {
    let dims = Dims {
        t: 1,
        c: 1,
        z: 64,
        y: 64,
        x: 64,
    };
    let report = dataset::analyze_layout(
        &level(
            dims,
            Dims {
                t: 1,
                c: 1,
                z: 1,
                y: 64,
                x: 64,
            },
        ),
        Dtype::U16,
    );

    let xy = report.view(Orientation::XY).expect("xy view");
    assert_eq!(xy.amplification, 1.0);
    assert!(!xy.hostile, "plane chunks are ideal for XY");

    for orientation in [Orientation::XZ, Orientation::YZ] {
        let view = report.view(orientation).expect("ortho view");
        assert_eq!(view.column_chunks, 64, "one chunk per z plane");
        assert_eq!(view.amplification, 64.0);
        assert!(view.hostile, "{orientation:?} must be flagged");
    }
    assert!(report.hostile);
    assert_eq!(report.hostile_views, vec![Orientation::XZ, Orientation::YZ]);
}

/// Deep z-bricks: ortho assembly is one decode, XY scrubbing decodes 64 planes to show
/// one (the z-extent 64 scenario).
#[test]
fn deep_zbrick_layout_is_hostile_for_xy() {
    let dims = Dims {
        t: 1,
        c: 1,
        z: 64,
        y: 64,
        x: 64,
    };
    let report = dataset::analyze_layout(
        &level(
            dims,
            Dims {
                t: 1,
                c: 1,
                z: 64,
                y: 32,
                x: 32,
            },
        ),
        Dtype::U16,
    );

    let xy = report.view(Orientation::XY).expect("xy view");
    assert_eq!(xy.chunks_decoded, 4);
    assert_eq!(xy.amplification, 64.0);
    assert!(xy.hostile, "z-extent 64 makes XY scrubbing slow");

    for orientation in [Orientation::XZ, Orientation::YZ] {
        let view = report.view(orientation).expect("ortho view");
        assert_eq!(view.column_chunks, 1);
        assert!(
            !view.hostile,
            "a full-z brick is the best case for {orientation:?}"
        );
    }
    assert_eq!(report.hostile_views, vec![Orientation::XY]);
}

/// The design's default brick shape trades XY amplification for ortho locality:
/// on a deep, wide level it decodes 16 z planes to show one, while an ortho column
/// shrinks to three chunk layers. Recorded here because it means the XY threshold, as
/// specified, still flags the layout re-chunking produces, a Spike 2 calibration item.
#[test]
fn default_brick_shape_trades_xy_amplification_for_ortho_locality() {
    let dims = Dims {
        t: 8,
        c: 2,
        z: 45,
        y: 2048,
        x: 2048,
    };
    let report = dataset::analyze_layout(
        &level(
            dims,
            Dims {
                t: 1,
                c: 1,
                z: 16,
                y: 256,
                x: 256,
            },
        ),
        Dtype::U16,
    );

    let xy = report.view(Orientation::XY).expect("xy view");
    assert_eq!(xy.chunks_decoded, 64);
    assert_eq!(xy.amplification, 16.0);
    assert!(
        xy.hostile,
        "16 z planes decoded per XY plane, amortized over 16 z steps by prefetch"
    );

    let xz = report.view(Orientation::XZ).expect("xz view");
    assert_eq!(xz.column_chunks, 3, "the brick column is bounded");
    assert_eq!(xz.chunks_decoded, 24);
    assert!(!xz.hostile);
}

#[test]
fn byte_counts_scale_with_sample_size() {
    let dims = Dims {
        t: 1,
        c: 1,
        z: 4,
        y: 16,
        x: 16,
    };
    let chunks = Dims {
        t: 1,
        c: 1,
        z: 4,
        y: 16,
        x: 16,
    };
    let u8_report = dataset::analyze_layout(&level(dims, chunks), Dtype::U8);
    let u16_report = dataset::analyze_layout(&level(dims, chunks), Dtype::U16);

    let (a, b) = (
        u8_report.view(Orientation::XY).expect("xy"),
        u16_report.view(Orientation::XY).expect("xy"),
    );
    assert_eq!(b.bytes_needed, a.bytes_needed * 2);
    assert_eq!(b.bytes_decoded, a.bytes_decoded * 2);
    assert_eq!(
        a.amplification, b.amplification,
        "the ratio is dtype-independent"
    );
}

#[test]
fn chunks_larger_than_the_array_do_not_inflate_the_report() {
    // A thin-Z store whose chunk claims 16 z planes only holds 3.
    let dims = Dims {
        t: 1,
        c: 1,
        z: 3,
        y: 32,
        x: 32,
    };
    let report = dataset::analyze_layout(
        &level(
            dims,
            Dims {
                t: 1,
                c: 1,
                z: 16,
                y: 64,
                x: 64,
            },
        ),
        Dtype::U16,
    );
    let xy = report.view(Orientation::XY).expect("xy view");
    assert_eq!(xy.amplification, 3.0);
    assert!(!report.hostile);
}

#[test]
fn layout_is_reported_for_every_level() {
    let data = support::build(
        support::Spec::new(support::Format::V2)
            .dims(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 16,
                x: 16,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 16,
                x: 16,
            })
            .levels(3),
    );
    let dataset = dataset::open(&data.root).expect("opens");
    let reports = dataset.layout();
    assert_eq!(reports.len(), 3);
    for (index, report) in reports.iter().enumerate() {
        assert_eq!(report.level, index as u32);
        assert!(!report.hostile, "whole-block chunks are never hostile");
    }
}

/// Cross-check against the shared correctness data when they have been generated.
/// Skipped rather than failed when absent, so these tests never depend on them.
#[test]
fn shared_hostile_data_agree_with_the_advisory() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.data");
    let planes = root.join("hostile_planes/image.zarr");
    let zbrick = root.join("hostile_zbrick/image.zarr");
    if !planes.is_dir() || !zbrick.is_dir() {
        eprintln!("skipping: shared data not generated");
        return;
    }

    let planes = dataset::open(&planes).expect("hostile_planes opens");
    let report = &planes.layout()[0];
    assert_eq!(report.hostile_views, vec![Orientation::XZ, Orientation::YZ]);

    let zbrick = dataset::open(&zbrick).expect("hostile_zbrick opens");
    let report = &zbrick.layout()[0];
    assert_eq!(report.hostile_views, vec![Orientation::XY]);
}
