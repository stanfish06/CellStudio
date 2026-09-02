//! Cell and track labels over HTTP: the label edit and its undo, per-definition states over
//! a chain, and the definition list with strip-on-delete.

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{Server, data, data_copy};

const EVENT_WINDOW: Duration = Duration::from_secs(5);
const JOB_WINDOW: Duration = Duration::from_secs(120);

/// tiny_v2 with the `tracking_valid` graph imported: chains per blob column, blob 0 divides
/// at t=1 (cells 1→7, then 13→19 and 18→24), `labels` on blobs 0 and 3, `track_labels`
/// `cell type 1` on cells 1 and 7.
fn imported(dir: &tempfile::TempDir) -> Server {
    let dataset = data_copy(dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    server.mutate(
        "/import/tracks",
        json!({ "path": data("tracking_valid", "tracks.json.gz") }),
    );
    let jobs = server.await_jobs(JOB_WINDOW);
    assert!(
        jobs.iter().all(|j| j["status"] == "done"),
        "import finished: {jobs:?}"
    );
    server
}

fn cells(server: &Server) -> Vec<Value> {
    server
        .json("/cells?t0=0&t1=3")
        .as_array()
        .cloned()
        .expect("cells")
}

fn cell(listed: &[Value], id: u32) -> &Value {
    listed
        .iter()
        .find(|c| c["id"] == u64::from(id))
        .unwrap_or_else(|| panic!("cell {id} missing: {listed:?}"))
}

fn names(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect()
}

fn definitions(server: &Server) -> Vec<(String, u64)> {
    server.json("/project")["labelDefinitions"]
        .as_array()
        .expect("definitions")
        .iter()
        .map(|d| {
            (
                d["name"].as_str().expect("name").to_owned(),
                d["uses"].as_u64().expect("uses"),
            )
        })
        .collect()
}

fn state_of<'a>(states: &'a [Value], name: &str) -> &'a Value {
    states
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no state for {name}: {states:?}"))
}

#[test]
fn label_edits_round_trip_over_http_and_undo_exactly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = imported(&dir);
    let ticket = server.ws_ticket();
    let mut events = server.connect_events(&ticket).expect("event socket");

    // the import seeded the vocabulary, with the unused name from metadata
    assert_eq!(
        definitions(&server),
        vec![
            ("ESI".to_owned(), 4),
            ("cell type 1".to_owned(), 2),
            ("control".to_owned(), 4),
            ("treated".to_owned(), 4),
            ("unused".to_owned(), 0),
        ]
    );
    let listed = cells(&server);
    assert_eq!(names(&cell(&listed, 1)["labels"]), vec!["ESI", "treated"]);
    assert_eq!(names(&cell(&listed, 1)["trackLabels"]), vec!["cell type 1"]);

    // cell scope on one cell
    let edit = server.mutate(
        "/graph/labels",
        json!({ "cellId": 2, "scope": "cell", "add": ["verified"] }),
    );
    assert_eq!(edit["domain"], "graph");
    let affected = edit["affectedCells"].as_array().expect("affected");
    assert_eq!(affected.len(), 1);
    assert_eq!(names(&affected[0]["labels"]), vec!["verified"]);
    assert_eq!(affected[0]["trackLabels"].as_array().map(Vec::len), Some(0));
    let changed = events.next_event_of("graphChanged", EVENT_WINDOW);
    assert_eq!(changed["sessionId"], server.session());
    assert_eq!(changed["graphVersion"], edit["graphVersion"]);

    // track scope on the daughter chain 13→19
    let edit = server.mutate(
        "/graph/labels",
        json!({ "cellId": 19, "scope": "track", "add": ["cell type 1"] }),
    );
    let mut touched: Vec<u64> = edit["affectedCells"]
        .as_array()
        .expect("affected")
        .iter()
        .map(|c| c["id"].as_u64().expect("id"))
        .collect();
    touched.sort_unstable();
    assert_eq!(touched, vec![13, 19]);

    let states = server.json("/graph/labels?cell=13");
    let states = states.as_array().expect("states");
    assert_eq!(state_of(states, "cell type 1")["track"], "all");
    assert_eq!(state_of(states, "cell type 1")["cell"], false);
    assert_eq!(state_of(states, "verified")["track"], "none");
    let states = server.json("/graph/labels?cell=2");
    assert_eq!(
        state_of(states.as_array().expect("states"), "verified")["cell"],
        true
    );

    // a partially labeled chain: split 2→8→14→20, tag the tail half, rejoin
    server.mutate("/graph/cut", json!({ "parentId": 8, "childId": 14 }));
    server.mutate(
        "/graph/labels",
        json!({ "cellId": 14, "scope": "track", "add": ["treated"] }),
    );
    server.mutate("/graph/link", json!({ "parentId": 8, "childId": 14 }));
    let states = server.json("/graph/labels?cell=2");
    assert_eq!(
        state_of(states.as_array().expect("states"), "treated")["track"],
        "some"
    );

    // the journal names the label and the target
    let scopes: Vec<String> = server
        .json("/edits")
        .as_array()
        .expect("edits")
        .iter()
        .map(|e| e["scope"].as_str().unwrap_or("").to_owned())
        .collect();
    assert!(
        scopes.iter().any(|s| s == "+verified · cell 2"),
        "{scopes:?}"
    );
    assert!(
        scopes
            .iter()
            .any(|s| s.starts_with("+cell type 1 · track ") && s.ends_with("(2 cells)")),
        "{scopes:?}"
    );

    // undo the link, then the tag: both label arrays return to the imported state
    server.mutate("/edits/undo", json!({}));
    let undone = server.mutate("/edits/undo", json!({}));
    assert_eq!(undone["domain"], "graph");
    let listed = cells(&server);
    assert_eq!(
        cell(&listed, 14)["trackLabels"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        cell(&listed, 20)["trackLabels"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        names(&cell(&listed, 13)["trackLabels"]),
        vec!["cell type 1"]
    );

    // a stale session cannot label
    let stale = server.post_as(
        "/graph/labels",
        "not-the-session",
        &json!({ "cellId": 2, "scope": "cell", "add": ["x"] }),
    );
    assert_eq!(stale.status().as_u16(), 409);
    // an empty edit is a bad request, a no-op a conflict
    let empty = server.post_as(
        "/graph/labels",
        &server.session(),
        &json!({ "cellId": 2, "scope": "cell" }),
    );
    assert_eq!(empty.status().as_u16(), 400);
    let noop = server.post_as(
        "/graph/labels",
        &server.session(),
        &json!({ "cellId": 2, "scope": "cell", "remove": ["never"] }),
    );
    assert_eq!(noop.status().as_u16(), 409);
}

#[test]
fn deleting_a_definition_strips_in_use_names_and_skips_unused_ones() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = imported(&dir);
    let edits_before = server.json("/edits").as_array().expect("edits").len();

    // unused: no journal row, just gone from the list
    let response = server.mutate_with(server.delete("/project/label-definitions/unused"));
    assert!(response.get("edit").is_none(), "{response}");
    assert!(
        !definitions(&server).iter().any(|(n, _)| n == "unused"),
        "{:?}",
        definitions(&server)
    );
    assert_eq!(
        server.json("/edits").as_array().expect("edits").len(),
        edits_before
    );

    // in use on 4 cells: one strip edit, then gone
    let response = server.mutate_with(server.delete("/project/label-definitions/control"));
    assert_eq!(response["edit"]["domain"], "graph");
    assert_eq!(
        response["edit"]["affectedCells"]
            .as_array()
            .expect("affected")
            .len(),
        4
    );
    assert!(!definitions(&server).iter().any(|(n, _)| n == "control"));
    let edits = server.json("/edits");
    assert_eq!(edits[0]["scope"], "strip control (4 cells)");
    let listed = cells(&server);
    assert_eq!(cell(&listed, 4)["labels"].as_array().map(Vec::len), Some(0));

    // undo the strip: the name is back on the cells and therefore back in the list
    server.mutate("/edits/undo", json!({}));
    assert!(definitions(&server).contains(&("control".to_owned(), 4)));

    // PUT replaces the stored list; in-use names stay through the union
    let response = server.mutate_with(server.put("/project/label-definitions").json(
        &json!({ "definitions": [
                { "name": "zeta", "color": "#00FF00" },
                { "name": "alpha" }
            ] }),
    ));
    let listed: Vec<&str> = response["definitions"]
        .as_array()
        .expect("definitions")
        .iter()
        .map(|d| d["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        listed,
        vec!["ESI", "alpha", "cell type 1", "control", "treated", "zeta"]
    );
    let zeta = response["definitions"]
        .as_array()
        .expect("definitions")
        .iter()
        .find(|d| d["name"] == "zeta")
        .expect("zeta");
    assert_eq!(zeta["color"], "#00ff00", "colours normalise and round-trip");
    assert!(
        server.json("/project")["labelDefinitions"]
            .as_array()
            .expect("defs")
            .iter()
            .any(|d| d["name"] == "zeta" && d["color"] == "#00ff00")
    );
    let bad = server
        .put("/project/label-definitions")
        .header(cellstudio_server::wire::SESSION_HEADER, server.session())
        .json(&json!({ "definitions": [{ "name": "ok" }, { "name": "  " }] }))
        .send()
        .expect("request");
    assert_eq!(bad.status().as_u16(), 400);
    let unfenced = server
        .put("/project/label-definitions")
        .json(&json!({ "definitions": [{ "name": "ok" }] }))
        .send()
        .expect("request");
    assert_eq!(
        unfenced.status().as_u16(),
        400,
        "the session header is required"
    );
}
