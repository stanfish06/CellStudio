//! Task 7.6: link/unlink ack latency on the imported F00 graph, against the 50 ms budget.
//! `cargo test -p cellstudio-db --release --test graph_latency -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cellstudio_core::tracks::open_tracking;
use cellstudio_db::Project;

fn data(name: &str, artifact: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.data")
        .join(name)
        .join(artifact);
    assert!(path.exists(), "missing data {path:?}");
    path
}

fn stats(mut samples: Vec<Duration>) -> (Duration, Duration, Duration) {
    samples.sort();
    (
        samples[0],
        samples[samples.len() / 2],
        *samples.last().unwrap(),
    )
}

#[test]
#[ignore = "imports F00 (168k cells); needs .data/F00"]
fn f00_link_and_unlink_latency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dir.path().join("data.zarr")).expect("project");
    let stream = open_tracking(&data("F00", "tracking.json.gz")).expect("open");
    project
        .db
        .stage_records(stream.records, &|_| {})
        .expect("stage");
    assert!(
        project
            .db
            .validate_staged(false)
            .expect("validate")
            .is_empty()
    );
    project
        .db
        .materialize_staged(&[], &Default::default())
        .expect("materialize");

    // 10 cells at t=100 whose track also has a member at t=99: unlink each chain,
    // then re-link the exact (t=99 → t=100) pair the unlink just broke.
    let at_99 = project.db.cells_window(99, 99, None).expect("window");
    let mut targets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in project.db.cells_window(100, 100, None).expect("window") {
        let Some(track) = row.track_id else { continue };
        let Some(prev) = at_99.iter().find(|c| c.track_id == Some(track)) else {
            continue;
        };
        if seen.insert(track) {
            targets.push((prev.id, row.id));
            if targets.len() == 10 {
                break;
            }
        }
    }
    assert_eq!(
        targets.len(),
        10,
        "expected 10 distinct tracks spanning t=99..100"
    );

    let mut unlink = Vec::new();
    let mut link = Vec::new();
    for (parent, child) in targets {
        let start = Instant::now();
        project.db.graph_unlink(child).expect("unlink");
        unlink.push(start.elapsed());

        let start = Instant::now();
        project.db.graph_link(parent, child).expect("link");
        link.push(start.elapsed());
    }

    let (u_min, u_med, u_max) = stats(unlink);
    let (l_min, l_med, l_max) = stats(link);
    println!("unlink min/median/max: {u_min:?} / {u_med:?} / {u_max:?}");
    println!("link   min/median/max: {l_min:?} / {l_med:?} / {l_max:?}");
    assert!(u_med < Duration::from_millis(50), "unlink median {u_med:?}");
    assert!(l_med < Duration::from_millis(50), "link median {l_med:?}");
}
