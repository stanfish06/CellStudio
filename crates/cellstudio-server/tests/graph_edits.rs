//! Graph mutations over HTTP: link/unlink, the discriminated `EditResult`, undo/redo
//! dispatch by journal domain, and the mask-delete → graph re-materialization cascade.

mod support;

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use support::{Binary, Server, data_copy, store_snapshot};

const EVENT_WINDOW: Duration = Duration::from_secs(5);

fn tiny(dir: &tempfile::TempDir) -> (Server, PathBuf) {
    let dataset = data_copy(dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    (server, dataset)
}

fn reserve(server: &Server, count: u32) -> u32 {
    server.mutate("/mask/reserve", json!({ "count": count }))["first"]
        .as_u64()
        .expect("first id") as u32
}

fn paint_at(server: &Server, t: u64, label: u32, centre: [f64; 3]) -> Value {
    server.mutate(
        "/mask/stroke",
        json!({
            "t": t,
            "label": label,
            "mode": "paint",
            "radius": 3.0,
            "plane": "z",
            "stamps": [centre],
            "only": null,
        }),
    )
}

fn paint(server: &Server, t: u64, label: u32) -> Value {
    paint_at(server, t, label, [1.5, 8.5, 8.5])
}

fn track_of(cells: &[Value], id: u32) -> Option<u64> {
    cells
        .iter()
        .find(|cell| cell["id"] == u64::from(id))
        .unwrap_or_else(|| panic!("cell {id} is not listed: {cells:?}"))["trackId"]
        .as_u64()
}

fn parent_of(cells: &[Value], id: u32) -> Option<u64> {
    cells
        .iter()
        .find(|cell| cell["id"] == u64::from(id))
        .unwrap_or_else(|| panic!("cell {id} is not listed: {cells:?}"))["parentId"]
        .as_u64()
}

fn cells(server: &Server) -> Vec<Value> {
    server
        .json("/cells?t0=0&t1=3")
        .as_array()
        .cloned()
        .expect("cells")
}

fn label_plane(server: &Server, t: u64, z: u64) -> Vec<u32> {
    Binary::read(
        server
            .get(&format!(
                "/slice?layer=labels&axis=xy&t={t}&cs=0&pos={z}&level=0"
            ))
            .send()
            .expect("request"),
    )
    .u32_values()
}

#[test]
fn mask_then_graph_then_undo_undo_redo_redo_dispatch_by_domain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let session = server.session();
    let first = reserve(&server, 8);
    let (a, b, c) = (first, first + 1, first + 2);

    let painted = paint(&server, 0, a);
    assert_eq!(painted["domain"], "mask", "mask results carry their domain");
    paint(&server, 1, b);
    paint(&server, 2, c);

    // graph: a → b, then b → c joins the chain under one identity
    let linked = server.mutate("/graph/link", json!({ "parentId": a, "childId": b }));
    assert_eq!(linked["domain"], "graph");
    assert_eq!(linked["sessionId"], session.as_str());
    assert!(linked["seq"].as_i64().expect("seq") > 0);
    let v1 = linked["graphVersion"].as_u64().expect("graphVersion");
    let affected = linked["affectedCells"].as_array().expect("affectedCells");
    assert_eq!(affected.len(), 2);
    let track = affected[0]["trackId"].as_u64().expect("a fresh track id");
    assert!(affected.iter().all(|cell| cell["trackId"] == track));
    assert_eq!(
        linked["affectedTracks"],
        json!([track]),
        "one identity for the joined chain"
    );

    let joined = server.mutate("/graph/link", json!({ "parentId": b, "childId": c }));
    assert!(joined["graphVersion"].as_u64().expect("graphVersion") > v1);
    let listed = cells(&server);
    assert_eq!(
        parent_of(&listed, a),
        None,
        "the chain's root has no parent"
    );
    assert_eq!(parent_of(&listed, b), Some(u64::from(a)));
    assert_eq!(parent_of(&listed, c), Some(u64::from(b)));
    assert_eq!(track_of(&listed, a), Some(track));
    assert_eq!(track_of(&listed, b), Some(track));
    assert_eq!(
        track_of(&listed, c),
        Some(track),
        "the join propagates downstream"
    );

    // the journal lists both domains
    let domains: Vec<Value> = server
        .json("/edits")
        .as_array()
        .expect("edits")
        .iter()
        .map(|entry| entry["domain"].clone())
        .collect();
    assert!(domains.contains(&json!("graph")) && domains.contains(&json!("mask")));

    // undo × 3: the two graph rows, then the newest mask row — dispatched by domain
    let undo1 = server.mutate("/edits/undo", json!({}));
    assert_eq!(undo1["domain"], "graph");
    assert_eq!(
        track_of(&cells(&server), c),
        None,
        "a join's undo restores the old identity, which was untracked"
    );
    let undo2 = server.mutate("/edits/undo", json!({}));
    assert_eq!(undo2["domain"], "graph");
    let listed = cells(&server);
    assert_eq!(track_of(&listed, a), None);
    assert_eq!(track_of(&listed, b), None);

    let undo3 = server.mutate("/edits/undo", json!({}));
    assert_eq!(
        undo3["domain"], "mask",
        "the next row down is the paint of c"
    );
    assert_eq!(undo3["removed"], json!([c]));

    // redo × 3 walks back up in order: mask, then the two graph rows
    let redo1 = server.mutate("/edits/redo", json!({}));
    assert_eq!(redo1["domain"], "mask");
    let redo2 = server.mutate("/edits/redo", json!({}));
    assert_eq!(redo2["domain"], "graph");
    let redo3 = server.mutate("/edits/redo", json!({}));
    assert_eq!(redo3["domain"], "graph");

    let listed = cells(&server);
    assert_eq!(track_of(&listed, a), Some(track));
    assert_eq!(track_of(&listed, b), Some(track));
    assert_eq!(
        track_of(&listed, c),
        Some(track),
        "redo reapplies the exact after-assignments"
    );
}

#[test]
fn rejected_links_return_the_reason_and_leave_the_graph_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let session = server.session();
    let first = reserve(&server, 8);
    paint_at(&server, 1, first, [1.5, 8.5, 8.5]);
    paint_at(&server, 1, first + 1, [1.5, 24.5, 24.5]);

    let response = server.post_as(
        "/graph/link",
        &session,
        &json!({ "parentId": first, "childId": first + 1 }),
    );
    assert_eq!(response.status().as_u16(), 409);
    let body: Value = serde_json::from_str(&response.text().expect("body")).expect("json");
    let reason = body["error"].as_str().expect("reason");
    assert!(
        reason.contains("forward in time"),
        "unexpected reason: {reason}"
    );

    let response = server.post_as("/graph/unlink", &session, &json!({ "cellId": 424242 }));
    assert_eq!(response.status().as_u16(), 404);

    assert!(
        cells(&server).iter().all(|cell| cell["trackId"].is_null()),
        "rejections left every cell untracked"
    );
}

#[test]
fn mask_delete_of_a_linked_cell_rematerializes_neighbors_and_one_undo_restores_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, dataset) = tiny(&dir);
    let session = server.session();
    let first = reserve(&server, 8);
    let (a, b, c) = (first, first + 1, first + 2);
    paint(&server, 0, a);
    paint(&server, 1, b);
    paint(&server, 2, c);
    server.mutate("/graph/link", json!({ "parentId": a, "childId": b }));
    server.mutate("/graph/link", json!({ "parentId": b, "childId": c }));
    let track = track_of(&cells(&server), a).expect("tracked");

    let mut labels_root = dataset.clone();
    labels_root.set_extension("cellstudio");
    let labels_root = labels_root.join("labels.zarr");
    let before_store = store_snapshot(&labels_root);
    let before_plane = label_plane(&server, 1, 1);
    assert!(before_plane.contains(&b));

    let ticket = server.ws_ticket();
    let mut events = server.connect_events(&ticket).expect("events");

    let deleted = server.mutate("/mask/delete", json!({ "t": 1, "label": b }));
    assert_eq!(deleted["domain"], "mask");
    assert_eq!(deleted["removed"], json!([b]));
    assert!(deleted["version"].as_u64().expect("labels version") > 0);
    let graph_version = deleted["graphVersion"]
        .as_u64()
        .expect("a topology-changing mask commit carries version.graph too");
    let affected = deleted["affectedTracks"]
        .as_array()
        .expect("affectedTracks");
    assert!(!affected.is_empty());

    // the session-scoped graphChanged rides beside the invalidation and versions events
    let changed = events.next_event_of("graphChanged", EVENT_WINDOW);
    assert_eq!(changed["sessionId"], session.as_str());
    assert_eq!(changed["graphVersion"], graph_version);
    let versions = events.next_event_of("versions", EVENT_WINDOW);
    assert_eq!(versions["versions"]["graph"], graph_version);

    let listed = cells(&server);
    assert!(!listed.iter().any(|cell| cell["id"] == u64::from(b)));
    assert_eq!(
        track_of(&listed, a),
        Some(track),
        "the upstream chain keeps its id"
    );
    let orphan = track_of(&listed, c).expect("still tracked");
    assert_ne!(
        orphan, track,
        "the orphaned child re-heads under a fresh id"
    );
    assert_eq!(
        server.json(&format!("/lineage?cell={a}"))["cells"]
            .as_array()
            .expect("lineage")
            .len(),
        1,
        "the lineage no longer reaches past the deleted cell"
    );

    // one undo restores pixels, links, and every neighbor's identity exactly
    let undone = server.mutate("/edits/undo", json!({}));
    assert_eq!(undone["domain"], "mask");
    assert!(undone["graphVersion"].as_u64().expect("graph bumped again") > graph_version);
    assert_eq!(
        store_snapshot(&labels_root),
        before_store,
        "pixels restored"
    );
    assert_eq!(label_plane(&server, 1, 1), before_plane);
    let listed = cells(&server);
    for id in [a, b, c] {
        assert_eq!(track_of(&listed, id), Some(track));
    }
    assert_eq!(
        server.json(&format!("/lineage?cell={a}"))["cells"]
            .as_array()
            .expect("lineage")
            .len(),
        3,
        "the lineage spans the restored links again"
    );
}

#[test]
fn unlink_over_http_removes_the_chain_and_is_undoable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (server, _dataset) = tiny(&dir);
    let first = reserve(&server, 8);
    let (a, b, c) = (first, first + 1, first + 2);
    paint(&server, 0, a);
    paint(&server, 1, b);
    paint(&server, 2, c);
    server.mutate("/graph/link", json!({ "parentId": a, "childId": b }));
    server.mutate("/graph/link", json!({ "parentId": b, "childId": c }));
    let track = track_of(&cells(&server), a).expect("tracked");

    let unlinked = server.mutate("/graph/unlink", json!({ "cellId": b }));
    assert_eq!(unlinked["domain"], "graph");
    let listed = cells(&server);
    // singletons: the chain head keeps the id it carried, the others draw fresh ones
    assert_eq!(track_of(&listed, a), Some(track));
    let b_track = track_of(&listed, b).expect("singleton id");
    let c_track = track_of(&listed, c).expect("singleton id");
    assert!(b_track != track && c_track != track && b_track != c_track);
    assert_eq!(
        server.json(&format!("/lineage?cell={b}"))["links"]
            .as_array()
            .expect("links")
            .len(),
        0
    );

    let undone = server.mutate("/edits/undo", json!({}));
    assert_eq!(undone["domain"], "graph");
    let listed = cells(&server);
    for id in [a, b, c] {
        assert_eq!(
            track_of(&listed, id),
            Some(track),
            "identities restored exactly"
        );
    }
    assert_eq!(
        server.json(&format!("/lineage?cell={a}"))["links"]
            .as_array()
            .expect("links")
            .len(),
        2
    );
}
