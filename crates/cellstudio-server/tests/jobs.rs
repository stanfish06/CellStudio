//! GET /jobs as the durable truth for reconnect reconciliation, scoped so superseded jobs stay out.

mod support;

use std::time::Duration;

use serde_json::{Value, json};

use support::{Server, data_copy, dev_dataset, skip};

const JOB_TIMEOUT: Duration = Duration::from_secs(120);

fn start_rechunk(server: &Server, body: Value) -> String {
    let started = server.post("/rechunk").json(&body).send().expect("rechunk");
    assert!(
        started.status().is_success(),
        "POST /rechunk -> {}",
        started.status()
    );
    started.json::<Value>().expect("JobRef")["id"]
        .as_str()
        .expect("job id")
        .to_owned()
}

#[test]
fn jobs_is_empty_until_something_is_scheduled() {
    let server = Server::without_proxy();
    assert_eq!(server.jobs(), Vec::<Value>::new(), "nothing has run yet");

    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    server.open_project(&dataset);
    assert_eq!(
        server.jobs(),
        Vec::<Value>::new(),
        "--no-proxy schedules no work on open"
    );
}

#[test]
fn a_rechunk_is_listed_while_it_runs_and_after_it_finishes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let id = start_rechunk(&server, json!({ "z": 16, "y": 32, "x": 32 }));

    let listed = server.jobs();
    let running = listed
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("job {id} is not listed right after it started: {listed:?}"));
    assert_eq!(running["kind"], "rechunk");
    assert!(
        ["running", "done"].contains(&running["status"].as_str().expect("status")),
        "a job is listed from the moment it is scheduled: {running}"
    );

    let finished = server.await_jobs(JOB_TIMEOUT);
    let job = finished
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("job {id} disappeared: {finished:?}"));
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["progress"], 1.0);
    assert!(
        job["message"]
            .as_str()
            .is_some_and(|message| message.contains("bricks.zarr")),
        "the finished job names what it wrote: {job}"
    );
}

/// A job scheduled under one session must not touch the next one.
#[test]
fn a_job_from_a_superseded_session_is_cancelled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = data_copy(&dir, "hostile_planes", "image.zarr");
    let second = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    let old_session = server.open_project(&first)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();

    let id = start_rechunk(&server, json!({ "z": 16, "y": 32, "x": 32 }));
    let new_session = server.open_project(&second)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();
    assert_ne!(new_session, old_session);

    // publishing the new session settles every job of the old one before the open returns
    let listed = server.jobs();
    let job = listed
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("job {id} is not listed: {listed:?}"));
    assert_ne!(
        job["status"], "running",
        "a superseded job must not still be running: {job}"
    );
    if job["status"] == "cancelled" {
        assert_eq!(
            job["message"], "superseded by a new session",
            "a cancelled job says why: {job}"
        );
    }

    // and whatever it did with its working copy, the new session did not adopt it
    let project = server.json("/project");
    assert_eq!(
        project["sessionId"], new_session,
        "the new session is the published one"
    );
    assert_eq!(
        project["dims"],
        json!({"t": 4, "c": 2, "z": 4, "y": 32, "x": 32}),
        "the new session reads its own dataset"
    );
    assert_eq!(
        project["levels"][0]["chunks"],
        json!({"t": 1, "c": 1, "z": 4, "y": 16, "x": 16}),
        "the superseded re-chunk must not have changed the new session's layout"
    );

    let settled = server.await_jobs(JOB_TIMEOUT);
    assert!(
        settled
            .iter()
            .all(|job| job["status"] != "running" || job["id"] != id.as_str()),
        "the superseded job never resumes: {settled:?}"
    );
}

#[test]
fn an_import_is_scheduled_as_a_job_and_reports_why_it_cannot_run_yet() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let tracks = support::data("tracking_valid", "tracks.json");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    let session = server.session();

    // label mask import still lands with a later phase; the job says so instead of hanging
    let started = server.post_as("/import/labels", &session, &json!({ "path": tracks }));
    assert!(started.status().is_success(), "POST /import/labels");
    let id = started.json::<Value>().expect("JobRef")["id"]
        .as_str()
        .expect("job id")
        .to_owned();

    let listed = server.jobs();
    let job = listed
        .iter()
        .find(|job| job["id"] == id.as_str())
        .unwrap_or_else(|| panic!("job {id} is not listed: {listed:?}"));
    assert_eq!(job["kind"], "import-labels");
    assert_eq!(job["status"], "failed", "{job}");
    assert!(
        job["message"]
            .as_str()
            .is_some_and(|message| message.contains("not implemented yet")),
        "a job that cannot run says so instead of hanging: {job}"
    );

    // an import of something that is not there never becomes a job at all
    let missing = server.post_as(
        "/import/tracks",
        &session,
        &json!({ "path": dir.path().join("nope.json") }),
    );
    assert_eq!(missing.status(), 404);
    assert_eq!(
        server.jobs().len(),
        1,
        "a refused import must not leave a job behind"
    );

    let unknown = server.post_as("/import/masks", &session, &json!({ "path": tracks }));
    assert_eq!(unknown.status(), 400, "unknown import kind");
}

#[test]
fn a_rechunk_is_refused_before_a_project_is_open_and_with_a_zero_extent() {
    let server = Server::without_proxy();
    assert_eq!(
        server
            .post("/rechunk")
            .json(&json!({}))
            .send()
            .expect("rechunk")
            .status(),
        404,
        "no project is open"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    server.open_project(&dataset);
    assert_eq!(
        server
            .post("/rechunk")
            .json(&json!({ "z": 0 }))
            .send()
            .expect("rechunk")
            .status(),
        400,
        "a zero brick extent is not a layout"
    );
    assert!(server.jobs().is_empty(), "a refused re-chunk starts no job");
}

#[test]
fn dev_dataset_open_schedules_no_proxy_because_its_pyramid_reads_in_one_chunk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(dataset) = dev_dataset(&dir) else {
        return skip("CELLSTUDIO_DEV_DATASET is not set");
    };
    let server = Server::start();
    server.open_project(&dataset);
    std::thread::sleep(Duration::from_millis(500));

    assert_eq!(
        server.jobs(),
        Vec::<Value>::new(),
        "one chunk per (t, c) at every level: a proxy would copy the level for no gain"
    );
    let volume = server
        .get("/volume?t=0&c=0&level=2")
        .send()
        .expect("request");
    assert_eq!(
        volume
            .headers()
            .get("x-cellstudio-volume-source")
            .and_then(|value| value.to_str().ok()),
        Some("pyramid")
    );
}
