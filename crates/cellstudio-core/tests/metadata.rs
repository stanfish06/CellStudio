//! Omero channel metadata and anisotropic voxel size.

mod support;

use cellstudio_core::axes::Dims;
use cellstudio_core::dataset::{self, DEFAULT_CHANNEL_COLORS};
use support::{Format, Spec};

#[test]
fn omero_channels_supply_name_color_and_window() {
    let data = support::build(
        Spec::new(Format::V2)
            .dims(Dims {
                t: 1,
                c: 3,
                z: 2,
                y: 8,
                x: 8,
            })
            .omero(true)
            .levels(1),
    );
    let dataset = dataset::open(&data.root).expect("opens");

    assert_eq!(dataset.channels.len(), 3);
    let names: Vec<&str> = dataset.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["probe-0", "probe-1", "probe-2"]);
    let colors: Vec<&str> = dataset.channels.iter().map(|c| c.color.as_str()).collect();
    assert_eq!(colors, vec!["FF0000", "FFB100", "37FF00"]);
    for (index, channel) in dataset.channels.iter().enumerate() {
        assert_eq!(
            channel.window,
            [100.0 * (index + 1) as f64, 4000.0 + index as f64]
        );
        assert_eq!(channel.limits, [0.0, 65535.0]);
        assert!(!channel.defaulted);
        assert!(channel.active);
    }
}

#[test]
fn channels_default_when_the_store_has_no_display_metadata() {
    let data = support::build(
        Spec::new(Format::V3)
            .dims(Dims {
                t: 1,
                c: 4,
                z: 2,
                y: 8,
                x: 8,
            })
            .omero(false)
            .levels(1),
    );
    let dataset = dataset::open(&data.root).expect("opens");

    assert_eq!(dataset.channels.len(), 4);
    for (index, channel) in dataset.channels.iter().enumerate() {
        assert!(
            channel.defaulted,
            "channel {index} should be marked defaulted"
        );
        assert_eq!(channel.name, format!("Channel {}", index + 1));
        assert_eq!(channel.color, DEFAULT_CHANNEL_COLORS[index]);
        assert_eq!(channel.window, [0.0, 65535.0], "uint16 full range");
        assert_eq!(channel.limits, [0.0, 65535.0]);
    }
    assert_eq!(
        &DEFAULT_CHANNEL_COLORS[..3],
        &["FF0000", "00FF00", "0000FF"],
        "red, green, blue"
    );
}

#[test]
fn colors_are_normalized_without_the_hash() {
    let data = support::build(Spec::new(Format::V2).omero(true).levels(1));
    let dataset = dataset::open(&data.root).expect("opens");
    for channel in &dataset.channels {
        assert!(!channel.color.starts_with('#'));
        assert_eq!(channel.color, channel.color.to_uppercase());
    }
}

#[test]
fn anisotropic_voxel_size_is_surfaced() {
    // 3.3:1 anisotropy with unequal Y and X, as in the development dataset.
    let data = support::build(
        Spec::new(Format::V2)
            .scale(Some(vec![600.014, 1.0, 2.0, 0.60296875, 0.6029296875]))
            .levels(2),
    );
    let dataset = dataset::open(&data.root).expect("opens");
    let scale = dataset.scale.expect("scale");

    assert_eq!(scale.z, 2.0);
    assert_eq!(scale.y, 0.60296875);
    assert_eq!(scale.x, 0.6029296875);
    assert_ne!(scale.y, scale.x, "Y and X spacing need not be equal");
    approx::assert_abs_diff_eq!(scale.ratio(scale.z, scale.x), 3.3171, epsilon = 1e-4);
}

#[test]
fn space_units_are_converted_to_micrometres() {
    let data = support::build(
        Spec::new(Format::V3)
            .space_unit("nanometer")
            .scale(Some(vec![1.0, 1.0, 2000.0, 500.0, 500.0]))
            .levels(1),
    );
    let dataset = dataset::open(&data.root).expect("opens");
    let scale = dataset.scale.expect("scale");
    assert_eq!((scale.z, scale.y, scale.x), (2.0, 0.5, 0.5));
}
