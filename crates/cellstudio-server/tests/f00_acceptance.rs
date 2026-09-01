//! Task 3.4 on the real F00 tracking file: 168k cells import through
//! `POST /import/tracks` with a session-scoped `graphChanged`. Store adoption is covered
//! synthetically in `tests/inventory.rs`, so this holds no 130 MB label fixture.
//! `cargo test -p cellstudio-server --release --test f00_acceptance -- --ignored --nocapture`

mod support;

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use support::Server;

const JOB_WINDOW: Duration = Duration::from_secs(300);

fn data(name: &str) -> PathBuf {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.data")
        .join(name);
    assert!(path.exists(), "missing data {path:?}");
    path
}

#[test]
#[ignore = "opens the 4 GB F00 dataset and imports 168k cells; needs .data locally"]
fn f00_tracking_imports_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    // the image is symlinked so the project container lands in the tempdir, never beside
    // the read-only original
    let link = dir.path().join("f00.zarr");
    std::os::unix::fs::symlink(data("260817_EXP63_live_bse_fa100_F00.zarr"), &link)
        .expect("symlink image");
    let project_dir = dir.path().join("f00.cellstudio");

    let server = Server::without_proxy();
    server.open_project(&link);
    assert!(
        project_dir.join("project.json").exists(),
        "project.json created"
    );
    assert!(
        project_dir.join("tracks.sqlite").exists(),
        "tracks.sqlite created"
    );
    let ticket = server.ws_ticket();
    let mut events = server.connect_events(&ticket).expect("events");
    let started = std::time::Instant::now();
    server.mutate(
        "/import/tracks",
        json!({ "path": data("F00").join("tracking.json.gz") }),
    );
    let jobs = server.await_jobs(JOB_WINDOW);
    let import = jobs
        .iter()
        .find(|j| j["kind"] == "import-tracks")
        .unwrap_or_else(|| panic!("no import job in {jobs:?}"));
    assert_eq!(import["status"], "done", "{import}");
    assert_eq!(import["progress"], 1.0);
    println!(
        "import job done in {:?}: {}",
        started.elapsed(),
        import["message"]
    );

    let changed = events.next_event_of("graphChanged", Duration::from_secs(30));
    assert_eq!(changed["sessionId"], server.session().as_str(), "{changed}");
    assert!(changed["graphVersion"].as_u64().expect("graph version") >= 1);

    let cells = server.json("/cells?t0=100&t1=100");
    let rows = cells.as_array().expect("cells");
    assert!(!rows.is_empty());
    assert!(
        rows.iter().all(|c| c["trackId"].as_u64().is_some()),
        "every imported cell carries a track id"
    );
    assert_eq!(server.json("/project")["hasGraph"], true);
}
