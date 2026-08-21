//! The /events WebSocket: current versions on (re)connect and job progress as it happens.

mod support;

use std::time::Duration;

use serde_json::json;

use support::{Server, data_copy, skip};

const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn connecting_delivers_the_current_versions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    let opened = server.open_project(&dataset);

    let mut events = server
        .connect_events(&server.ws_ticket())
        .expect("connect /events");
    let frame = events.next_event(FRAME_TIMEOUT);
    assert_eq!(frame["type"], "versions", "{frame}");
    assert_eq!(
        frame["versions"], opened["versions"],
        "the opening frame carries exactly what /project reports"
    );
    assert_eq!(frame["versions"]["sessionId"], opened["sessionId"]);
    assert_eq!(
        frame["versions"],
        json!({
            "sessionId": opened["sessionId"],
            "image": 0,
            "labels": 0,
            "graph": 0,
            "settings": 0,
        })
    );
}

#[test]
fn a_reconnect_delivers_the_versions_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let mut first = server
        .connect_events(&server.ws_ticket())
        .expect("connect /events");
    let opening = first.next_event(FRAME_TIMEOUT);
    assert_eq!(opening["type"], "versions");
    drop(first);

    // a write the disconnected client missed
    server
        .put("/settings")
        .json(&json!({ "activeView": "xz" }))
        .send()
        .expect("put");

    let mut second = server
        .connect_events(&server.ws_ticket())
        .expect("reconnect /events");
    let resync = second.next_event(FRAME_TIMEOUT);
    assert_eq!(resync["type"], "versions", "{resync}");
    assert_eq!(
        resync["versions"]["settings"], 1,
        "the reconnect hands over the version written while it was away: {resync}"
    );
    assert_eq!(
        resync["versions"],
        server.json("/project")["versions"],
        "the reconnect frame agrees with the queryable truth"
    );
}

#[test]
fn a_connected_client_is_told_when_a_project_opens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();

    // connect before any project exists: there is nothing to open with
    let mut events = server
        .connect_events(&server.ws_ticket())
        .expect("connect /events");
    assert!(
        events.try_next(Duration::from_millis(250)).is_none(),
        "no project is open, so there are no versions to send"
    );

    let opened = server.open_project(&dataset);
    let frame = events.next_event_of("versions", FRAME_TIMEOUT);
    assert_eq!(frame["versions"]["sessionId"], opened["sessionId"]);
}

#[test]
fn a_settings_write_publishes_the_new_versions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let mut events = server
        .connect_events(&server.ws_ticket())
        .expect("connect /events");
    assert_eq!(events.next_event(FRAME_TIMEOUT)["type"], "versions");

    server
        .put("/settings")
        .json(&json!({ "activeView": "yz" }))
        .send()
        .expect("put");
    let frame = events.next_event_of("versions", FRAME_TIMEOUT);
    assert_eq!(frame["versions"]["settings"], 1, "{frame}");
}

/// A job's whole life is published: running when it starts, progress as it moves, done at
/// the end, while `GET /jobs` stays the queryable truth.
#[test]
fn job_progress_events_arrive_for_a_rechunk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let mut events = server
        .connect_events(&server.ws_ticket())
        .expect("connect /events");
    assert_eq!(events.next_event(FRAME_TIMEOUT)["type"], "versions");

    let started = server
        .post("/rechunk")
        .json(&json!({ "z": 16, "y": 32, "x": 32 }))
        .send()
        .expect("rechunk");
    assert!(started.status().is_success());
    let id = started.json::<serde_json::Value>().expect("JobRef")["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let mut progress = Vec::new();
    let mut terminal = None;
    for _ in 0..200 {
        let frame = events.next_event(FRAME_TIMEOUT);
        if frame["type"] != "job" || frame["job"]["id"] != id.as_str() {
            continue;
        }
        assert_eq!(frame["job"]["kind"], "rechunk", "{frame}");
        progress.push(frame["job"]["progress"].as_f64().expect("progress"));
        if frame["job"]["status"] != "running" {
            terminal = Some(frame["job"].clone());
            break;
        }
    }

    let terminal = terminal.unwrap_or_else(|| panic!("the re-chunk never settled: {progress:?}"));
    assert_eq!(terminal["status"], "done", "{terminal}");
    assert_eq!(terminal["progress"], 1.0);
    assert!(
        progress.first() == Some(&0.0),
        "the first frame is the job starting: {progress:?}"
    );
    assert!(
        progress.windows(2).all(|pair| pair[0] <= pair[1]),
        "progress must never go backwards: {progress:?}"
    );

    // the same state is queryable, which is what a reconnecting client reconciles from
    let listed = server.jobs();
    let job = listed
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("job {id} is not listed: {listed:?}"));
    assert_eq!(job["status"], "done");
    assert_eq!(job["progress"], 1.0);
}

#[test]
fn graph_changed_events_need_the_track_editing_phase() {
    skip(
        "no route publishes Event::GraphChanged yet; POST /edits/* lands with track editing, \
         so the variant is unreachable from the HTTP surface this crate exposes",
    );
}
