//! The adopted-store inventory over HTTP: the job at open, the reservation floor after it
//! completes, and the refusal of id reservation and mask writes while it has not.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cellstudio_core::labels::{self, StrokeMode, StrokeSpec};
use serde_json::{Value, json};
use support::{Server, data_copy};

const JOB_WINDOW: Duration = Duration::from_secs(10);

fn labels_root(dataset: &Path) -> PathBuf {
    let mut root = dataset.to_path_buf();
    root.set_extension("cellstudio");
    root.join("labels.zarr")
}

/// A store the conversion script would leave behind: written into the project directory
/// before the app ever opens it, one blob per `(t, label)`, no completeness marker.
fn adopted_store(dataset: &Path, cells: &[(u64, u32)]) {
    let image = cellstudio_core::open(dataset).expect("image");
    let root = labels_root(dataset);
    std::fs::create_dir_all(root.parent().expect("project dir")).expect("mkdir");
    let store = labels::ensure_store(&root, &image).expect("store");
    for (t, label) in cells {
        labels::apply(
            &store,
            *t,
            &StrokeSpec {
                mode: StrokeMode::Paint { label: *label },
                radius: 3.0,
                plane: None,
                centres: vec![[1.5, 16.0, 16.0]],
            },
        )
        .expect("paint");
    }
}

fn inventory_job(jobs: &[Value]) -> &Value {
    jobs.iter()
        .find(|job| job["kind"] == "inventory")
        .unwrap_or_else(|| panic!("no inventory job in {jobs:?}"))
}

#[test]
fn an_adopted_store_is_inventoried_at_open_and_reservation_starts_above_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    adopted_store(&dataset, &[(0, 5), (1, 9)]);

    let server = Server::without_proxy();
    server.open_project(&dataset);
    let jobs = server.await_jobs(JOB_WINDOW);
    let job = inventory_job(&jobs);
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["progress"], 1.0);

    let cells = server.json("/cells?t0=0&t1=3");
    assert_eq!(
        cells
            .as_array()
            .expect("cells")
            .iter()
            .map(|c| (c["id"].as_u64(), c["t"].as_u64(), c["trackId"].clone()))
            .collect::<Vec<_>>(),
        vec![
            (Some(5), Some(0), Value::Null),
            (Some(9), Some(1), Value::Null),
        ],
        "the inventory seeded a cells row per (t, label)"
    );

    let lease = server.mutate("/mask/reserve", json!({ "count": 1 }));
    assert_eq!(
        lease["first"], 10,
        "the reserved id is above every id present in the store"
    );
    let stroke = server.mutate(
        "/mask/stroke",
        json!({
            "t": 2,
            "label": 10,
            "mode": "paint",
            "radius": 3.0,
            "plane": "z",
            "stamps": [[1.5, 8.5, 8.5]],
            "only": null,
        }),
    );
    assert!(stroke["version"].as_u64().expect("version") > 0);
}

#[test]
fn a_store_the_inventory_cannot_cover_keeps_reservation_and_mask_writes_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    // one id past the renderable ceiling and one id on two frames: the scan flags both,
    // the job fails, and the marker never commits
    adopted_store(&dataset, &[(0, 5), (2, 5), (1, 1 << 24)]);

    let server = Server::without_proxy();
    server.open_project(&dataset);
    let jobs = server.await_jobs(JOB_WINDOW);
    let job = inventory_job(&jobs);
    assert_eq!(job["status"], "failed", "{job}");
    let message = job["message"].as_str().expect("message");
    assert!(
        message.contains("16777216"),
        "names the oversized id: {message}"
    );
    assert!(message.contains("5"), "names the two-frame id: {message}");

    let session = server.session();
    let reserve = server.post_as("/mask/reserve", &session, &json!({ "count": 1 }));
    assert_eq!(reserve.status().as_u16(), 409);
    let error = reserve.json::<Value>().expect("error")["error"]
        .as_str()
        .expect("message")
        .to_owned();
    assert!(error.contains("inventor"), "names the inventory: {error}");

    let stroke = server.post_as(
        "/mask/stroke",
        &session,
        &json!({
            "t": 3,
            "label": 5,
            "mode": "paint",
            "radius": 3.0,
            "plane": "z",
            "stamps": [[1.5, 8.5, 8.5]],
            "only": null,
        }),
    );
    assert_eq!(stroke.status().as_u16(), 409, "mask writes are refused too");
}
