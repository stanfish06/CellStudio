//! Tracking import over HTTP: the job end-to-end with progress events, the session fence
//! on the start request, the one-import-at-a-time lock, and the v1 empty-graph policy.

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{Server, data, data_copy};

/// Two tests import the 168k-cell F00 fixture to open a race window; a debug build under
/// the full workspace's parallel test load needs well past 30 s to finish one.
const JOB_WINDOW: Duration = Duration::from_secs(120);
const EVENT_WINDOW: Duration = Duration::from_secs(10);

fn import_job<'a>(jobs: &'a [Value], id: &str) -> &'a Value {
    jobs.iter()
        .find(|job| job["id"] == id)
        .unwrap_or_else(|| panic!("job {id} is not listed: {jobs:?}"))
}

#[test]
fn a_valid_tracking_file_imports_end_to_end_with_progress_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    let ticket = server.ws_ticket();
    let mut events = server.connect_events(&ticket).expect("event socket");

    let started = server.mutate(
        "/import/tracks",
        json!({ "path": data("tracking_valid", "tracks.json.gz") }),
    );
    let id = started["id"].as_str().expect("job id").to_owned();

    // the job announces itself, progresses, and finishes over the socket; the commit's
    // Versions announcement arrives before the terminal job frame
    let mut statuses = Vec::new();
    let mut versions = Vec::new();
    loop {
        let event = events.next_event(EVENT_WINDOW);
        match event["type"].as_str() {
            Some("versions") => versions.push(event["versions"].clone()),
            Some("job") if event["job"]["id"] == id.as_str() => {
                let status = event["job"]["status"].as_str().expect("status").to_owned();
                statuses.push(status.clone());
                if status != "running" {
                    assert_eq!(status, "done", "{event}");
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(
        statuses.iter().filter(|s| *s == "running").count() >= 1,
        "progress events were observed: {statuses:?}"
    );
    let announced = versions
        .iter()
        .find(|v| v["graph"] == 1)
        .unwrap_or_else(|| panic!("no Versions frame carries the bumped graph: {versions:?}"));
    assert_eq!(announced["sessionId"], server.session());

    let jobs = server.await_jobs(JOB_WINDOW);
    let job = import_job(&jobs, &id);
    assert_eq!(job["status"], "done", "{job}");
    assert!(
        job["message"]
            .as_str()
            .is_some_and(|m| m.contains("24 cells") && m.contains("18 links")),
        "{job}"
    );

    let cells = server.json("/cells?t0=0&t1=3");
    let cells = cells.as_array().expect("cells");
    assert_eq!(cells.len(), 24);
    assert!(
        cells.iter().all(|c| c["trackId"].is_u64()),
        "every imported cell carries its track id"
    );
    let lineage = server.json("/lineage?cell=7");
    assert_eq!(
        lineage["links"]
            .as_array()
            .expect("links")
            .iter()
            .filter(|l| l["parent"] == 7)
            .count(),
        2,
        "the division survives the wire"
    );
}

#[test]
fn an_invalid_file_fails_the_job_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let started = server.mutate(
        "/import/tracks",
        json!({ "path": data("tracking_broken_reference", "tracks.json") }),
    );
    let id = started["id"].as_str().expect("job id").to_owned();
    let jobs = server.await_jobs(JOB_WINDOW);
    let job = import_job(&jobs, &id);
    assert_eq!(job["status"], "failed", "{job}");
    let message = job["message"].as_str().expect("message");
    assert!(
        message.contains("999999") && message.contains("nothing was written"),
        "the offender is named: {message}"
    );

    assert_eq!(
        server
            .json("/cells?t0=0&t1=3")
            .as_array()
            .expect("cells")
            .len(),
        0,
        "the database is unchanged"
    );
    assert_eq!(server.json("/project")["versions"]["graph"], 0);

    // the failed job released the lock: the valid file imports afterwards
    let started = server.mutate(
        "/import/tracks",
        json!({ "path": data("tracking_valid", "tracks.json") }),
    );
    let id = started["id"].as_str().expect("job id").to_owned();
    let jobs = server.await_jobs(JOB_WINDOW);
    assert_eq!(import_job(&jobs, &id)["status"], "done");
}

#[test]
fn the_start_request_is_session_fenced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    let tracks = json!({ "path": data("tracking_valid", "tracks.json") });

    let unfenced = server
        .post("/import/tracks")
        .json(&tracks)
        .send()
        .expect("request");
    assert_eq!(unfenced.status().as_u16(), 400, "no session header");

    let stale = server.post_as("/import/tracks", "not-the-open-session", &tracks);
    assert_eq!(stale.status().as_u16(), 409, "a stale session is refused");
    assert_eq!(server.jobs(), Vec::<Value>::new(), "no job was started");
}

#[test]
fn a_second_import_is_rejected_while_one_is_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    let session = server.session();

    // the F00 file is large enough that the second request lands mid-import
    let first = server.post_as(
        "/import/tracks",
        &session,
        &json!({ "path": data("F00", "tracking.json.gz") }),
    );
    assert!(first.status().is_success(), "{}", first.status());
    let first_id = first.json::<Value>().expect("JobRef")["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let second = server.post_as(
        "/import/tracks",
        &session,
        &json!({ "path": data("tracking_valid", "tracks.json") }),
    );
    if second.status().as_u16() != 409 {
        // the first import can only have won the race by finishing already
        let jobs = server.jobs();
        let first_job = import_job(&jobs, &first_id);
        assert_ne!(
            first_job["status"], "running",
            "a second import ran beside a live one: {first_job}"
        );
    } else {
        let error = second.json::<Value>().expect("error")["error"]
            .as_str()
            .expect("message")
            .to_owned();
        assert!(error.contains("one import"), "{error}");
    }

    let jobs = server.await_jobs(JOB_WINDOW);
    assert_eq!(import_job(&jobs, &first_id)["status"], "done");
}

#[test]
fn a_project_switch_mid_import_publishes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    let session = server.session();

    let started = server.post_as(
        "/import/tracks",
        &session,
        &json!({ "path": data("F00", "tracking.json.gz") }),
    );
    assert!(started.status().is_success(), "{}", started.status());
    let id = started.json::<Value>().expect("JobRef")["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    // replacing the session cancels the job before it can publish
    server.open_project(&dataset);
    assert_ne!(server.session(), session, "a reopen is a new session");

    let jobs = server.await_jobs(JOB_WINDOW);
    let job = jobs
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("job {id} is not listed: {jobs:?}"));
    if job["status"] == "cancelled" {
        assert_eq!(
            server.json("/project")["versions"]["graph"],
            0,
            "nothing was published"
        );
        assert_eq!(
            server
                .json("/cells?t0=0&t1=300")
                .as_array()
                .expect("cells")
                .len(),
            0
        );
    } else {
        // the import can only escape cancellation by finishing before the reopen landed
        assert_eq!(job["status"], "done", "{job}");
    }
}

#[test]
fn a_project_with_a_graph_rejects_import_naming_the_condition() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    let tracks = json!({ "path": data("tracking_valid", "tracks.json") });

    server.mutate("/import/tracks", tracks.clone());
    let jobs = server.await_jobs(JOB_WINDOW);
    assert!(jobs.iter().any(|j| j["status"] == "done"), "{jobs:?}");

    let again = server.post_as("/import/tracks", &server.session(), &tracks);
    assert_eq!(again.status().as_u16(), 409);
    let error = again.json::<Value>().expect("error")["error"]
        .as_str()
        .expect("message")
        .to_owned();
    assert!(
        error.contains("track graph"),
        "the condition is named: {error}"
    );
    assert_eq!(
        server.jobs().len(),
        1,
        "the refusal never became a job: {:?}",
        server.jobs()
    );
}
