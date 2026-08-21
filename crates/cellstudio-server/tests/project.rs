//! Session lifecycle and the project container: /project, /project/open, /settings.

mod support;

use std::path::Path;

use serde_json::{Value, json};

use support::{Server, data_copy, store_snapshot};

/// Every route that must name the session it was served under.
const STAMPED: [&str; 9] = [
    "/health",
    "/project",
    "/settings",
    "/jobs",
    "/store/.zattrs",
    "/slice?axis=xz&t=0&cs=0,1&pos=0",
    "/volume?t=0&c=0&level=2",
    "/pixel?t=0&c=0&z=0&y=0&x=0",
    "/histogram?t=0&c=0",
];

fn session_header(response: &reqwest::blocking::Response) -> Option<String> {
    response
        .headers()
        .get("x-cellstudio-session")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[test]
fn project_is_404_before_open_and_complete_after() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();

    let before = server.get("/project").send().expect("request");
    assert_eq!(before.status(), 404, "no project is open yet");
    let error: Value = before.json().expect("error body");
    assert_eq!(
        error["error"], "no project is open",
        "the client must be able to tell 'no project' from 'backend down'"
    );

    let opened = server.open_project(&dataset);
    let fetched = server.json("/project");
    assert_eq!(
        fetched, opened,
        "GET /project repeats what the open returned"
    );

    assert_eq!(
        fetched["dims"],
        json!({"t": 4, "c": 2, "z": 4, "y": 32, "x": 32})
    );
    assert_eq!(fetched["dtype"], "u16");
    assert_eq!(fetched["scale"], json!({"z": 2.0, "y": 0.5, "x": 0.5}));
    assert_eq!(fetched["levels"].as_array().expect("levels").len(), 3);
    assert_eq!(fetched["channels"].as_array().expect("channels").len(), 2);
    assert_eq!(fetched["channels"][0]["name"], "mNeonGreen-H2B");
    assert_eq!(fetched["channels"][0]["color"], "37FF00");
    assert_eq!(fetched["hasLabels"], false);
    assert_eq!(
        fetched["sourcePath"].as_str().map(Path::new),
        Some(dataset.as_path()),
        "the source is referenced in place"
    );

    // the project container sits beside the dataset and holds the database
    let project_path = Path::new(fetched["projectPath"].as_str().expect("projectPath"));
    assert_eq!(project_path.parent(), dataset.parent());
    assert!(
        project_path.join("project.json").is_file(),
        "{project_path:?}"
    );
    assert!(
        project_path.join("tracks.sqlite").is_file(),
        "{project_path:?}"
    );
    assert!(project_path.join("cache").is_dir(), "{project_path:?}");
}

#[test]
fn opening_a_dataset_never_writes_into_the_source_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let before = store_snapshot(&dataset);

    {
        let server = Server::without_proxy();
        server.open_project(&dataset);
        // read on every path the server offers
        server.json("/project");
        server.json("/pixel?t=0&c=0&z=0&y=0&x=0");
        server.json("/histogram?t=0&c=0");
        for path in ["/slice?axis=xz&t=0&cs=0,1&pos=0", "/volume?t=0&c=0&level=0"] {
            assert!(
                server
                    .get(path)
                    .send()
                    .expect("request")
                    .status()
                    .is_success()
            );
        }
    }

    let after = store_snapshot(&dataset);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "opening and viewing must not add or remove files in the source store"
    );
    for (path, bytes) in &before {
        assert!(
            after[path] == *bytes,
            "{path:?} was modified inside the source store"
        );
    }
}

#[test]
fn an_invalid_dataset_path_errors_and_creates_no_project_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = Server::without_proxy();

    let missing = dir.path().join("does_not_exist.zarr");
    let response = server.try_open_project(&missing);
    assert_eq!(response.status(), 404, "a path that is not there");
    let error: Value = response.json().expect("error body");
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("does not exist")),
        "the error must name the failing condition: {error}"
    );

    let not_a_store = dir.path().join("not_a_store.zarr");
    std::fs::create_dir_all(not_a_store.join("0")).expect("create the decoy");
    std::fs::write(not_a_store.join("readme.txt"), b"no zarr here").expect("write the decoy");
    let response = server.try_open_project(&not_a_store);
    assert_eq!(
        response.status(),
        400,
        "a directory that is not a zarr store"
    );
    let error: Value = response.json().expect("error body");
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("zarr")),
        "the error must name what was missing or malformed: {error}"
    );

    // nothing was created for either attempt
    for name in ["does_not_exist.cellstudio", "not_a_store.cellstudio"] {
        assert!(
            !dir.path().join(name).exists(),
            "{name} must not exist: an invalid store creates no project"
        );
    }
    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read tempdir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        !entries.iter().any(|name| name.ends_with(".cellstudio")),
        "a refused open left a project container behind: {entries:?}"
    );
    assert_eq!(
        server.get("/project").send().expect("request").status(),
        404,
        "a refused open leaves no project open"
    );
}

/// A failed open must not take the current project down with it: nothing was created, so
/// nothing should have been destroyed either.
#[test]
fn a_failed_open_leaves_the_previously_open_project_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    let opened = server.open_project(&dataset);

    let refused = server.try_open_project(&dir.path().join("does_not_exist.zarr"));
    assert_eq!(refused.status(), 404);

    let response = server.get("/project").send().expect("request");
    assert_eq!(
        response.status(),
        200,
        "the open project must survive a refused open of another store"
    );
    let still: Value = response.json().expect("ProjectInfo");
    assert_eq!(
        still["sessionId"], opened["sessionId"],
        "a refused open must not mint or drop a session"
    );
}

#[test]
fn every_response_carries_the_session_it_was_served_under() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();

    let before = server.get("/health").send().expect("request");
    assert_eq!(
        session_header(&before),
        None,
        "there is no session to name before the first open"
    );

    let session = server.open_project(&dataset)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();
    for path in STAMPED {
        let response = server.get(path).send().expect("request");
        assert!(
            response.status().is_success(),
            "GET {path} -> {}",
            response.status()
        );
        assert_eq!(
            session_header(&response).as_deref(),
            Some(session.as_str()),
            "GET {path} must name its session"
        );
    }
    // errors are stamped too, so a client can tell which session refused it
    let refused = server.get("/store/nope").send().expect("request");
    assert_eq!(refused.status(), 404);
    assert_eq!(session_header(&refused).as_deref(), Some(session.as_str()));
}

#[test]
fn opening_a_second_project_mints_a_new_session_and_makes_the_old_one_stale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = data_copy(&dir, "tiny_v2", "image.zarr");
    let second = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::without_proxy();

    let opened = server.open_project(&first);
    let old_session = opened["sessionId"].as_str().expect("sessionId").to_owned();
    // a response captured under the first session, held across the switch
    let stale = server
        .get("/slice?axis=xz&t=0&cs=0&pos=0")
        .send()
        .expect("request");
    assert_eq!(
        session_header(&stale).as_deref(),
        Some(old_session.as_str())
    );

    let reopened = server.open_project(&second);
    let new_session = reopened["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();
    assert_ne!(new_session, old_session, "every open yields a new session");
    assert_eq!(server.session(), new_session);

    // the held response still names the session it was served under, which is how a client
    // recognises it as stale
    assert_eq!(
        session_header(&stale).as_deref(),
        Some(old_session.as_str()),
        "a superseded result keeps its own session id"
    );
    assert_ne!(
        session_header(&stale).as_deref(),
        Some(new_session.as_str())
    );
    assert_eq!(
        server.json("/project")["dims"],
        json!({"t": 2, "c": 1, "z": 64, "y": 64, "x": 64}),
        "the new session serves the second dataset"
    );
}

#[test]
fn reopening_the_same_store_mints_a_new_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();

    let first = server.open_project(&dataset)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();
    let second = server.open_project(&dataset)["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned();
    assert_ne!(first, second, "a re-open is still a new session");
    assert_eq!(
        server.json("/project")["versions"]["sessionId"],
        second,
        "the versions block names the current session"
    );
}

#[test]
fn settings_round_trip_and_bump_the_settings_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    assert_eq!(
        server.json("/settings"),
        json!({}),
        "a project that has never stored settings answers with an empty object"
    );
    let before = server.json("/project")["versions"]["settings"]
        .as_u64()
        .expect("settings version");

    let blob = json!({
        "activeView": "xy",
        "views": { "xy": { "zoom": 1.5, "center": [12.0, 34.0] } },
        "channels": [{ "visible": true, "gamma": 0.8 }],
    });
    let stored = server.put("/settings").json(&blob).send().expect("put");
    assert_eq!(
        stored.status(),
        204,
        "PUT /settings acknowledges without a body"
    );
    assert!(
        session_header(&stored).is_some(),
        "the ack names its session"
    );

    assert_eq!(
        server.json("/settings"),
        blob,
        "the blob round-trips verbatim"
    );
    assert_eq!(
        server.json("/project")["versions"]["settings"]
            .as_u64()
            .expect("settings version"),
        before + 1,
        "a settings write bumps the settings counter"
    );

    // the blob is opaque, but it must be an object
    let refused = server
        .put("/settings")
        .json(&json!([1, 2, 3]))
        .send()
        .expect("put");
    assert_eq!(refused.status(), 400);
    assert_eq!(
        server.json("/settings"),
        blob,
        "the refused write changed nothing"
    );
}

/// The advisory describes the store `/store` serves, and a re-chunk only ever moves the
/// *assembled* path onto the working copy, so a re-chunk must not be able to present
/// itself as having improved XY.
#[test]
fn a_rechunk_leaves_the_xy_advisory_describing_the_source_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_zbrick", "image.zarr");
    let server = Server::without_proxy();
    let opened = server.open_project(&dataset);
    let source_xy = opened["layout"]["amplification"]["xy"].clone();
    assert_eq!(
        source_xy, 64.0,
        "the z-brick data decodes 64x the bytes an XY plane needs: {opened}"
    );
    assert_eq!(opened["layout"]["affectedViews"], json!(["xy"]));

    server
        .post("/rechunk")
        .json(&json!({ "z": 16, "y": 64, "x": 64 }))
        .send()
        .expect("rechunk");
    let jobs = server.await_jobs(std::time::Duration::from_secs(120));
    assert!(
        jobs.iter()
            .any(|job| job["kind"] == "rechunk" && job["status"] == "done"),
        "re-chunk did not finish: {jobs:?}"
    );

    let after = server.json("/project");
    assert_eq!(
        after["layout"]["amplification"]["xy"], source_xy,
        "XY still reads the source through /store, so its amplification cannot have changed"
    );
    assert_eq!(
        after["layout"]["affectedViews"],
        json!(["xy"]),
        "the XY advisory persists while the condition holds: {after}"
    );
}

#[test]
fn settings_and_project_need_an_open_project() {
    let server = Server::without_proxy();
    for path in ["/project", "/settings"] {
        assert_eq!(
            server.get(path).send().expect("request").status(),
            404,
            "GET {path} before any open"
        );
    }
    assert_eq!(
        server
            .put("/settings")
            .json(&json!({ "zoom": 1 }))
            .send()
            .expect("put")
            .status(),
        404,
        "PUT /settings before any open"
    );
}
