//! The staged tracking import: stage → validate → materialize, all-or-nothing on every
//! abort path, and the v1 empty-graph policy.

use std::path::{Path, PathBuf};

use cellstudio_core::labels::ExtentRow;
use cellstudio_core::tracks::open_tracking;
use cellstudio_db::import::StageError;
use cellstudio_db::{EditDomain, Project};

fn data(name: &str, artifact: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.data")
        .join(name)
        .join(artifact);
    assert!(path.exists(), "missing data {path:?} (run `mise run data`)");
    path
}

fn project() -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = Project::create_or_open(&dir.path().join("data.zarr")).expect("project");
    (dir, project)
}

/// Stage a fixture, as the import job does.
fn stage(project: &Project, path: &Path) -> Result<u64, StageError> {
    let stream = open_tracking(path).expect("fixture header");
    project
        .db
        .stage_records(stream.records, &|_| {})
        .map(|report| report.cells)
}

fn assert_untouched(project: &Project) {
    assert_eq!(project.db.staged_counts().expect("staging"), (0, 0));
    assert!(
        project
            .db
            .cells_window(0, 100, None)
            .expect("cells")
            .is_empty()
    );
    assert_eq!(project.db.versions().expect("versions").graph, 0);
}

#[test]
fn the_valid_fixture_stages_validates_and_materializes() {
    let (_dir, project) = project();
    let staged = stage(&project, &data("tracking_valid", "tracks.json")).expect("stage");
    assert_eq!(staged, 24);

    let offenders = project.db.validate_staged(false).expect("validate");
    assert_eq!(offenders, vec![], "the fixture is valid");

    let summary = project.db.materialize_staged().expect("materialize");
    assert_eq!(
        (
            summary.cells,
            summary.links,
            summary.tracks,
            summary.divisions
        ),
        (24, 18, 8, 1),
        "18 distinct links even though the file declares both sides"
    );
    assert_eq!(project.db.staged_counts().expect("staging"), (0, 0));
    assert_eq!(project.db.versions().expect("versions").graph, 1);

    let cells = project.db.cells_window(0, 3, None).expect("cells");
    assert_eq!(cells.len(), 24);
    let first = &cells[0];
    assert_eq!(
        (
            first.id,
            first.t,
            first.track_id,
            first.src_id,
            first.seg_id
        ),
        (1, 0, Some(1), Some(1), Some(1)),
        "the file id is the project id and the round-trip src_id"
    );
    assert_eq!(first.labels, vec!["ESI".to_owned(), "treated".to_owned()]);
    assert_eq!(
        first
            .features
            .get("area")
            .and_then(serde_json::Value::as_i64),
        Some(40)
    );

    // the division is one parent with two children, confidences carried
    let lineage = project.db.lineage(7).expect("lineage");
    let children: Vec<_> = lineage
        .links
        .iter()
        .filter(|l| l.parent == 7)
        .map(|l| (l.child, l.confidence))
        .collect();
    assert_eq!(children, vec![(13, Some(0.822)), (18, Some(0.937))]);
}

#[test]
fn materialization_updates_inventory_created_cell_rows_in_place() {
    let (_dir, project) = project();
    // the inventory created a row for the mask-backed cell: centroid and area from the scan
    let scanned = ExtentRow {
        t: 0,
        label: 1,
        bbox: None,
        area: 4,
        sum_z: 4.0,
        sum_y: 8.0,
        sum_x: 12.0,
    };
    project
        .db
        .publish_inventory(&[scanned], 1, "store")
        .expect("inventory");

    let stream = open_tracking(&data("tracking_valid", "tracks.json")).expect("fixture");
    // one cell, links stripped: its partners are not staged in this test
    let one_cell = stream.records.take(1).map(|r| {
        r.map(|mut cell| {
            cell.children.clear();
            cell.parent = None;
            cell
        })
    });
    project.db.stage_records(one_cell, &|_| {}).expect("stage");
    project.db.materialize_staged().expect("materialize");

    let cells = project.db.cells_window(0, 0, None).expect("cells");
    assert_eq!(cells.len(), 1, "updated in place, not duplicated");
    let cell = &cells[0];
    assert_eq!(cell.track_id, Some(1), "tracking fields landed");
    assert_eq!(cell.area, Some(4), "the scanned area survives the import");
    assert_eq!(
        cell.centroid,
        Some([1.05, 23.3, 18.975]),
        "the file's centroid replaces the scanned one when provided"
    );
}

#[test]
fn a_broken_reference_aborts_naming_the_offender_and_writes_nothing() {
    let (_dir, project) = project();
    stage(&project, &data("tracking_broken_reference", "tracks.json")).expect("stage");
    let offenders = project.db.validate_staged(false).expect("validate");
    assert_eq!(offenders.len(), 1, "{offenders:?}");
    assert_eq!(offenders[0].cell_id, 19);
    assert!(offenders[0].message.contains("999999"), "{offenders:?}");

    project.db.clear_staging().expect("clear");
    assert_untouched(&project);
}

#[test]
fn a_child_at_an_earlier_frame_aborts() {
    let (_dir, project) = project();
    stage(
        &project,
        &data("tracking_child_earlier_frame", "tracks.json"),
    )
    .expect("stage");
    let offenders = project.db.validate_staged(false).expect("validate");
    assert!(!offenders.is_empty());
    assert!(
        offenders.iter().any(|e| e.cell_id == 3
            && e.message.contains("t=0")
            && e.message.contains("parent 20")),
        "{offenders:?}"
    );

    project.db.clear_staging().expect("clear");
    assert_untouched(&project);
}

#[test]
fn an_id_past_u32_is_a_parse_error_and_staging_is_cleared() {
    let (_dir, project) = project();
    let error = stage(&project, &data("tracking_ids_too_wide", "tracks.json"))
        .expect_err("2^32 does not fit u32");
    assert!(matches!(error, StageError::Parse(_)), "{error}");
    assert_untouched(&project);
}

#[test]
fn a_parse_failure_after_many_records_clears_staging_and_publishes_nothing() {
    let (_dir, project) = project();
    // a truncated copy: many records stream fine, then the file ends mid-record
    let full = std::fs::read_to_string(data("tracking_valid", "tracks.json")).expect("fixture");
    let cut = full.find(r#""id": 20"#).expect("record 20 exists");
    let truncated = project.root.join("truncated.json");
    std::fs::write(&truncated, &full[..cut]).expect("write");

    let error = stage(&project, &truncated).expect_err("truncation");
    assert!(matches!(error, StageError::Parse(_)), "{error}");
    assert_untouched(&project);

    // the aborted stream left nothing behind for the next import to inherit
    stage(&project, &data("tracking_valid", "tracks.json")).expect("stage");
    assert_eq!(project.db.validate_staged(false).expect("validate"), vec![]);
    let summary = project.db.materialize_staged().expect("materialize");
    assert_eq!(summary.cells, 24);
}

#[test]
fn a_duplicate_cell_id_aborts_staging() {
    let (_dir, project) = project();
    let stream = open_tracking(&data("tracking_valid", "tracks.json")).expect("fixture");
    let records: Vec<_> = stream.records.collect();
    let doubled = records
        .iter()
        .cloned()
        .chain(records.iter().take(1).cloned());
    let error = project
        .db
        .stage_records(doubled, &|_| {})
        .expect_err("cell 1 twice");
    assert!(matches!(error, StageError::DuplicateCell(1)), "{error}");
    assert_untouched(&project);
}

#[test]
fn mask_resolution_checks_coalesce_seg_id_id_against_the_inventory() {
    let (_dir, project) = project();
    stage(&project, &data("tracking_valid", "tracks.json")).expect("stage");

    // the fixture's masks: value 100t+1..100t+6 on each of 4 frames (labels_background0)
    let row = |t: u64, label: u32| ExtentRow {
        t,
        label,
        bbox: None,
        area: 1,
        sum_z: 0.0,
        sum_y: 0.0,
        sum_x: 0.0,
    };
    let mut rows = Vec::new();
    for t in 0..4u64 {
        for k in 1..=6u32 {
            rows.push(row(t, 100 * t as u32 + k));
        }
    }
    // an incomplete inventory: t=3 is missing, so the six cells there cannot resolve
    project
        .db
        .publish_inventory(&rows[..18], 306, "store")
        .expect("inventory");
    let offenders = project.db.validate_staged(true).expect("validate");
    assert_eq!(offenders.len(), 6, "{offenders:?}");
    assert!(
        offenders
            .iter()
            .any(|e| e.cell_id == 19 && e.message.contains("301")),
        "COALESCE(seg_id, id) is named: {offenders:?}"
    );

    // the full inventory resolves every cell
    project
        .db
        .publish_inventory(&rows, 306, "store")
        .expect("inventory");
    assert_eq!(project.db.validate_staged(true).expect("validate"), vec![]);
}

#[test]
fn the_v1_policy_refuses_a_non_empty_graph_or_graph_history() {
    let (_dir, project) = project();
    assert_eq!(project.db.import_blocker().expect("blocker"), None);

    stage(&project, &data("tracking_valid", "tracks.json")).expect("stage");
    assert_eq!(project.db.validate_staged(false).expect("validate"), vec![]);
    project.db.materialize_staged().expect("materialize");
    let blocked = project
        .db
        .import_blocker()
        .expect("blocker")
        .expect("links");
    assert!(blocked.contains("track graph"), "{blocked}");

    let (_dir2, edited) = self::project();
    edited
        .db
        .record_edit(
            EditDomain::Graph,
            &serde_json::json!({"kind": "link"}),
            &serde_json::Value::Null,
        )
        .expect("journal");
    let blocked = edited
        .db
        .import_blocker()
        .expect("blocker")
        .expect("history");
    assert!(blocked.contains("graph edit history"), "{blocked}");
}

/// The real thing, ignored by default: `cargo test -p cellstudio-db --test import -- --ignored`.
/// Imports F00's 168,093-cell tracking.json.gz through the db layer and prints wall times.
#[test]
#[ignore = "runs the full F00 import; slow and needs .data/F00"]
fn f00_import_end_to_end() {
    let (_dir, project) = project();
    let path = data("F00", "tracking.json.gz");

    let start = std::time::Instant::now();
    let stream = open_tracking(&path).expect("open");
    let report = project
        .db
        .stage_records(stream.records, &|_| {})
        .expect("stage");
    let staged_at = start.elapsed();
    let offenders = project.db.validate_staged(false).expect("validate");
    assert_eq!(offenders, vec![]);
    let validated_at = start.elapsed();
    let summary = project.db.materialize_staged().expect("materialize");
    let done = start.elapsed();

    println!(
        "staged {} cells / {} links in {staged_at:?}; validated at {validated_at:?}; \
         materialized {} cells, {} links, {} tracks, {} divisions at {done:?}",
        report.cells, report.links, summary.cells, summary.links, summary.tracks, summary.divisions
    );
    assert_eq!(summary.cells, 168_093);
    assert_eq!(project.db.versions().expect("versions").graph, 1);
}
