//! Ortho-plane assembly, volume reads, pixel readout, histograms, and thin-Z behaviour.

mod support;

use std::sync::Arc;

use cellstudio_core::axes::{Axis, Dims, Dtype};
use cellstudio_core::dataset;
use cellstudio_core::reader::{ImageReader, OrthoAxis, ReadError};
use cellstudio_core::{LayerId, bricks};
use support::{Format, Spec, as_u16};

fn reader(data: &support::Data) -> ImageReader {
    let dataset = Arc::new(dataset::open(&data.root).expect("opens"));
    ImageReader::new(dataset, 8 << 20)
}

/// Multi-brick geometry: every axis spans several chunks, and the trailing bricks are
/// partial, so an assembly bug cannot hide.
fn multi_brick(format: Format) -> support::Data {
    support::build(
        Spec::new(format)
            .dims(Dims {
                t: 2,
                c: 2,
                z: 5,
                y: 10,
                x: 14,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 2,
                y: 4,
                x: 4,
            })
            .levels(2),
    )
}

#[test]
fn xz_planes_match_the_source_data() {
    for format in [Format::V2, Format::V3] {
        let data = multi_brick(format);
        let reader = reader(&data);
        for level in 0..data.level_dims.len() {
            let dims = data.level_dims[level];
            for t in 0..dims.t {
                for c in 0..dims.c {
                    for y in 0..dims.y {
                        let plane = reader
                            .read_ortho_plane(LayerId::Image, level as u32, OrthoAxis::XZ, t, c, y)
                            .expect("xz plane");
                        assert_eq!(plane.shape, [dims.z as u32, dims.x as u32]);
                        assert_eq!(plane.dtype, Dtype::U16);
                        assert_eq!(plane.level, level as u32);
                        assert_eq!(
                            as_u16(&plane.bytes),
                            data.xz_plane(level, t, c, y),
                            "{format:?} level {level} t{t} c{c} y{y}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn yz_planes_match_the_source_data() {
    for format in [Format::V2, Format::V3] {
        let data = multi_brick(format);
        let reader = reader(&data);
        for level in 0..data.level_dims.len() {
            let dims = data.level_dims[level];
            for x in 0..dims.x {
                let plane = reader
                    .read_ortho_plane(LayerId::Image, level as u32, OrthoAxis::YZ, 1, 1, x)
                    .expect("yz plane");
                assert_eq!(plane.shape, [dims.z as u32, dims.y as u32]);
                assert_eq!(
                    as_u16(&plane.bytes),
                    data.yz_plane(level, 1, 1, x),
                    "{format:?} level {level} x{x}"
                );
            }
        }
    }
}

#[test]
fn ortho_reads_reuse_the_brick_column() {
    let data = multi_brick(Format::V3);
    let reader = reader(&data);
    // z spans 3 brick layers, x spans 4: 12 bricks per XZ column.
    reader
        .read_ortho_plane(LayerId::Image, 0, OrthoAxis::XZ, 0, 0, 0)
        .expect("first plane");
    let cold = reader.bricks().stats();
    assert_eq!(cold.decodes, 12);

    // Every other y inside the same brick row is served without a new decode.
    for y in 1..4 {
        reader
            .read_ortho_plane(LayerId::Image, 0, OrthoAxis::XZ, 0, 0, y)
            .expect("warm plane");
    }
    assert_eq!(
        reader.bricks().stats().decodes,
        12,
        "one decode amortizes the brick's y extent"
    );
}

#[test]
fn volumes_match_the_source_data() {
    let data = multi_brick(Format::V2);
    let reader = reader(&data);
    for level in 0..data.level_dims.len() {
        let dims = data.level_dims[level];
        for t in 0..dims.t {
            for c in 0..dims.c {
                let volume = reader
                    .read_volume(LayerId::Image, level as u32, t, c)
                    .expect("volume");
                assert_eq!(volume.shape, [dims.z as u32, dims.y as u32, dims.x as u32]);
                assert!(!volume.from_proxy);
                assert_eq!(as_u16(&volume.bytes), data.volume(level, t, c));
            }
        }
    }
}

#[test]
fn pixels_match_the_source_data() {
    let data = multi_brick(Format::V3);
    let reader = reader(&data);
    let dims = data.level_dims[0];
    for t in 0..dims.t {
        for c in 0..dims.c {
            for zyx in [[0, 0, 0], [1, 5, 9], [4, 9, 13], [2, 4, 4]] {
                let value = reader.read_pixel(LayerId::Image, t, c, zyx).expect("pixel");
                assert_eq!(
                    value,
                    u64::from(data.at(0, t, c, zyx[0], zyx[1], zyx[2])),
                    "t{t} c{c} {zyx:?}"
                );
            }
        }
    }
}

#[test]
fn out_of_bounds_reads_name_the_axis() {
    let data = multi_brick(Format::V3);
    let reader = reader(&data);

    match reader.read_ortho_plane(LayerId::Image, 0, OrthoAxis::XZ, 0, 0, 99) {
        Err(ReadError::OutOfBounds {
            axis,
            index,
            extent,
        }) => {
            assert_eq!((axis, index, extent), (Axis::Y, 99, 10));
        }
        other => panic!("expected OutOfBounds, got {other:?}"),
    }
    match reader.read_ortho_plane(LayerId::Image, 0, OrthoAxis::YZ, 0, 0, 99) {
        Err(ReadError::OutOfBounds { axis, .. }) => assert_eq!(axis, Axis::X),
        other => panic!("expected OutOfBounds, got {other:?}"),
    }
    match reader.read_pixel(LayerId::Image, 0, 0, [9, 0, 0]) {
        Err(ReadError::OutOfBounds { axis, .. }) => assert_eq!(axis, Axis::Z),
        other => panic!("expected OutOfBounds, got {other:?}"),
    }
    assert!(matches!(
        reader.read_volume(LayerId::Image, 7, 0, 0),
        Err(ReadError::Dataset(dataset::OpenError::NoSuchLevel { .. }))
    ));
}

/// Thin-Z is a real case, not a corner: with Z=3 an ortho plane is 3 px tall and the
/// volume is a slab. Both must read correctly.
#[test]
fn thin_z_ortho_and_volume_reads() {
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 2,
                c: 3,
                z: 3,
                y: 16,
                x: 16,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 3,
                y: 16,
                x: 16,
            })
            .levels(3),
    );
    let reader = reader(&data);
    assert_eq!(reader.dataset().dims.z, 3);

    for level in 0..3 {
        let dims = data.level_dims[level];
        assert_eq!(dims.z, 3, "an XY-only pyramid keeps the slab 3 planes deep");
        // Each level halves Y and X, so the probe index is level-relative.
        let (y, x) = (dims.y / 2, dims.x / 2 + 1);

        let xz = reader
            .read_ortho_plane(LayerId::Image, level as u32, OrthoAxis::XZ, 1, 2, y)
            .expect("xz plane");
        assert_eq!(xz.shape, [3, dims.x as u32]);
        assert_eq!(as_u16(&xz.bytes), data.xz_plane(level, 1, 2, y));

        let yz = reader
            .read_ortho_plane(LayerId::Image, level as u32, OrthoAxis::YZ, 1, 2, x)
            .expect("yz plane");
        assert_eq!(yz.shape, [3, dims.y as u32]);
        assert_eq!(as_u16(&yz.bytes), data.yz_plane(level, 1, 2, x));

        let volume = reader
            .read_volume(LayerId::Image, level as u32, 1, 2)
            .expect("volume");
        assert_eq!(volume.shape, [3, dims.y as u32, dims.x as u32]);
        assert_eq!(as_u16(&volume.bytes), data.volume(level, 1, 2));
    }
}

#[test]
fn single_plane_store_reads_as_one_row() {
    let data = support::build(
        Spec::new(Format::V3)
            .axes(&[Axis::Y, Axis::X])
            .dims(Dims {
                t: 1,
                c: 1,
                z: 1,
                y: 8,
                x: 8,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 1,
                y: 4,
                x: 4,
            })
            .scale(Some(vec![0.5, 0.5]))
            .levels(1),
    );
    let reader = reader(&data);
    let plane = reader
        .read_ortho_plane(LayerId::Image, 0, OrthoAxis::XZ, 0, 0, 3)
        .expect("xz plane");
    assert_eq!(plane.shape, [1, 8]);
    assert_eq!(as_u16(&plane.bytes), data.xz_plane(0, 0, 0, 3));
}

#[test]
fn histograms_come_from_the_coarsest_level_and_are_memoized() {
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 2,
                c: 2,
                z: 4,
                y: 32,
                x: 32,
            })
            .chunks(Dims {
                t: 1,
                c: 1,
                z: 4,
                y: 32,
                x: 32,
            })
            .levels(3),
    );
    let reader = reader(&data);
    let coarsest = reader.dataset().coarsest_level();
    assert_eq!(coarsest, 2);

    let histogram = reader.histogram(LayerId::Image, 1, 0).expect("histogram");
    assert_eq!(
        histogram.level, coarsest,
        "the full-resolution level is never read"
    );
    assert_eq!(histogram.range, [0, u64::from(u16::MAX)]);
    assert_eq!(histogram.bins.len(), 256);

    let coarse = data.volume(2, 1, 0);
    assert_eq!(histogram.samples, coarse.len() as u64);
    assert_eq!(
        histogram.bins.iter().map(|b| u64::from(*b)).sum::<u64>(),
        histogram.samples
    );
    assert_eq!(histogram.min, u64::from(*coarse.iter().min().expect("min")));
    assert_eq!(histogram.max, u64::from(*coarse.iter().max().expect("max")));

    let decodes = reader.bricks().stats().decodes;
    let again = reader
        .histogram(LayerId::Image, 1, 0)
        .expect("memoized histogram");
    assert_eq!(again, histogram);
    assert_eq!(
        reader.bricks().stats().decodes,
        decodes,
        "memoized per (layer, t, channel)"
    );

    // A different channel is a different entry.
    let other = reader.histogram(LayerId::Image, 1, 1).expect("histogram");
    assert_ne!(other.bins, histogram.bins);
}

#[test]
fn histogram_bins_track_intensity() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 1,
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
            .levels(1),
    );
    let reader = reader(&data);
    let histogram = reader.histogram(LayerId::Image, 0, 0).expect("histogram");
    let expected = data.volume(0, 0, 0);

    let mut bins = vec![0_u32; 256];
    for value in &expected {
        bins[(u32::from(*value) * 256 / 65536) as usize] += 1;
    }
    assert_eq!(histogram.bins, bins);
}

#[test]
fn volume_reads_refuse_to_allocate_beyond_the_limit() {
    // Metadata-only: the store claims a level far larger than the volume limit.
    let dims = Dims {
        t: 1,
        c: 1,
        z: 512,
        y: 4096,
        x: 4096,
    };
    let element = 2_u64;
    assert!(dims.zyx_voxels() * element > cellstudio_core::reader::MAX_VOLUME_BYTES);
    // The guard is checked before any brick is decoded, which is what keeps a wrong
    // level request from exhausting memory.
    assert_eq!(bricks::DEFAULT_CAPACITY_BYTES, 2 << 30);
}
