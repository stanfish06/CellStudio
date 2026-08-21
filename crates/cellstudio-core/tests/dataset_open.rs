//! Opening v2/v3 stores: axis normalization, per-level factors, rejections, read-only guarantees.

mod support;

use cellstudio_core::axes::{Axis, Dims, Dtype};
use cellstudio_core::dataset::{self, OpenError, ZarrFormat};
use support::{Format, Spec};

#[test]
fn opens_zarr_v2_ngff_04() {
    let data = support::build(Spec::new(Format::V2).omero(true));
    let dataset = dataset::open(&data.root).expect("v2 store opens");

    assert_eq!(dataset.format, ZarrFormat::V2);
    assert_eq!(
        dataset.dims,
        Dims {
            t: 2,
            c: 2,
            z: 4,
            y: 8,
            x: 8
        }
    );
    assert_eq!(dataset.dtype, Dtype::U16);
    assert_eq!(dataset.levels.len(), 2);
    assert_eq!(dataset.channels.len(), 2);
    let scale = dataset.scale.expect("scale from coordinateTransformations");
    assert_eq!((scale.z, scale.y, scale.x), (2.0, 0.5, 0.5));
}

#[test]
fn opens_zarr_v3_with_ome_nested_metadata() {
    let data = support::build(Spec::new(Format::V3).omero(true));
    let dataset = dataset::open(&data.root).expect("v3 store opens");

    assert_eq!(dataset.format, ZarrFormat::V3);
    assert_eq!(
        dataset.dims,
        Dims {
            t: 2,
            c: 2,
            z: 4,
            y: 8,
            x: 8
        }
    );
    assert_eq!(dataset.levels.len(), 2);
    // NGFF 0.5 nests multiscales under `ome`; the scale must still be found.
    assert!(dataset.scale.is_some());
    assert_eq!(dataset.channels[0].name, "probe-0");
}

#[test]
fn v2_and_v3_report_identical_geometry() {
    let v2 = dataset::open(&support::build(Spec::new(Format::V2)).root).expect("v2");
    let v3 = dataset::open(&support::build(Spec::new(Format::V3)).root).expect("v3");

    assert_eq!(v2.dims, v3.dims);
    assert_eq!(v2.dtype, v3.dtype);
    assert_eq!(v2.levels.len(), v3.levels.len());
    for (a, b) in v2.levels.iter().zip(&v3.levels) {
        assert_eq!(a.dims, b.dims);
        assert_eq!(a.chunks, b.chunks);
        assert_eq!(a.factor, b.factor);
    }
}

#[test]
fn missing_axes_normalize_to_extent_one() {
    // YX only: t, c and z are absent from the store.
    let yx = support::build(
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
            .levels(1),
    );
    let dataset = dataset::open(&yx.root).expect("yx store opens");
    assert_eq!(
        dataset.dims,
        Dims {
            t: 1,
            c: 1,
            z: 1,
            y: 8,
            x: 8
        }
    );
    assert_eq!(
        dataset.levels[0].chunks,
        Dims {
            t: 1,
            c: 1,
            z: 1,
            y: 4,
            x: 4
        }
    );
    assert_eq!(dataset.channels.len(), 1);

    // CZYX: no time axis.
    let czyx = support::build(
        Spec::new(Format::V2)
            .axes(&[Axis::C, Axis::Z, Axis::Y, Axis::X])
            .dims(Dims {
                t: 1,
                c: 3,
                z: 4,
                y: 8,
                x: 8,
            })
            .scale(Some(vec![1.0, 2.0, 0.5, 0.5]))
            .levels(1),
    );
    let dataset = dataset::open(&czyx.root).expect("czyx store opens");
    assert_eq!(
        dataset.dims,
        Dims {
            t: 1,
            c: 3,
            z: 4,
            y: 8,
            x: 8
        }
    );
    let scale = dataset.scale.expect("scale");
    assert_eq!((scale.z, scale.y, scale.x), (2.0, 0.5, 0.5));
}

#[test]
fn xy_only_pyramid_keeps_z_factor_at_one() {
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 1,
                c: 1,
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
    let dataset = dataset::open(&data.root).expect("opens");

    let z_extents: Vec<u64> = dataset.levels.iter().map(|l| l.dims.z).collect();
    assert_eq!(
        z_extents,
        vec![3, 3, 3],
        "an XY-only pyramid keeps Z constant"
    );
    let factors: Vec<[f64; 3]> = dataset.levels.iter().map(|l| l.factor).collect();
    assert_eq!(
        factors,
        vec![[1.0, 1.0, 1.0], [1.0, 2.0, 2.0], [1.0, 4.0, 4.0]]
    );
}

#[test]
fn isotropic_pyramid_reports_z_factors_as_stored() {
    let data = support::build(
        Spec::new(Format::V3)
            .isotropic_pyramid()
            .dims(Dims {
                t: 1,
                c: 1,
                z: 8,
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
            .levels(2),
    );
    let dataset = dataset::open(&data.root).expect("opens");
    assert_eq!(dataset.levels[1].factor, [2.0, 2.0, 2.0]);
    assert_eq!(
        dataset.levels[1].dims,
        Dims {
            t: 1,
            c: 1,
            z: 4,
            y: 8,
            x: 8
        }
    );
}

#[test]
fn factors_fall_back_to_extent_ratios_without_scale_metadata() {
    let data = support::build(Spec::new(Format::V2).scale(None).levels(3));
    let dataset = dataset::open(&data.root).expect("opens");

    assert!(
        dataset.scale.is_none(),
        "no scale metadata means no physical scale"
    );
    assert_eq!(dataset.levels[2].factor, [1.0, 4.0, 4.0]);
}

#[test]
fn bare_array_store_opens_as_one_level() {
    let data = support::build(Spec::new(Format::V3).bare_array().dims(Dims {
        t: 2,
        c: 1,
        z: 2,
        y: 8,
        x: 8,
    }));
    let dataset = dataset::open(&data.root).expect("bare array opens");
    assert_eq!(dataset.levels.len(), 1);
    assert_eq!(
        dataset.dims,
        Dims {
            t: 2,
            c: 1,
            z: 2,
            y: 8,
            x: 8
        }
    );
    assert!(dataset.scale.is_none());
}

#[test]
fn missing_path_is_named() {
    let err = dataset::open(std::path::Path::new("/nonexistent/cellstudio")).expect_err("rejected");
    assert!(matches!(err, OpenError::NotFound(_)), "got {err}");
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn empty_directory_is_not_a_zarr_store() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let err = dataset::open(dir.path()).expect_err("rejected");
    assert!(matches!(err, OpenError::NotAZarrStore(_)), "got {err}");
    let message = err.to_string();
    assert!(message.contains("zarr.json"), "{message}");
    assert!(message.contains(".zarray"), "{message}");
}

#[test]
fn group_without_multiscales_is_named() {
    let (_dir, root) = support::group_without_multiscales();
    let err = dataset::open(&root).expect_err("rejected");
    assert!(matches!(err, OpenError::MissingMultiscales), "got {err}");
    assert!(err.to_string().contains("multiscales"));
}

#[test]
fn dangling_level_names_the_level() {
    let (_dir, root) = support::multiscales_without_level();
    let err = dataset::open(&root).expect_err("rejected");
    match err {
        OpenError::MissingLevel {
            level, ref path, ..
        } => {
            assert_eq!(level, 0);
            assert_eq!(path, "0");
        }
        other => panic!("expected MissingLevel, got {other}"),
    }
}

#[test]
fn float_sample_type_is_rejected_by_name() {
    let (_dir, root) = support::float_store();
    let err = dataset::open(&root).expect_err("rejected");
    match err {
        OpenError::UnsupportedDtype { ref dtype } => assert!(dtype.contains("float32"), "{dtype}"),
        other => panic!("expected UnsupportedDtype, got {other}"),
    }
    assert!(err.to_string().contains("uint16"));
}

#[test]
fn non_tczyx_axis_names_are_rejected() {
    let (_dir, root) = support::unsupported_axes();
    let err = dataset::open(&root).expect_err("rejected");
    match err {
        OpenError::UnsupportedAxes { ref names } => {
            assert!(names.iter().any(|n| n == "wavelength"), "{names:?}");
        }
        other => panic!("expected UnsupportedAxes, got {other}"),
    }
}

#[test]
fn axes_out_of_tczyx_order_are_rejected() {
    let (_dir, root) = support::out_of_order_axes();
    let err = dataset::open(&root).expect_err("rejected");
    assert!(
        matches!(err, OpenError::UnsupportedAxisOrder { .. }),
        "got {err}"
    );
    assert!(err.to_string().contains("TCZYX order"));
}

#[test]
fn opening_never_writes_to_the_source() {
    let data = support::build(Spec::new(Format::V2).omero(true));
    let before = support::store_digest(&data.root);

    let dataset = dataset::open(&data.root).expect("opens");
    let _ = dataset.layout();
    let _ = dataset::open(&data.root).expect("reopens");

    let after = support::store_digest(&data.root);
    assert_eq!(
        before, after,
        "opening must not create, modify or delete any file"
    );
}
