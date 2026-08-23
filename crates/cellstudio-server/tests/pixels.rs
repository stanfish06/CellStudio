//! Assembled pixel serving (/slice, /volume, /pixel, /histogram) with cross-endpoint agreement.

mod support;

use std::time::{Duration, Instant};

use support::{Binary, Server, data_copy, dev_dataset, skip};

const JOB_TIMEOUT: Duration = Duration::from_secs(120);

/// The tiny data: TCZYX 4x2x4x32x32 uint16 with three XY-only levels.
const TINY_Z: usize = 4;
const TINY_XY: usize = 32;

fn tiny(dir: &tempfile::TempDir) -> (Server, std::path::PathBuf) {
    let dataset = data_copy(dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    (server, dataset)
}

fn pixel(server: &Server, t: u64, c: u64, z: u64, y: u64, x: u64) -> u64 {
    server.json(&format!("/pixel?t={t}&c={c}&z={z}&y={y}&x={x}"))["value"]
        .as_u64()
        .expect("pixel value is a number")
}

#[test]
fn an_xz_slice_packs_every_requested_channel_into_one_response() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let y = 17;

    let plane = Binary::read(
        server
            .get(&format!("/slice?axis=xz&t=1&cs=0,1&pos={y}"))
            .send()
            .expect("request"),
    );
    // Binary::read has already checked content-length == product(shape) * itemsize
    assert_eq!(
        plane.shape,
        vec![2, TINY_Z as u64, TINY_XY as u64],
        "the shape header is c,h,w with both channels in one response"
    );
    assert_eq!(plane.dtype, "u16");
    assert_eq!(plane.level, 0);
    assert_eq!(plane.session, server.session(), "the serving session");
    assert_eq!(plane.bytes.len(), 2 * TINY_Z * TINY_XY * 2);

    // every voxel of the plane, against the cursor readout at the same coordinate
    let values = plane.u16_values();
    for c in 0..2_usize {
        for z in 0..TINY_Z {
            for x in (0..TINY_XY).step_by(8) {
                let packed = values[c * TINY_Z * TINY_XY + z * TINY_XY + x];
                assert_eq!(
                    u64::from(packed),
                    pixel(&server, 1, c as u64, z as u64, y, x as u64),
                    "xz plane at y={y} channel {c} row z={z} column x={x}"
                );
            }
        }
    }
}

#[test]
fn a_yz_slice_packs_every_requested_channel_into_one_response() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let x = 5;

    let plane = Binary::read(
        server
            .get(&format!("/slice?axis=yz&t=2&cs=0,1&pos={x}"))
            .send()
            .expect("request"),
    );
    assert_eq!(plane.shape, vec![2, TINY_Z as u64, TINY_XY as u64]);
    assert_eq!(plane.dtype, "u16");
    assert_eq!(plane.level, 0);

    let values = plane.u16_values();
    for c in 0..2_usize {
        for z in 0..TINY_Z {
            for y in (0..TINY_XY).step_by(8) {
                let packed = values[c * TINY_Z * TINY_XY + z * TINY_XY + y];
                assert_eq!(
                    u64::from(packed),
                    pixel(&server, 2, c as u64, z as u64, y as u64, x),
                    "yz plane at x={x} channel {c} row z={z} column y={y}"
                );
            }
        }
    }
}

/// One channel asked for is one channel served, and the packing order is `cs` order.
#[test]
fn a_single_channel_slice_is_the_matching_slab_of_the_packed_pair() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);

    let both = Binary::read(
        server
            .get("/slice?axis=xz&t=0&cs=0,1&pos=16")
            .send()
            .expect("request"),
    );
    let first = Binary::read(
        server
            .get("/slice?axis=xz&t=0&cs=0&pos=16")
            .send()
            .expect("request"),
    );
    let second = Binary::read(
        server
            .get("/slice?axis=xz&t=0&cs=1&pos=16")
            .send()
            .expect("request"),
    );
    assert_eq!(first.shape, vec![1, TINY_Z as u64, TINY_XY as u64]);
    let half = TINY_Z * TINY_XY * 2;
    assert_eq!(
        &both.bytes[..half],
        first.bytes.as_slice(),
        "channel 0 slab"
    );
    assert_eq!(
        &both.bytes[half..],
        second.bytes.as_slice(),
        "channel 1 slab"
    );

    // reversing cs reverses the packing
    let reversed = Binary::read(
        server
            .get("/slice?axis=xz&t=0&cs=1,0&pos=16")
            .send()
            .expect("request"),
    );
    assert_eq!(&reversed.bytes[..half], second.bytes.as_slice());
    assert_eq!(&reversed.bytes[half..], first.bytes.as_slice());
}

#[test]
fn a_coarse_level_slice_reports_the_level_it_was_read_at() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);

    let plane = Binary::read(
        server
            .get("/slice?axis=xz&t=0&cs=0&level=2&pos=4")
            .send()
            .expect("request"),
    );
    assert_eq!(plane.level, 2);
    assert_eq!(
        plane.shape,
        vec![1, TINY_Z as u64, 8],
        "level 2 is 8 wide in the data's XY-only pyramid"
    );
}

#[test]
fn a_slice_request_the_reader_cannot_serve_is_a_client_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);

    for (query, status) in [
        ("/slice?axis=zz&t=0&cs=0&pos=0", 400),
        ("/slice?axis=xy&t=0&cs=0&pos=99", 400), // xy is served now; the index still is not
        ("/slice?axis=xz&t=0&cs=&pos=0", 400),
        ("/slice?axis=xz&t=0&cs=a&pos=0", 400),
        ("/slice?axis=xz&t=99&cs=0&pos=0", 400),
        ("/slice?axis=xz&t=0&cs=0&pos=999", 400),
        ("/slice?axis=xz&t=0&cs=0&pos=0&level=9", 400),
        ("/pixel?t=0&c=0&z=99&y=0&x=0", 400),
        ("/pixel?t=0&c=9&z=0&y=0&x=0", 400),
    ] {
        let response = server.get(query).send().expect("request");
        assert_eq!(response.status(), status, "GET {query}");
    }
}

#[test]
fn a_volume_is_assembled_from_the_pyramid_when_no_proxy_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    assert!(
        server.jobs().is_empty(),
        "--no-proxy schedules nothing: {:?}",
        server.jobs()
    );

    let volume = Binary::read(
        server
            .get("/volume?t=0&c=0&level=0")
            .send()
            .expect("request"),
    );
    assert_eq!(
        volume.shape,
        vec![TINY_Z as u64, TINY_XY as u64, TINY_XY as u64]
    );
    assert_eq!(volume.dtype, "u16");
    assert_eq!(volume.level, 0);
    assert_eq!(
        volume
            .extra
            .get("x-cellstudio-volume-source")
            .map(String::as_str),
        Some("pyramid"),
        "no proxy exists, so the pyramid assembles it"
    );

    // the XZ plane at y is the volume's y-th row plane
    let y = 9_usize;
    let plane = Binary::read(
        server
            .get(&format!("/slice?axis=xz&t=0&cs=0&pos={y}"))
            .send()
            .expect("request"),
    );
    let (volume_values, plane_values) = (volume.u16_values(), plane.u16_values());
    for z in 0..TINY_Z {
        for x in 0..TINY_XY {
            assert_eq!(
                plane_values[z * TINY_XY + x],
                volume_values[z * TINY_XY * TINY_XY + y * TINY_XY + x],
                "xz plane at y={y} must be the volume's cross-section at z={z} x={x}"
            );
        }
    }
}

/// `hostile_planes` is the data whose pyramid needs 64 decodes for one ZYX block, which
/// is what makes a proxy worth its disk.
#[test]
fn a_volume_is_served_from_the_proxy_once_the_build_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::start();
    server.open_project(&dataset);

    let jobs = server.await_jobs(JOB_TIMEOUT);
    let proxy = jobs
        .iter()
        .find(|job| job["kind"] == "proxy")
        .unwrap_or_else(|| panic!("no proxy job was scheduled: {jobs:?}"));
    assert_eq!(proxy["status"], "done", "proxy build: {proxy}");
    assert_eq!(proxy["progress"], 1.0);

    let volume = Binary::read(
        server
            .get("/volume?t=1&c=0&level=0")
            .send()
            .expect("request"),
    );
    assert_eq!(
        volume
            .extra
            .get("x-cellstudio-volume-source")
            .map(String::as_str),
        Some("proxy"),
        "an attached proxy at this level serves the volume"
    );
    assert_eq!(volume.shape, vec![64, 64, 64]);

    // the proxy must hold the same voxels the pyramid assembles from a pristine copy
    let other_dir = tempfile::tempdir().expect("tempdir");
    let pristine = data_copy(&other_dir, "hostile_planes", "image.zarr");
    let pyramid_server = Server::without_proxy();
    pyramid_server.open_project(&pristine);
    let pyramid = Binary::read(
        pyramid_server
            .get("/volume?t=1&c=0&level=0")
            .send()
            .expect("request"),
    );
    assert_eq!(
        pyramid
            .extra
            .get("x-cellstudio-volume-source")
            .map(String::as_str),
        Some("pyramid")
    );
    assert_eq!(
        volume.bytes, pyramid.bytes,
        "the proxy must be voxel-identical to the pyramid it was built from"
    );
}

#[test]
fn a_pixel_matches_the_assembled_volume_at_the_same_coordinate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);

    let volume = Binary::read(
        server
            .get("/volume?t=3&c=1&level=0")
            .send()
            .expect("request"),
    );
    let values = volume.u16_values();
    for (z, y, x) in [(0, 0, 0), (1, 7, 30), (2, 16, 16), (3, 31, 31)] {
        assert_eq!(
            pixel(&server, 3, 1, z as u64, y as u64, x as u64),
            u64::from(values[z * TINY_XY * TINY_XY + y * TINY_XY + x]),
            "/pixel at z={z} y={y} x={x}"
        );
    }
    // the data is not uniform, so the cross-check above is not comparing zeros
    assert!(
        values.iter().any(|&v| v != values[0]),
        "the data volume must not be constant"
    );
}

#[test]
fn a_histogram_is_sampled_from_the_coarsest_level() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);

    let levels = server.json("/project")["levels"]
        .as_array()
        .expect("levels")
        .len() as u64;
    let response = server.get("/histogram?t=0&c=0").send().expect("request");
    assert_eq!(response.status(), 200);
    let reported_level = response
        .headers()
        .get("x-cellstudio-level")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("the histogram names the level it sampled");
    let histogram: serde_json::Value = response.json().expect("histogram body");

    assert_eq!(
        histogram["level"], reported_level,
        "the header and the body must name the same level"
    );
    assert_eq!(
        reported_level,
        levels - 1,
        "the histogram reads the coarsest level, never full resolution"
    );
    assert_eq!(histogram["sampled"], true);

    let counts = histogram["counts"].as_array().expect("counts");
    assert_eq!(counts.len(), 256, "the popover draws 256 bins");
    let total: u64 = counts
        .iter()
        .map(|count| count.as_u64().expect("bin count"))
        .sum();
    assert_eq!(
        total,
        histogram["samples"].as_u64().expect("samples"),
        "every sample lands in exactly one bin"
    );
    // the coarsest level of the data is 4x8x8
    assert!(
        total <= 4 * 8 * 8,
        "the sample cannot exceed the coarse level's voxels: {histogram}"
    );
    assert!(
        histogram["min"].as_u64().expect("min") <= histogram["max"].as_u64().expect("max"),
        "{histogram}"
    );
}

#[test]
fn dev_dataset_slices_pack_two_channels_and_agree_with_the_cursor_readout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(dataset) = dev_dataset(&dir) else {
        return skip("CELLSTUDIO_DEV_DATASET is not set");
    };
    let server = Server::without_proxy();
    let info = server.open_project(&dataset);
    assert_eq!(info["dims"]["z"], 3);
    assert_eq!(info["dims"]["x"], 1024);

    for (axis, pos, other) in [("xz", 512_u64, "x"), ("yz", 512, "y")] {
        let plane = Binary::read(
            server
                .get(&format!("/slice?axis={axis}&t=10&cs=0,1&pos={pos}"))
                .send()
                .expect("request"),
        );
        assert_eq!(
            plane.shape,
            vec![2, 3, 1024],
            "{axis} plane is c,h,w over two channels"
        );
        assert_eq!(plane.dtype, "u16");
        assert_eq!(plane.level, 0);
        assert_eq!(
            plane.bytes.len(),
            12288,
            "2 channels * 3 z * 1024 * 2 bytes"
        );

        let values = plane.u16_values();
        for c in 0..2_u64 {
            for z in 0..3_u64 {
                for index in [0_u64, 512, 1023] {
                    let packed = values[(c * 3 * 1024 + z * 1024 + index) as usize];
                    let (y, x) = match other {
                        "x" => (pos, index),
                        _ => (index, pos),
                    };
                    assert_eq!(
                        u64::from(packed),
                        pixel(&server, 10, c, z, y, x),
                        "{axis} plane at {pos} channel {c} z={z} {other}={index}"
                    );
                }
            }
        }
    }

    // the value the reference read was recorded against
    assert_eq!(pixel(&server, 10, 0, 1, 512, 512), 1464);
}

#[test]
fn dev_dataset_volumes_come_from_the_pyramid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(dataset) = dev_dataset(&dir) else {
        return skip("CELLSTUDIO_DEV_DATASET is not set");
    };
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let volume = Binary::read(
        server
            .get("/volume?t=10&c=0&level=2")
            .send()
            .expect("request"),
    );
    assert_eq!(volume.shape, vec![3, 256, 256]);
    assert_eq!(volume.dtype, "u16");
    assert_eq!(volume.level, 2);
    assert_eq!(volume.bytes.len(), 3 * 256 * 256 * 2);
    assert_eq!(
        volume
            .extra
            .get("x-cellstudio-volume-source")
            .map(String::as_str),
        Some("pyramid"),
        "this store reads a whole ZYX block in one chunk, so no proxy is built for it"
    );
}

#[test]
fn dev_dataset_histogram_reads_the_coarsest_level_not_the_full_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(dataset) = dev_dataset(&dir) else {
        return skip("CELLSTUDIO_DEV_DATASET is not set");
    };
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let started = Instant::now();
    let response = server.get("/histogram?t=10&c=0").send().expect("request");
    let elapsed = started.elapsed();
    assert_eq!(response.status(), 200);
    let histogram: serde_json::Value = response.json().expect("histogram body");

    assert_eq!(
        histogram["level"], 2,
        "level 2 is the coarsest of the three, and it is what the settings popover reads"
    );
    assert_eq!(histogram["sampled"], true);
    assert_eq!(histogram["counts"].as_array().expect("counts").len(), 256);
    assert_eq!(histogram["min"], 410);
    assert_eq!(histogram["max"], 55561);

    let samples = histogram["samples"].as_u64().expect("samples");
    assert_eq!(samples, 3 * 256 * 256, "one whole level-2 timepoint");
    assert!(
        samples < 3 * 1024 * 1024,
        "a full-resolution read would be {} voxels",
        3 * 1024 * 1024
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the settings popover must not stall: {elapsed:?}"
    );
}
