//! The adopted-store inventory: the reservation gate, the one-transaction publisher, and
//! the rerun-from-scratch behavior of an interrupted inventory.

use std::path::{Path, PathBuf};

use cellstudio_core::labels::{ExtentRow, VoxelBox};
use cellstudio_db::{DbError, Project};
use rusqlite::Connection;

fn dataset(dir: &Path) -> PathBuf {
    dir.join("data.zarr")
}

fn db_path(dir: &Path) -> PathBuf {
    dir.join("data.cellstudio").join("tracks.sqlite")
}

fn project() -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");
    (dir, project)
}

/// A scanned `(t, label)` occurrence: two voxels at (1, 2, 4) and (1, 2, 5).
fn row(t: u64, label: u32) -> ExtentRow {
    ExtentRow {
        t,
        label,
        bbox: Some(VoxelBox {
            z0: 1,
            z1: 1,
            y0: 2,
            y1: 2,
            x0: 4,
            x1: 5,
        }),
        area: 2,
        sum_z: 2.0,
        sum_y: 4.0,
        sum_x: 9.0,
    }
}

#[test]
fn reservation_is_refused_while_an_adopted_store_awaits_inventory() {
    let (_dir, project) = project();
    assert!(!project.db.inventory_pending().expect("pending"));

    project
        .db
        .require_inventory("labels.zarr|1")
        .expect("require");
    assert!(project.db.inventory_pending().expect("pending"));
    assert!(matches!(
        project.db.reserve_label_ids(4),
        Err(DbError::InventoryPending)
    ));

    // a store the app created itself carries the marker from creation: no gate
    project
        .db
        .set_inventory_marker("labels.zarr|1")
        .expect("marker");
    assert!(!project.db.inventory_pending().expect("pending"));
    assert_eq!(project.db.reserve_label_ids(4).expect("reserve"), 1);
}

#[test]
fn publishing_the_inventory_seeds_rows_and_opens_reservation_above_every_stored_id() {
    let (_dir, project) = project();
    project.db.require_inventory("store-a").expect("require");
    // a journal row against the pre-inventory store must not survive the publish
    project
        .db
        .record_edit(
            cellstudio_db::EditDomain::Mask,
            &serde_json::json!({"kind": "stroke"}),
            &serde_json::Value::Null,
        )
        .expect("journal");

    project
        .db
        .publish_inventory(&[row(0, 5), row(1, 9)], 9, "store-a")
        .expect("publish");
    assert!(!project.db.inventory_pending().expect("pending"));
    assert_eq!(
        project.db.inventory_marker().expect("marker").as_deref(),
        Some("store-a")
    );

    let cells = project.db.cells_window(0, 9, None).expect("cells");
    assert_eq!(
        cells
            .iter()
            .map(|c| (c.id, c.t, c.centroid, c.area, c.track_id))
            .collect::<Vec<_>>(),
        vec![
            (5, 0, Some([1.0, 2.0, 4.5]), Some(2), None),
            (9, 1, Some([1.0, 2.0, 4.5]), Some(2), None),
        ],
        "centroid from sums/area, tracking fields null"
    );
    let extent = project.db.extent_of(1, 9).expect("extent").expect("row");
    assert_eq!((extent.area, extent.sum_x), (2, 9.0));
    assert_eq!(
        extent.bbox.map(|b| (b.z, b.y, b.x)),
        Some(([1, 1], [2, 2], [4, 5]))
    );

    assert!(
        project.db.edits(10).expect("edits").is_empty(),
        "the mask journal is cleared with the publish"
    );
    assert_eq!(
        project.db.reserve_label_ids(1).expect("reserve"),
        10,
        "the floor is past the highest stored id"
    );
}

#[test]
fn an_interrupted_inventory_reruns_from_scratch_and_ends_consistent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(dir.path());
    drop(Project::create_or_open(&dataset(dir.path())).expect("create"));
    {
        // rows a run that never committed its marker would have left behind, were the
        // publish not one transaction; the rerun must own and replace them
        let conn = Connection::open(&path).expect("raw");
        conn.execute_batch(
            r#"
INSERT INTO mask_labels(t, label) VALUES (0, 5);
INSERT INTO mask_extent(t, label, z0, z1, y0, y1, x0, x1, area, sum_z, sum_y, sum_x)
  VALUES (0, 5, 0, 0, 0, 0, 0, 0, 1, 0.0, 0.0, 0.0);
"#,
        )
        .expect("partial rows");
    }

    let project = Project::create_or_open(&dataset(dir.path())).expect("reopen");
    project.db.require_inventory("store-b").expect("require");
    assert!(
        matches!(
            project.db.reserve_label_ids(1),
            Err(DbError::InventoryPending)
        ),
        "partial rows without the marker are not treated as complete"
    );

    project
        .db
        .publish_inventory(&[row(2, 7)], 7, "store-b")
        .expect("publish");
    assert_eq!(
        project.db.extent_of(0, 5).expect("extent"),
        None,
        "the rewrite owns the whole table: the partial row is gone"
    );
    assert!(project.db.extent_of(2, 7).expect("extent").is_some());
    assert_eq!(project.db.reserve_label_ids(1).expect("reserve"), 8);
}
