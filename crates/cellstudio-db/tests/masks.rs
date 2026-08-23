//! Schema v2: the v1 migration, incremental mask stats, id reservation, and journal pruning.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use cellstudio_db::{
    CellChange, ChunkSnapshot, DbError, EditDomain, ExtentDelta, ExtentRow, LinkRow, MAX_LABEL_ID,
    Project, VoxelBox,
};
use rusqlite::Connection;
use serde_json::json;

/// The v1 schema verbatim, so the migration test pins what shipped rather than what the
/// current source happens to say v1 was.
const V1_SCHEMA: &str = r#"
CREATE TABLE cells (
  id INTEGER PRIMARY KEY,
  t INTEGER NOT NULL,
  z REAL, y REAL, x REAL,
  area INTEGER,
  detection_confidence REAL,
  state TEXT,
  track_id INTEGER,
  src_id INTEGER,
  seg_id INTEGER,
  labels TEXT,
  features TEXT,
  reviewed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX cells_by_t ON cells(t);
CREATE INDEX cells_by_track ON cells(track_id);

CREATE TABLE mask_labels (
  t INTEGER NOT NULL,
  label INTEGER NOT NULL,
  PRIMARY KEY (t, label)
);

CREATE TABLE links (
  parent INTEGER NOT NULL REFERENCES cells(id),
  child  INTEGER NOT NULL REFERENCES cells(id),
  confidence REAL,
  reviewed INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (parent, child)
);
CREATE INDEX links_by_child ON links(child);

CREATE TABLE edits (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  domain TEXT NOT NULL,
  op TEXT NOT NULL,
  inverse TEXT NOT NULL,
  undone INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE edit_blobs (
  seq INTEGER NOT NULL,
  chunk_key TEXT NOT NULL,
  before BLOB NOT NULL
);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE staging_cells (
  id INTEGER PRIMARY KEY,
  t INTEGER NOT NULL,
  z REAL, y REAL, x REAL,
  seg_id INTEGER,
  track_id INTEGER,
  detection_confidence REAL,
  state TEXT,
  labels TEXT,
  features TEXT
);
CREATE INDEX staging_cells_by_t ON staging_cells(t);

CREATE TABLE staging_links (
  parent INTEGER NOT NULL,
  child INTEGER NOT NULL,
  confidence REAL,
  side TEXT NOT NULL,
  PRIMARY KEY (parent, child, side)
);
CREATE INDEX staging_links_by_child ON staging_links(child);

INSERT OR IGNORE INTO meta(key, value) VALUES
  ('version.image', '0'),
  ('version.labels', '0'),
  ('version.graph', '0'),
  ('version.settings', '0'),
  ('settings', '{}');
PRAGMA user_version = 1;
"#;

fn dataset(dir: &Path) -> PathBuf {
    dir.join("data.zarr")
}

fn db_path(dir: &Path) -> PathBuf {
    dir.join("data.cellstudio").join("tracks.sqlite")
}

/// Raw access, only while no `Project` holds the file exclusively.
fn raw(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    conn
}

/// Every object in the database, with SQL comments and layout stripped so the comparison is
/// over columns and constraints rather than prose.
fn schema_of(path: &Path) -> Vec<(String, String)> {
    let conn = raw(path);
    let mut stmt = conn
        .prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master ORDER BY name")
        .expect("prepare");
    stmt.query_map([], |row| {
        let sql: String = row.get(1)?;
        let bare: String = sql
            .lines()
            .map(|line| line.split("--").next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok((row.get(0)?, bare))
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

/// A paint of `voxels` for `label`, as the rasterizer reports it.
fn paint(label: u32, voxels: &[[u32; 3]]) -> ExtentDelta {
    let mut delta = ExtentDelta {
        label,
        area: voxels.len() as i64,
        sum_z: 0.0,
        sum_y: 0.0,
        sum_x: 0.0,
        bbox: None,
    };
    for v in voxels {
        delta.sum_z += f64::from(v[0]);
        delta.sum_y += f64::from(v[1]);
        delta.sum_x += f64::from(v[2]);
        let one = VoxelBox {
            z: [v[0], v[0]],
            y: [v[1], v[1]],
            x: [v[2], v[2]],
        };
        delta.bbox = Some(delta.bbox.map_or(one, |b| b.union(one)));
    }
    delta
}

/// The inverse of [`paint`]: the same voxels leaving, growing no bbox.
fn erase(label: u32, voxels: &[[u32; 3]]) -> ExtentDelta {
    let painted = paint(label, voxels);
    ExtentDelta {
        area: -painted.area,
        sum_z: -painted.sum_z,
        sum_y: -painted.sum_y,
        sum_x: -painted.sum_x,
        bbox: None,
        ..painted
    }
}

fn updated(change: &CellChange) -> &cellstudio_db::CellRow {
    match change {
        CellChange::Updated(cell) => cell,
        other => panic!("expected an updated cell, got {other:?}"),
    }
}

#[test]
fn a_v1_database_migrates_to_v2_with_its_rows_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(dir.path());
    std::fs::create_dir_all(path.parent().expect("parent")).expect("container");

    {
        let conn = raw(&path);
        conn.execute_batch(V1_SCHEMA).expect("v1 schema");
        conn.execute_batch(
            r#"
INSERT INTO cells(id, t, z, y, x, area, track_id, reviewed) VALUES
  (1, 0, 1.0, 2.0, 3.0, 10, 1, 0),
  (2, 1, 1.0, 2.0, 3.0, 12, 1, 0);
INSERT INTO links(parent, child, confidence, reviewed) VALUES (1, 2, 0.9, 1);
INSERT INTO mask_labels(t, label) VALUES (0, 1), (1, 2);
INSERT INTO edits(ts, domain, op, inverse) VALUES ('2024-01-01T00:00:00.000Z', 'mask',
  '{"kind":"brush"}', '{"kind":"restore_blobs"}');
INSERT INTO edit_blobs(seq, chunk_key, before) VALUES (1, 'c/0/0/0/0/0', x'0102');
"#,
        )
        .expect("v1 rows");
    }

    let project = Project::create_or_open(&dataset(dir.path())).expect("open migrates");
    let cells = project.db.cells_window(0, 9, None).expect("cells");
    assert_eq!(
        cells
            .iter()
            .map(|c| (c.id, c.t, c.area))
            .collect::<Vec<_>>(),
        vec![(1, 0, Some(10)), (2, 1, Some(12))],
        "v1 rows survive the migration"
    );
    let entries = project.db.edits(10).expect("edits");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].undoable, "the v1 blob is carried into v2");

    let blobs = project.db.take_blobs(entries[0].seq).expect("blobs");
    assert_eq!(
        blobs,
        vec![ChunkSnapshot {
            chunk_key: "c/0/0/0/0/0".to_string(),
            existed: true,
            before: Some(vec![1, 2]),
        }],
        "every v1 snapshot had an object behind it, so existed is 1"
    );

    // the v2 tables are usable on the migrated database
    project
        .db
        .apply_extent_delta(0, &[paint(1, &[[0, 0, 0]])])
        .expect("delta");
    drop(project);

    let fresh_dir = tempfile::tempdir().expect("tempdir");
    let fresh = Project::create_or_open(&dataset(fresh_dir.path())).expect("fresh project");
    drop(fresh);
    assert_eq!(
        schema_of(&path),
        schema_of(&db_path(fresh_dir.path())),
        "a migrated database and a freshly created one are the same schema"
    );
    assert_eq!(
        raw(&path)
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("user_version"),
        2
    );
}

#[test]
fn the_extent_delta_keeps_area_and_centroid_exact_over_paints_and_erases() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");

    let changes = project
        .db
        .apply_extent_delta(3, &[paint(7, &[[0, 0, 0], [0, 2, 4], [4, 4, 8]])])
        .expect("paint");
    let cell = updated(&changes[0]);
    assert_eq!((cell.id, cell.t, cell.area), (7, 3, Some(3)));
    assert_eq!(
        cell.centroid,
        Some([4.0 / 3.0, 2.0, 4.0]),
        "centroid is the exact mean of the painted voxels"
    );

    let changes = project
        .db
        .apply_extent_delta(3, &[paint(7, &[[8, 8, 12]])])
        .expect("second stroke");
    let cell = updated(&changes[0]);
    assert_eq!(cell.area, Some(4));
    assert_eq!(cell.centroid, Some([3.0, 3.5, 6.0]));

    let changes = project
        .db
        .apply_extent_delta(3, &[erase(7, &[[8, 8, 12]])])
        .expect("erase");
    let cell = updated(&changes[0]);
    assert_eq!(cell.area, Some(3));
    assert_eq!(
        cell.centroid,
        Some([4.0 / 3.0, 2.0, 4.0]),
        "an erase folds back to exactly the pre-paint sums"
    );

    let extent = project.db.extent_of(3, 7).expect("extent").expect("row");
    assert_eq!(
        extent,
        ExtentRow {
            bbox: Some(VoxelBox {
                z: [0, 8],
                y: [0, 8],
                x: [0, 12]
            }),
            area: 3,
            sum_z: 4.0,
            sum_y: 6.0,
            sum_x: 12.0,
        },
        "the bbox still covers the erased voxel: it is an upper bound, never shrunk"
    );

    assert_eq!(
        project.db.extent_of(4, 7).expect("extent"),
        None,
        "extents are per frame"
    );
}

#[test]
fn erasing_the_last_voxel_drops_the_cell_its_links_and_its_mask_label() {
    let dir = tempfile::tempdir().expect("tempdir");
    let voxels = [[1, 1, 1], [1, 1, 2]];

    let project = Project::create_or_open(&dataset(dir.path())).expect("project");
    project
        .db
        .apply_extent_delta(0, &[paint(1, &voxels)])
        .expect("paint parent");
    project
        .db
        .apply_extent_delta(1, &[paint(2, &voxels)])
        .expect("paint child");
    drop(project);
    raw(&db_path(dir.path()))
        .execute(
            "INSERT INTO links(parent, child, confidence) VALUES (1, 2, 0.9)",
            [],
        )
        .expect("link");

    let project = Project::create_or_open(&dataset(dir.path())).expect("reopen");
    let changes = project
        .db
        .apply_extent_delta(1, &[erase(2, &voxels)])
        .expect("erase every voxel");
    let CellChange::Removed(snapshot) = &changes[0] else {
        panic!("expected the cell to be removed, got {changes:?}");
    };
    assert_eq!(snapshot.cell.id, 2);
    assert_eq!(
        snapshot.links,
        vec![LinkRow {
            parent: 1,
            child: 2,
            confidence: Some(0.9),
            reviewed: false,
        }],
        "the removal carries the links away with it, so the inverse can put them back"
    );

    assert_eq!(
        project.db.cells_window(0, 9, None).expect("cells").len(),
        1,
        "only the parent is left"
    );
    assert!(project.db.lineage(2).is_err(), "the child row is gone");
    assert_eq!(
        project.db.review_queue(10).expect("queue"),
        vec![],
        "its link went with it"
    );

    // and the inverse restores all three
    let seq = project
        .db
        .record_edit_pending(EditDomain::Mask, &json!({"kind": "erase"}), &json!({}), &[])
        .expect("journal");
    let commit = project
        .db
        .commit_edit(seq, 1, &[paint(2, &voxels)], std::slice::from_ref(snapshot))
        .expect("undo");
    assert_eq!(commit.version, 1, "the commit bumps version.labels with it");
    assert_eq!(updated(&commit.cells[0]).id, 2);
    assert_eq!(project.db.review_queue(10).expect("queue").len(), 1);
    assert_eq!(project.db.lineage(2).expect("lineage").root, 1);
}

#[test]
fn painting_an_existing_id_at_another_frame_is_refused_rather_than_moving_the_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");
    project
        .db
        .apply_extent_delta(2, &[paint(9, &[[0, 0, 0]])])
        .expect("paint");

    let err = project
        .db
        .apply_extent_delta(5, &[paint(9, &[[0, 0, 1]])])
        .expect_err("one id, one frame");
    assert!(
        matches!(
            err,
            DbError::LabelFrameConflict {
                label: 9,
                existing: 2,
                requested: 5
            }
        ),
        "unexpected error: {err}"
    );

    let cells = project.db.cells_window(0, 9, None).expect("cells");
    assert_eq!(
        cells.iter().map(|c| (c.id, c.t)).collect::<Vec<_>>(),
        vec![(9, 2)],
        "the refused edit left the cell on its own frame"
    );
    assert_eq!(
        project.db.extent_of(5, 9).expect("extent"),
        None,
        "and wrote no extent for the frame it was refused on"
    );

    let err = project
        .db
        .ensure_extent(5, 9, || Ok::<_, DbError>(ExtentRow::default()))
        .expect_err("seeding is refused on the same grounds");
    assert!(matches!(err, DbError::LabelFrameConflict { .. }));
}

#[test]
fn ensure_extent_scans_once_and_later_edits_are_incremental_from_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");

    // an adopted store already holds 100 voxels of label 42 on frame 0
    let scans = Cell::new(0u32);
    let scan = || {
        scans.set(scans.get() + 1);
        Ok::<_, DbError>(ExtentRow {
            bbox: Some(VoxelBox {
                z: [0, 3],
                y: [10, 20],
                x: [10, 20],
            }),
            area: 100,
            sum_z: 200.0,
            sum_y: 1500.0,
            sum_x: 1600.0,
        })
    };

    assert!(project.db.ensure_extent(0, 42, scan).expect("seed"));
    assert!(
        !project.db.ensure_extent(0, 42, scan).expect("seed again"),
        "the second call finds the row and does not scan"
    );
    assert_eq!(scans.get(), 1);

    let changes = project
        .db
        .apply_extent_delta(0, &[paint(42, &[[4, 30, 30], [4, 30, 32]])])
        .expect("stroke");
    let cell = updated(&changes[0]);
    assert_eq!(
        cell.area,
        Some(102),
        "the stroke adds to the scanned area rather than replacing it"
    );
    assert_eq!(
        cell.centroid,
        Some([208.0 / 102.0, 1560.0 / 102.0, 1662.0 / 102.0])
    );

    let extent = project.db.extent_of(0, 42).expect("extent").expect("row");
    assert_eq!(
        extent.bbox,
        Some(VoxelBox {
            z: [0, 4],
            y: [10, 30],
            x: [10, 32]
        }),
        "the scanned bbox grows by the stroke"
    );

    project
        .db
        .apply_extent_delta(0, &[paint(43, &[[0, 0, 0]])])
        .expect("a new label");
    assert_eq!(
        scans.get(),
        1,
        "a label painted from nothing never pays for a scan"
    );
}

#[test]
fn a_delta_that_would_undercount_a_label_is_an_error_rather_than_a_wrong_area() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");
    let err = project
        .db
        .apply_extent_delta(0, &[erase(5, &[[0, 0, 0]])])
        .expect_err("no extent was seeded");
    assert!(
        matches!(
            err,
            DbError::ExtentUnderflow {
                t: 0,
                label: 5,
                area: -1
            }
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn a_delta_on_the_background_id_touches_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");
    let changes = project
        .db
        .apply_extent_delta(0, &[erase(0, &[[0, 0, 0]])])
        .expect("an erase writes 0, which is not a cell");
    assert!(changes.is_empty());
    assert_eq!(project.db.extent_of(0, 0).expect("extent"), None);
}

#[test]
fn reservation_is_monotonic_seeds_past_existing_ids_and_stops_at_the_overlay_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");

    assert_eq!(
        project.db.reserve_label_ids(64).expect("reserve"),
        1,
        "0 is background, so an empty project starts at 1"
    );
    assert_eq!(project.db.reserve_label_ids(64).expect("reserve"), 65);
    drop(project);

    let project = Project::create_or_open(&dataset(dir.path())).expect("reopen");
    assert_eq!(
        project.db.reserve_label_ids(1).expect("reserve"),
        129,
        "the counter is durable across a reopen"
    );

    // a label the counter never issued — an adopted store, or an import
    project
        .db
        .apply_extent_delta(0, &[paint(5000, &[[0, 0, 0]])])
        .expect("paint");
    assert_eq!(
        project.db.reserve_label_ids(4).expect("reserve"),
        5001,
        "reservation seeds past every id in cells and mask_labels"
    );
    assert_eq!(project.db.reserve_label_ids(1).expect("reserve"), 5005);

    let err = project
        .db
        .reserve_label_ids(MAX_LABEL_ID)
        .expect_err("past 2^24-1");
    assert!(
        matches!(err, DbError::LabelIdsExhausted { .. }),
        "unexpected error: {err}"
    );
    assert_eq!(
        project.db.reserve_label_ids(1).expect("reserve"),
        5006,
        "the refused reservation did not advance the counter"
    );
}

#[test]
fn blob_pruning_keeps_the_newest_fifty_and_leaves_the_older_rows_not_undoable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");

    let mut seqs = Vec::new();
    for i in 0..60u32 {
        seqs.push(
            project
                .db
                .record_edit_pending(
                    EditDomain::Mask,
                    &json!({"kind": "brush", "scope": format!("stroke {i}")}),
                    &json!({"kind": "restore_blobs"}),
                    &[ChunkSnapshot {
                        chunk_key: format!("c/0/0/0/0/{i}"),
                        existed: i % 2 == 0,
                        before: (i % 2 == 0).then(|| vec![i as u8]),
                    }],
                )
                .expect("journal"),
        );
    }

    assert_eq!(project.db.prune_blobs(50).expect("prune"), 10);
    assert!(
        project.db.take_blobs(seqs[9]).expect("blobs").is_empty(),
        "the tenth-oldest stroke lost its snapshots"
    );
    assert_eq!(
        project.db.take_blobs(seqs[10]).expect("blobs"),
        vec![ChunkSnapshot {
            chunk_key: "c/0/0/0/0/10".to_string(),
            existed: true,
            before: Some(vec![10]),
        }]
    );
    assert_eq!(
        project.db.take_blobs(seqs[11]).expect("blobs"),
        vec![ChunkSnapshot {
            chunk_key: "c/0/0/0/0/11".to_string(),
            existed: false,
            before: None,
        }],
        "a chunk that did not exist journals no bytes"
    );

    let entries = project.db.edits(60).expect("edits");
    assert_eq!(entries.len(), 60, "history keeps every row");
    assert_eq!(
        entries.iter().filter(|e| e.undoable).count(),
        50,
        "the pruned rows report as not undoable rather than failing at the attempt"
    );
}

#[test]
fn undo_and_redo_walk_the_journal_and_a_new_edit_clears_the_redo_stack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dataset(dir.path())).expect("project");

    let journal = |kind: &str| {
        project
            .db
            .record_edit_pending(
                EditDomain::Mask,
                &json!({"kind": kind}),
                &json!({"kind": "restore_blobs"}),
                &[ChunkSnapshot {
                    chunk_key: kind.to_string(),
                    existed: false,
                    before: None,
                }],
            )
            .expect("journal")
    };
    let first = journal("a");
    project.db.clear_pending(first).expect("commit");
    let second = journal("b");
    project.db.clear_pending(second).expect("commit");

    let next = project.db.undo_next().expect("undo").expect("a row");
    assert_eq!((next.seq, next.undoable), (second, true));
    assert_eq!(next.op, json!({"kind": "b"}));
    assert_eq!(next.inverse, json!({"kind": "restore_blobs"}));
    project.db.mark_undone(second, true).expect("mark");

    assert_eq!(
        project.db.undo_next().expect("undo").expect("a row").seq,
        first,
        "undo walks backwards"
    );
    assert_eq!(
        project.db.redo_next().expect("redo").expect("a row").seq,
        second,
        "redo takes the most recently undone row"
    );

    let third = journal("c");
    assert_eq!(
        project.db.redo_next().expect("redo"),
        None,
        "a new edit clears the redo stack"
    );
    assert_eq!(
        project.db.edits(10).expect("edits").len(),
        2,
        "the undone row and its blobs are gone"
    );
    assert_eq!(
        project.db.undo_next().expect("undo").expect("a row").seq,
        first,
        "the new row is still pending, so undo skips past it"
    );

    let pending = project.db.pending_edits().expect("pending");
    assert_eq!(
        pending.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![third]
    );
    project.db.delete_edit(third).expect("recover");
    assert!(project.db.take_blobs(third).expect("blobs").is_empty());
    assert_eq!(project.db.edits(10).expect("edits").len(), 1);
}
