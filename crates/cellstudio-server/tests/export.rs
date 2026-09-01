//! The tracking snapshot job over HTTP: fence and graph gate on the start request, the
//! timestamped file under `snapshots/`, re-importability, and no leftovers on cancellation.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cellstudio_core::tracks::open_tracking;
use serde_json::{Value, json};
use support::{Server, data, data_copy};

const JOB_WINDOW: Duration = Duration::from_secs(60);

fn job<'a>(jobs: &'a [Value], id: &str) -> &'a Value {
    jobs.iter()
        .find(|job| job["id"] == id)
        .unwrap_or_else(|| panic!("job {id} is not listed: {jobs:?}"))
}

fn await_job(server: &Server, id: &str) -> Value {
    let jobs = server.await_jobs(JOB_WINDOW);
    job(&jobs, id).clone()
}

fn import_graph(server: &Server, tracks: &Path) {
    let started = server.mutate("/import/tracks", json!({ "path": tracks }));
    let id = started["id"].as_str().expect("job id").to_owned();
    assert_eq!(await_job(server, &id)["status"], "done");
}

fn snapshot_entries(project_path: &str) -> Vec<PathBuf> {
    let dir = Path::new(project_path).join("snapshots");
    if !dir.exists() {
        return Vec::new();
    }
    std::fs::read_dir(dir)
        .expect("read snapshots")
        .map(|entry| entry.expect("entry").path())
        .collect()
}

#[test]
fn a_snapshot_lands_timestamped_in_the_project_and_reimports() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    import_graph(&server, &data("tracking_valid", "tracks.json"));
    let project_path = server.json("/project")["projectPath"]
        .as_str()
        .expect("projectPath")
        .to_owned();
    assert_eq!(server.json("/project")["hasGraph"], true);

    let started = server.mutate("/export/tracks", json!({}));
    let id = started["id"].as_str().expect("job id").to_owned();
    let job = await_job(&server, &id);
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["kind"], "export");

    // the completion message carries the written path
    let written = PathBuf::from(job["message"].as_str().expect("path message"));
    assert!(written.exists(), "{written:?}");
    assert!(
        written.starts_with(Path::new(&project_path).join("snapshots")),
        "{written:?} is under the project's snapshots dir"
    );
    let name = written.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("tracking-") && name.ends_with("Z.json.gz") && name.len() == 33,
        "timestamped name: {name}"
    );
    let entries = snapshot_entries(&project_path);
    assert_eq!(entries, vec![written.clone()], "no temp files remain");

    // the gzipped snapshot parses, its metadata stamp matches the filename, and it
    // carries the app version
    let stream = open_tracking(&written).expect("snapshot parses");
    let created = stream.header.metadata.created.clone().expect("created");
    let stamp: String = created
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    assert_eq!(name, format!("tracking-{stamp}.json.gz"));
    assert_eq!(
        stream
            .header
            .metadata
            .extra
            .get("app_version")
            .and_then(Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(stream.records.count(), 24);

    // the snapshot re-imports into a fresh project and reproduces the graph
    let second_dir = tempfile::tempdir().expect("tempdir");
    let second = data_copy(&second_dir, "tiny_v2", "image.zarr");
    server.open_project(&second);
    import_graph(&server, &written);
    let cells = server.json("/cells?t0=0&t1=3");
    assert_eq!(cells.as_array().expect("cells").len(), 24);
    let lineage = server.json("/lineage?cell=7");
    assert_eq!(
        lineage["links"]
            .as_array()
            .expect("links")
            .iter()
            .filter(|l| l["parent"] == 7)
            .count(),
        2,
        "the division survives the round trip"
    );
}

#[test]
fn the_start_request_is_fenced_and_needs_a_graph() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    assert_eq!(server.json("/project")["hasGraph"], false);

    let unfenced = server.post("/export/tracks").send().expect("request");
    assert_eq!(unfenced.status().as_u16(), 400, "no session header");

    let empty = server.post_as("/export/tracks", &server.session(), &json!({}));
    assert_eq!(empty.status().as_u16(), 409, "no graph to snapshot");
    assert_eq!(server.jobs(), Vec::<Value>::new(), "no job was started");
}

#[test]
fn a_cancelled_export_leaves_no_snapshot_and_no_temp_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);
    // the F00 graph is large enough that the reopen usually lands mid-export
    import_graph(&server, &data("F00", "tracking.json.gz"));
    let project_path = server.json("/project")["projectPath"]
        .as_str()
        .expect("projectPath")
        .to_owned();

    let started = server.mutate("/export/tracks", json!({}));
    let id = started["id"].as_str().expect("job id").to_owned();

    // replacing the session cancels the job before the temp file is renamed
    server.open_project(&dataset);
    let job = await_job(&server, &id);

    // the status flips terminal at the reopen while the blocking body may still be
    // writing; wait until it has cleaned its temp file before judging what remains
    let deadline = std::time::Instant::now() + JOB_WINDOW;
    let entries = loop {
        let entries = snapshot_entries(&project_path);
        if entries
            .iter()
            .all(|p| p.extension() != Some("tmp".as_ref()))
        {
            break entries;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "temp files remain: {entries:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    if job["status"] == "cancelled" {
        assert_eq!(entries, Vec::<PathBuf>::new(), "nothing was published");
    } else {
        // the export can only escape cancellation by finishing before the reopen landed
        assert_eq!(job["status"], "done", "{job}");
        assert_eq!(entries.len(), 1);
        assert!(open_tracking(&entries[0]).is_ok(), "the survivor is whole");
    }
}
