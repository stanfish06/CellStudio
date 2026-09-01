//! Mask edits over HTTP: the store's lifetime, the write path's visibility to the read
//! routes, undo and redo, and the session fence in front of all of them.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use support::{Binary, Server, copy_tree, data_copy, store_snapshot};

/// The tiny data: TCZYX 4x2x4x32x32 uint16, three XY-only levels, voxel 2.0x0.5x0.5 um.
const EVENT_WINDOW: Duration = Duration::from_secs(5);

fn tiny(dir: &tempfile::TempDir) -> (Server, PathBuf) {
    let dataset = data_copy(dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    (server, dataset)
}

fn labels_root(dataset: &Path) -> PathBuf {
    let mut root = dataset.to_path_buf();
    root.set_extension("cellstudio");
    root.join("labels.zarr")
}

fn reserve(server: &Server, count: u32) -> u32 {
    server.mutate("/mask/reserve", json!({ "count": count }))["first"]
        .as_u64()
        .expect("first id") as u32
}

/// A disk in one XY slice, centred on `[z, y, x]`.
fn stroke_body(t: u64, label: u32, centre: [f64; 3], radius: f64) -> Value {
    json!({
        "t": t,
        "label": label,
        "mode": "paint",
        "radius": radius,
        "plane": "z",
        "stamps": [centre],
        "only": null,
    })
}

/// The label plane the renderer would draw, as u32 ids.
fn label_plane(server: &Server, t: u64, z: u64) -> Vec<u32> {
    let plane = Binary::read(
        server
            .get(&format!(
                "/slice?layer=labels&axis=xy&t={t}&cs=0&pos={z}&level=0"
            ))
            .send()
            .expect("request"),
    );
    assert_eq!(plane.shape, vec![1, 32, 32]);
    plane.u32_values()
}

fn count_of(plane: &[u32], label: u32) -> usize {
    plane.iter().filter(|value| **value == label).count()
}

#[test]
fn a_stroke_on_a_project_with_no_labels_creates_the_store_and_becomes_readable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    assert_eq!(server.json("/project")["hasLabels"], false);
    assert!(!labels_root(&dataset).exists());

    let label = reserve(&server, 8);
    let result = server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));

    assert_eq!(result["hasLabels"], true, "the store exists from here on");
    assert!(result["seq"].as_i64().expect("seq") > 0);
    assert!(result["version"].as_u64().expect("version") > 0);
    assert_eq!(result["sessionId"], server.session().as_str());
    assert!(!result["chunks"].as_array().expect("chunks").is_empty());
    assert!(labels_root(&dataset).exists(), "labels.zarr was created");

    let painted = result["cells"][0]["area"].as_u64().expect("area");
    assert!(painted > 0);
    assert_eq!(result["cells"][0]["id"].as_u64().expect("id"), label as u64);
    assert_eq!(result["removed"].as_array().expect("removed").len(), 0);

    let plane = label_plane(&server, 0, 1);
    assert_eq!(
        count_of(&plane, label) as u64,
        painted,
        "the plane holds exactly the voxels the commit reported"
    );
    // the disk is round in x and y and pinned to one z
    assert_eq!(count_of(&label_plane(&server, 0, 0), label), 0);
    assert_eq!(count_of(&label_plane(&server, 1, 1), label), 0);
}

#[test]
fn a_stroke_into_a_store_that_already_holds_masks_leaves_every_other_label_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let first = reserve(&server, 8);

    server.mutate("/mask/stroke", stroke_body(0, first, [1.5, 8.5, 8.5], 3.0));
    server.mutate(
        "/mask/stroke",
        stroke_body(1, first + 1, [1.5, 8.5, 8.5], 3.0),
    );
    let before_second = label_plane(&server, 0, 1);
    let untouched_frame = label_plane(&server, 1, 1);
    let store = store_snapshot(&labels_root(&dataset));

    let result = server.mutate(
        "/mask/stroke",
        stroke_body(0, first + 2, [1.5, 24.5, 24.5], 3.0),
    );

    let after = label_plane(&server, 0, 1);
    assert_eq!(
        count_of(&after, first),
        count_of(&before_second, first),
        "the neighbouring label keeps every voxel"
    );
    assert!(count_of(&after, first + 2) > 0);
    assert_eq!(
        label_plane(&server, 1, 1),
        untouched_frame,
        "another frame is not touched"
    );

    let chunks: Vec<String> = result["chunks"]
        .as_array()
        .expect("chunks")
        .iter()
        .map(|key| key.as_str().expect("chunk key").to_owned())
        .collect();
    for (path, bytes) in changed(&store, &store_snapshot(&labels_root(&dataset))) {
        let key = path.to_string_lossy().replace('\\', "/");
        assert!(
            chunks.contains(&key),
            "{key} changed ({} bytes) but the commit did not report it",
            bytes.len()
        );
    }
}

#[test]
fn reserve_alone_creates_no_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let first = reserve(&server, 64);
    assert!(first >= 1);
    assert!(
        !labels_root(&dataset).exists(),
        "selecting the brush leaves nothing behind on a project the user never paints"
    );
    assert_eq!(server.json("/project")["hasLabels"], false);
    // reservation is monotonic across requests
    assert_eq!(reserve(&server, 4), first + 64);
}

#[test]
fn an_erase_leaves_the_neighbouring_label_and_drops_the_cell_it_empties() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let first = reserve(&server, 8);
    server.mutate("/mask/stroke", stroke_body(0, first, [1.5, 8.5, 8.5], 3.0));
    let painted = server.mutate(
        "/mask/stroke",
        stroke_body(0, first + 1, [1.5, 20.5, 20.5], 3.0),
    );
    let neighbour = count_of(&label_plane(&server, 0, 1), first);

    // scoped to one label: the eraser follows the selection the same way the brush does
    let scoped = server.mutate(
        "/mask/stroke",
        json!({
            "t": 0, "label": first + 1, "mode": "erase", "radius": 3.0,
            "plane": "z", "stamps": [[1.5, 20.5, 20.5]], "only": first + 1,
        }),
    );
    let plane = label_plane(&server, 0, 1);
    assert_eq!(count_of(&plane, first + 1), 0);
    assert_eq!(
        count_of(&plane, first),
        neighbour,
        "the neighbour is intact"
    );
    assert_eq!(
        scoped["removed"].as_array().expect("removed"),
        &vec![json!(first + 1)],
        "its last voxel erased, the cell record goes with it"
    );
    assert!(painted["cells"][0]["area"].as_u64().expect("area") > 0);
}

#[test]
fn an_adopted_store_opens_and_its_cells_are_measured_whole_rather_than_by_this_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = data_copy(&dir, "tiny_v2", "image.zarr");
    let adopted = dir.path().join("adopted.zarr");
    copy_tree(&source, &adopted);
    let painted;
    let label;
    {
        let server = Server::without_proxy();
        server.open_project(&source);
        label = reserve(&server, 8);
        let result = server.mutate(
            "/mask/stroke",
            stroke_body(0, label, [1.5, 16.5, 16.5], 4.0),
        );
        painted = result["cells"][0]["area"].as_u64().expect("area");
    }

    // the same store in a project that never wrote it: no cells, no mask_extent, no counter
    let store = labels_root(&adopted);
    std::fs::create_dir_all(store.parent().expect("project root")).expect("project root");
    copy_tree(&labels_root(&source), &store);

    let server = Server::without_proxy();
    server.open_project(&adopted);
    assert_eq!(server.json("/project")["hasLabels"], true);
    assert_eq!(
        count_of(&label_plane(&server, 0, 1), label) as u64,
        painted,
        "the overlay renders at open"
    );
    // the adoption inventory records every (t, label) before any edit is allowed
    let jobs = server.await_jobs(EVENT_WINDOW);
    assert!(
        jobs.iter()
            .any(|job| job["kind"] == "inventory" && job["status"] == "done"),
        "an adopted store is inventoried at open: {jobs:?}"
    );
    let cells = server.json("/cells?t0=0&t1=0");
    assert_eq!(
        cells
            .as_array()
            .expect("cells")
            .iter()
            .map(|cell| (cell["id"].as_u64(), cell["area"].as_u64()))
            .collect::<Vec<_>>(),
        vec![(Some(u64::from(label)), Some(painted))],
        "the inventory measured the adopted cell whole"
    );

    // painting over part of it measures what is left of the adopted cell, not the overlap
    let over = reserve(&server, 8) + 4;
    let result = server.mutate("/mask/stroke", stroke_body(0, over, [1.5, 16.5, 16.5], 2.0));
    let plane = label_plane(&server, 0, 1);
    let remaining = count_of(&plane, label) as u64;
    assert!(remaining > 0 && remaining < painted);

    let cells = server.json("/cells?t0=0&t1=0");
    let adopted_cell = cells
        .as_array()
        .expect("cells")
        .iter()
        .find(|cell| cell["id"] == label)
        .unwrap_or_else(|| panic!("the overwritten cell is recorded: {cells}"));
    assert_eq!(
        adopted_cell["area"].as_u64().expect("area"),
        remaining,
        "the extent was seeded from a scan of the store, not from this session's voxels"
    );
    assert_eq!(
        result["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .find(|cell| cell["id"] == over)
            .and_then(|cell| cell["area"].as_u64()),
        Some(count_of(&plane, over) as u64)
    );
}

#[test]
fn a_label_store_that_fails_the_contract_refuses_the_project_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    {
        let server = Server::without_proxy();
        server.open_project(&dataset);
        let label = reserve(&server, 4);
        server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));
    }

    // every level, so the store stays self-consistent and it is the label contract that
    // refuses it rather than the dataset reader
    let root = labels_root(&dataset);
    for level in 0..3 {
        let path = root.join(level.to_string()).join("zarr.json");
        let mut broken: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("level")).expect("json");
        broken["data_type"] = json!("uint16");
        std::fs::write(&path, broken.to_string()).expect("write");
    }

    let server = Server::without_proxy();
    let response = server.try_open_project(&dataset);
    let status = response.status();
    let body = response.text().expect("body");
    assert_eq!(status, 400, "a store that is not u32 refuses the project");
    assert!(
        body.contains("labels.zarr") && body.contains("u32"),
        "the error names the store and the failed check: {body}"
    );

    // the same project without that store opens
    std::fs::remove_dir_all(&root).expect("remove the store");
    server.open_project(&dataset);
    assert_eq!(server.json("/project")["hasLabels"], false);
}

#[test]
fn a_label_store_missing_a_level_refuses_the_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    {
        let server = Server::without_proxy();
        server.open_project(&dataset);
        let label = reserve(&server, 4);
        server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));
    }

    let root = labels_root(&dataset);
    let group = root.join("zarr.json");
    let mut attributes: Value =
        serde_json::from_str(&std::fs::read_to_string(&group).expect("group")).expect("json");
    let datasets = attributes["attributes"]["ome"]["multiscales"][0]["datasets"]
        .as_array_mut()
        .expect("datasets");
    datasets.pop();
    std::fs::write(&group, attributes.to_string()).expect("write");

    let server = Server::without_proxy();
    let response = server.try_open_project(&dataset);
    let status = response.status();
    let body = response.text().expect("body");
    assert_eq!(status, 400);
    assert!(
        body.contains("level") && body.contains("labels.zarr"),
        "the error names the store and the failed check: {body}"
    );
}

#[test]
fn a_stroke_publishes_one_invalidate_and_one_versions_that_name_the_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let session = server.session();
    let label = reserve(&server, 4);

    let ticket = server.ws_ticket();
    let mut events = server.connect_events(&ticket).expect("events");
    assert_eq!(events.next_event(EVENT_WINDOW)["type"], "versions");

    let result = server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));
    let frames = events.drain(Duration::from_millis(400));
    let kinds: Vec<&str> = frames
        .iter()
        .map(|frame| frame["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        vec!["invalidate", "versions"],
        "one invalidation and one version frame, in that order: {frames:?}"
    );
    assert_eq!(frames[0]["sessionId"], session.as_str());
    assert_eq!(frames[0]["layer"], "labels");
    assert_eq!(frames[0]["version"], result["version"]);
    assert_eq!(frames[0]["chunks"], result["chunks"]);
    assert_eq!(frames[1]["versions"]["sessionId"], session.as_str());
    assert_eq!(frames[1]["versions"]["labels"], result["version"]);
}

#[test]
fn undo_returns_the_chunks_to_their_prior_bytes_and_redo_restores_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let label = reserve(&server, 4);
    server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));
    let before = store_snapshot(&labels_root(&dataset));
    let plane_before = label_plane(&server, 0, 1);

    let second = server.mutate(
        "/mask/stroke",
        stroke_body(0, label + 1, [1.5, 24.5, 24.5], 3.0),
    );
    let after = store_snapshot(&labels_root(&dataset));
    let plane_after = label_plane(&server, 0, 1);
    assert_ne!(before, after);

    let undone = server.mutate("/edits/undo", json!({}));
    assert_eq!(undone["seq"], second["seq"]);
    assert_eq!(
        store_snapshot(&labels_root(&dataset)),
        before,
        "byte-identical at every level"
    );
    assert_eq!(label_plane(&server, 0, 1), plane_before);
    assert_eq!(
        undone["removed"].as_array().expect("removed"),
        &vec![json!(label + 1)],
        "the cell the stroke created no longer exists"
    );
    assert!(
        server
            .json("/cells?t0=0&t1=0")
            .as_array()
            .expect("cells")
            .len()
            == 1
    );

    let redone = server.mutate("/edits/redo", json!({}));
    assert_eq!(redone["seq"], second["seq"]);
    assert_eq!(store_snapshot(&labels_root(&dataset)), after);
    assert_eq!(label_plane(&server, 0, 1), plane_after);
    assert_eq!(
        server
            .json("/cells?t0=0&t1=0")
            .as_array()
            .expect("cells")
            .len(),
        2
    );

    // a new edit clears the redo stack
    server.mutate("/edits/undo", json!({}));
    server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 4.5, 4.5], 2.0));
    let refused = server.post_as("/edits/redo", &server.session(), &json!({}));
    assert_eq!(refused.status(), 409, "the undone stroke cannot reappear");
}

#[test]
fn the_history_reports_a_pruned_entry_rather_than_failing_at_the_attempt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let first = reserve(&server, 64);
    for i in 0..3 {
        server.mutate(
            "/mask/stroke",
            stroke_body(0, first + i, [1.5, 8.5, 8.5], 2.0),
        );
    }
    let entries = server.json("/edits?limit=10");
    let entries = entries.as_array().expect("edits");
    assert_eq!(entries.len(), 3);
    for entry in entries {
        assert_eq!(entry["undoable"], true, "the newest 50 stay undoable");
        assert_eq!(entry["domain"], "mask");
        assert_eq!(entry["scope"], "stroke");
    }
}

#[test]
fn delete_clears_the_frame_and_leaves_the_neighbouring_frame_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let first = reserve(&server, 8);
    server.mutate("/mask/stroke", stroke_body(0, first, [1.5, 8.5, 8.5], 3.0));
    server.mutate(
        "/mask/stroke",
        stroke_body(1, first + 1, [1.5, 8.5, 8.5], 3.0),
    );
    let neighbour = label_plane(&server, 1, 1);

    let deleted = server.mutate("/mask/delete", json!({ "t": 0, "label": first }));
    assert_eq!(
        deleted["removed"].as_array().expect("removed"),
        &vec![json!(first)]
    );
    assert_eq!(count_of(&label_plane(&server, 0, 1), first), 0);
    assert_eq!(label_plane(&server, 1, 1), neighbour);
    assert_eq!(
        server
            .json("/cells?t0=0&t1=1")
            .as_array()
            .expect("cells")
            .len(),
        1,
        "the cell record went with the voxels"
    );

    let undone = server.mutate("/edits/undo", json!({}));
    assert!(count_of(&label_plane(&server, 0, 1), first) > 0);
    assert_eq!(
        undone["cells"][0]["id"].as_u64().expect("id"),
        first as u64,
        "one undo restores the voxels and the record together"
    );
}

#[test]
fn an_unreserved_id_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let session = server.session();

    let response = server.post_as(
        "/mask/stroke",
        &session,
        &stroke_body(0, 9_999, [1.5, 8.5, 8.5], 3.0),
    );
    assert_eq!(response.status(), 409);
    assert!(response.text().expect("body").contains("reserve"));
    assert_eq!(server.json("/edits").as_array().expect("edits").len(), 0);

    // an id painted earlier on that frame needs no new lease
    let label = reserve(&server, 1);
    server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));
    server.mutate(
        "/mask/stroke",
        stroke_body(0, label, [1.5, 12.5, 12.5], 3.0),
    );
    assert!(labels_root(&dataset).exists());

    // one id, one frame: the same id at another t would move the cell
    let moved = server.post_as(
        "/mask/stroke",
        &session,
        &stroke_body(2, label, [1.5, 8.5, 8.5], 3.0),
    );
    assert_eq!(moved.status(), 409);
    assert!(moved.text().expect("body").contains("one id, one frame"));
}

#[test]
fn concurrent_strokes_serialize_rather_than_interleave() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let session = server.session();
    let first = reserve(&server, 16);
    // one level-0 chunk holds this whole frame, so an interleaved read-modify-write loses
    // whichever stroke committed first
    let centres = [
        [1.5, 4.5, 4.5],
        [1.5, 12.5, 4.5],
        [1.5, 20.5, 4.5],
        [1.5, 28.5, 4.5],
    ];

    std::thread::scope(|scope| {
        for (i, centre) in centres.iter().enumerate() {
            let server = &server;
            let session = session.as_str();
            scope.spawn(move || {
                let body = stroke_body(0, first + i as u32, *centre, 2.0);
                let response = server.post_as("/mask/stroke", session, &body);
                assert!(response.status().is_success(), "{:?}", response.text());
            });
        }
    });

    let plane = label_plane(&server, 0, 1);
    for i in 0..centres.len() as u32 {
        assert!(
            count_of(&plane, first + i) > 0,
            "label {} was lost to an interleaved write",
            first + i
        );
    }
}

#[test]
fn a_committed_stroke_is_immediately_visible_to_pixel_and_slice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let label = reserve(&server, 4);
    server.mutate("/mask/stroke", stroke_body(0, label, [1.5, 8.5, 8.5], 3.0));

    // warm every read path on the pre-edit bytes, which is what makes a missing brick-cache
    // eviction visible.
    let pixel = |z: u64, y: u64, x: u64| {
        server.json(&format!("/pixel?layer=labels&t=0&c=0&z={z}&y={y}&x={x}"))["value"]
            .as_u64()
            .expect("value")
    };
    let volume = |server: &Server| {
        Binary::read(
            server
                .get("/volume?layer=labels&t=0&c=0&level=0")
                .send()
                .expect("request"),
        )
        .u32_values()
    };
    assert_eq!(pixel(1, 24, 24), 0);
    assert_eq!(count_of(&label_plane(&server, 0, 1), label + 1), 0);
    assert!(!volume(&server).contains(&(label + 1)));

    server.mutate(
        "/mask/stroke",
        stroke_body(0, label + 1, [1.5, 24.5, 24.5], 3.0),
    );

    assert_eq!(
        pixel(1, 24, 24),
        u64::from(label + 1),
        "/pixel sees the committed edit rather than the brick it cached"
    );
    assert!(count_of(&label_plane(&server, 0, 1), label + 1) > 0);
    assert!(
        volume(&server).contains(&(label + 1)),
        "/volume sees it too, from the same bricks"
    );
}

#[test]
fn a_mutation_without_a_session_identifier_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    for (path, body) in [
        ("/mask/reserve", json!({ "count": 4 })),
        ("/mask/stroke", stroke_body(0, 1, [1.5, 8.5, 8.5], 3.0)),
        ("/mask/delete", json!({ "t": 0, "label": 1 })),
        ("/edits/undo", json!({})),
        ("/edits/redo", json!({})),
    ] {
        let response = server.post(path).json(&body).send().expect("request");
        assert_eq!(response.status(), 400, "POST {path} with no session header");
        assert!(
            response
                .text()
                .expect("body")
                .contains("x-cellstudio-session")
        );
    }
    assert!(!labels_root(&dataset).exists(), "nothing was written");
    assert_eq!(server.json("/edits").as_array().expect("edits").len(), 0);
}

#[test]
fn a_stale_session_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let stale = server.session();
    let other = data_copy(&dir, "tiny_v3", "image.zarr");
    server.open_project(&other);
    let current = server.session();
    assert_ne!(stale, current);

    let response = server.post_as(
        "/mask/stroke",
        &stale,
        &stroke_body(0, 1, [1.5, 8.5, 8.5], 3.0),
    );
    assert_eq!(response.status(), 409);
    assert_eq!(
        response
            .headers()
            .get("x-cellstudio-session")
            .and_then(|value| value.to_str().ok()),
        Some(current.as_str()),
        "the refusal names the session that is actually open"
    );
    let body = response.text().expect("body");
    assert!(
        body.contains(&stale),
        "the refusal names the session it was fenced on: {body}"
    );

    assert!(
        !labels_root(&dataset).exists(),
        "the old project is untouched"
    );
    assert!(!labels_root(&other).exists(), "and so is the new one");
    assert_eq!(server.json("/edits").as_array().expect("edits").len(), 0);
}

#[test]
fn a_reservation_from_the_old_session_cannot_authorize_a_stroke_in_the_new_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let leased = reserve(&server, 8);

    // a same-store reopen: the same labels.zarr, a new session, and no lease
    server.open_project(&dataset);
    let reopened = server.session();
    let response = server.post_as(
        "/mask/stroke",
        &reopened,
        &stroke_body(0, leased, [1.5, 8.5, 8.5], 3.0),
    );
    assert_eq!(response.status(), 409);
    assert!(response.text().expect("body").contains("reserve"));

    // and the reopened session serializes on the same lock over the same store
    let first = reserve(&server, 8);
    let session = server.session();
    std::thread::scope(|scope| {
        for i in 0..4_u32 {
            let server = &server;
            let session = session.as_str();
            scope.spawn(move || {
                let centre = [1.5, 4.5 + f64::from(i) * 8.0, 4.5];
                let body = stroke_body(0, first + i, centre, 2.0);
                assert!(
                    server
                        .post_as("/mask/stroke", session, &body)
                        .status()
                        .is_success()
                );
            });
        }
    });
    let plane = label_plane(&server, 0, 1);
    for i in 0..4 {
        assert!(
            count_of(&plane, first + i) > 0,
            "label {} was lost",
            first + i
        );
    }
}

/// Files that differ between two store snapshots, keyed by their path under the store.
fn changed(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> BTreeMap<PathBuf, Vec<u8>> {
    after
        .iter()
        .filter(|(path, bytes)| before.get(*path) != Some(*bytes))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}
