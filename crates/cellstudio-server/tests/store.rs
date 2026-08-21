//! GET /store serves stored zarr bytes from the source store even after a working copy exists.

mod support;

use std::path::Path;
use std::time::Duration;

use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use serde_json::json;

use support::{Server, dev_dataset, data_copy, skip, store_snapshot};

/// A re-chunk of a KB-sized data; generous enough to survive a loaded machine.
const JOB_TIMEOUT: Duration = Duration::from_secs(120);

fn body_of(response: reqwest::blocking::Response) -> Vec<u8> {
    let status = response.status();
    assert!(status.is_success(), "store read -> {status}");
    response.bytes().expect("body").to_vec()
}

#[test]
fn stored_objects_are_byte_identical_to_the_files_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    // group metadata, array metadata, and chunks at two levels
    for object in [
        ".zattrs",
        ".zgroup",
        "0/.zarray",
        "0/.zattrs",
        "0/0.0.0.0.0",
        "0/3.1.0.1.1",
        "2/1.0.0.0.0",
    ] {
        let expected = std::fs::read(dataset.join(object)).expect("the data file on disk");
        let response = server
            .get(&format!("/store/{object}"))
            .send()
            .expect("request");
        assert_eq!(response.status(), 200, "GET /store/{object}");
        let declared = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("/store/{object} declares no content-length"));
        let served = body_of(response);
        assert_eq!(
            declared,
            expected.len(),
            "/store/{object} content-length must be the file's length"
        );
        assert_eq!(
            served,
            expected,
            "/store/{object} must be the stored bytes verbatim ({} served, {} on disk)",
            served.len(),
            expected.len()
        );
    }
}

#[test]
fn zarr_v3_metadata_and_chunks_are_served_unmodified() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v3", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    for object in ["zarr.json", "0/zarr.json", "0/c/0/0/0/0/0", "2/c/3/1/0/0/0"] {
        let expected = std::fs::read(dataset.join(object)).expect("the data file on disk");
        let served = body_of(
            server
                .get(&format!("/store/{object}"))
                .send()
                .expect("request"),
        );
        assert_eq!(served, expected, "/store/{object} must be unmodified");
    }

    // the metadata a standard zarr client resolves shape, dtype and chunk grid from
    let root: serde_json::Value = serde_json::from_slice(&body_of(
        server.get("/store/zarr.json").send().expect("request"),
    ))
    .expect("root zarr.json is JSON");
    assert_eq!(root["node_type"], "group", "{root}");
}

#[test]
fn head_returns_the_headers_without_a_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    for object in ["0/.zarray", "0/0.0.0.0.0"] {
        let expected = std::fs::read(dataset.join(object)).expect("the data file on disk");
        let response = server
            .head(&format!("/store/{object}"))
            .send()
            .expect("request");
        assert_eq!(response.status(), 200, "HEAD /store/{object}");
        assert_eq!(
            response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok()),
            Some(expected.len().to_string().as_str()),
            "HEAD /store/{object} must size the object"
        );
        assert_eq!(
            response
                .headers()
                .get("accept-ranges")
                .and_then(|v| v.to_str().ok()),
            Some("bytes"),
            "HEAD /store/{object} must advertise range support"
        );
        assert!(
            response.bytes().expect("body").is_empty(),
            "HEAD /store/{object} must carry no body"
        );
    }
}

#[test]
fn a_range_request_returns_206_with_the_requested_slice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let object = "0/0.0.0.0.0";
    let file = std::fs::read(dataset.join(object)).expect("the data file on disk");
    let total = file.len();
    assert!(total > 32, "the data chunk is too small to range into");

    for (header, first, last) in [
        ("bytes=4-19", 4_usize, 19_usize),
        ("bytes=0-0", 0, 0),
        ("bytes=-8", total - 8, total - 1),
        (
            // an end past the object clamps to the last byte rather than failing
            "bytes=8-999999",
            8,
            total - 1,
        ),
    ] {
        let response = server
            .get(&format!("/store/{object}"))
            .header(RANGE, header)
            .send()
            .expect("request");
        assert_eq!(response.status(), 206, "GET /store/{object} {header}");
        assert_eq!(
            response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some(format!("bytes {first}-{last}/{total}").as_str()),
            "{header} must name the span it served"
        );
        assert_eq!(
            body_of(response).as_slice(),
            &file[first..=last],
            "{header} must be the stored slice"
        );
    }

    let past_the_end = server
        .get(&format!("/store/{object}"))
        .header(RANGE, format!("bytes={total}-"))
        .send()
        .expect("request");
    assert_eq!(
        past_the_end.status(),
        416,
        "a range starting past the object is unsatisfiable, not a whole-object 200"
    );
    assert_eq!(
        past_the_end
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some(format!("bytes */{total}").as_str())
    );
}

#[test]
fn an_absent_chunk_is_404_so_the_client_can_fill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    for absent in ["0/9.9.9.9.9", "0/0.0.0.9.0", "7/.zarray", "not-a-key"] {
        let response = server
            .get(&format!("/store/{absent}"))
            .send()
            .expect("request");
        assert_eq!(response.status(), 404, "GET /store/{absent}");
    }

    // a directory is not an object either
    assert_eq!(
        server.get("/store/0").send().expect("request").status(),
        404,
        "a chunk directory is not a stored object"
    );
}

#[test]
fn a_store_path_cannot_climb_out_of_the_layer_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, b"not part of the store").expect("write the decoy");

    let server = Server::without_proxy();
    server.open_project(&dataset);

    for hostile in [
        "..%2fsecret.txt",
        "0%2f..%2f..%2fsecret.txt",
        "%2e%2e/secret.txt",
    ] {
        let response = server
            .get(&format!("/store/{hostile}"))
            .send()
            .expect("request");
        assert!(
            response.status() == 400 || response.status() == 404,
            "/store/{hostile} must not resolve outside the layer root (got {})",
            response.status()
        );
        assert!(
            !body_of_or_empty(response)
                .windows(9)
                .any(|w| w == b"not part"),
            "/store/{hostile} leaked a file outside the store"
        );
    }
}

fn body_of_or_empty(response: reqwest::blocking::Response) -> Vec<u8> {
    response.bytes().map(|b| b.to_vec()).unwrap_or_default()
}

#[test]
fn the_store_needs_an_open_project_and_an_existing_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();

    assert_eq!(
        server
            .get("/store/.zattrs")
            .send()
            .expect("request")
            .status(),
        404,
        "no project is open yet"
    );

    server.open_project(&dataset);
    assert_eq!(
        server
            .get("/store/.zattrs?layer=labels")
            .send()
            .expect("request")
            .status(),
        404,
        "the project has no label store until masks are imported"
    );
}

/// The rule the whole two-path design rests on: re-chunking moves the *assembled*
/// reads onto the brick working copy and leaves the raw passthrough on the source, so the
/// renderer's zarr client only ever meets the source's layout.
#[test]
fn the_store_serves_the_source_even_after_a_rechunk_is_adopted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    let opened = server.open_project(&dataset);
    assert_eq!(
        opened["levels"][0]["chunks"],
        json!({"t": 1, "c": 1, "z": 4, "y": 16, "x": 16}),
        "the data's stored chunking"
    );

    let source_zarray = std::fs::read(dataset.join("0/.zarray")).expect("source .zarray");
    let source_chunk = std::fs::read(dataset.join("0/0.0.0.0.0")).expect("source chunk");

    let started = server
        .post("/rechunk")
        .json(&json!({ "z": 2, "y": 8, "x": 8 }))
        .send()
        .expect("rechunk");
    assert!(
        started.status().is_success(),
        "POST /rechunk -> {}",
        started.status()
    );

    let jobs = server.await_jobs(JOB_TIMEOUT);
    let rechunk = jobs
        .iter()
        .find(|job| job["kind"] == "rechunk")
        .unwrap_or_else(|| panic!("no re-chunk job in {jobs:?}"));
    assert_eq!(
        rechunk["status"], "done",
        "re-chunk did not finish: {rechunk}"
    );

    // the assembled path switched to the working copy
    let after = server.json("/project");
    assert_eq!(
        after["levels"][0]["chunks"],
        json!({"t": 1, "c": 1, "z": 2, "y": 8, "x": 8}),
        "assembled reads must serve the brick working copy"
    );
    let working_copy = Path::new(
        after["projectPath"]
            .as_str()
            .expect("projectPath is a string"),
    )
    .join("cache/bricks.zarr");
    assert!(working_copy.is_dir(), "{working_copy:?} must exist");

    // ... and the raw path did not
    assert_eq!(
        body_of(server.get("/store/0/.zarray").send().expect("request")),
        source_zarray,
        "/store must keep serving the source's array metadata"
    );
    assert_eq!(
        body_of(server.get("/store/0/0.0.0.0.0").send().expect("request")),
        source_chunk,
        "/store must keep serving the source's chunk bytes"
    );
    // the working copy's own chunk grid is not addressable through /store at all
    assert_eq!(
        server
            .get("/store/0/0.0.1.1.1")
            .send()
            .expect("request")
            .status(),
        404,
        "a key that only exists in the working copy must not resolve"
    );
}

#[test]
fn a_rechunk_leaves_the_source_store_byte_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let before = store_snapshot(&dataset);

    let server = Server::without_proxy();
    server.open_project(&dataset);
    server
        .post("/rechunk")
        .json(&json!({ "z": 2, "y": 8, "x": 8 }))
        .send()
        .expect("rechunk");
    let jobs = server.await_jobs(JOB_TIMEOUT);
    assert!(
        jobs.iter()
            .any(|job| job["kind"] == "rechunk" && job["status"] == "done"),
        "re-chunk did not finish: {jobs:?}"
    );

    let after = store_snapshot(&dataset);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "the source store gained or lost files"
    );
    for (path, bytes) in &before {
        let served = &after[path];
        assert_eq!(
            served.len(),
            bytes.len(),
            "{path:?} changed length inside the source store"
        );
        assert!(
            served == bytes,
            "{path:?} changed content inside the source store"
        );
    }
}

#[test]
fn dev_dataset_metadata_is_byte_identical_to_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(dataset) = dev_dataset(&dir) else {
        return skip("CELLSTUDIO_DEV_DATASET is not set");
    };
    let server = Server::without_proxy();
    server.open_project(&dataset);

    for object in [".zattrs", ".zgroup", "0/.zarray", "1/.zarray", "2/.zarray"] {
        let expected = std::fs::read(dataset.join(object)).expect("the dataset file on disk");
        let served = body_of(
            server
                .get(&format!("/store/{object}"))
                .send()
                .expect("request"),
        );
        assert_eq!(served, expected, "/store/{object} must be verbatim");
    }

    // a real blosc-compressed chunk, whole and then ranged into
    let object = "0/10.0.0.0.0";
    let chunk = std::fs::read(dataset.join(object)).expect("the chunk on disk");
    assert_eq!(
        body_of(
            server
                .get(&format!("/store/{object}"))
                .send()
                .expect("request")
        ),
        chunk,
        "a compressed chunk must not be decoded server-side"
    );
    let ranged = server
        .get(&format!("/store/{object}"))
        .header(RANGE, "bytes=0-15")
        .send()
        .expect("request");
    assert_eq!(ranged.status(), 206);
    assert_eq!(
        ranged
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some(format!("bytes 0-15/{}", chunk.len()).as_str())
    );
    assert_eq!(body_of(ranged).as_slice(), &chunk[0..16]);
}
