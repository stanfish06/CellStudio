//! The streaming tracking export: round-trip identity through the importer, metadata
//! content, and the graph-present summary.

use std::path::{Path, PathBuf};

use cellstudio_core::tracks::{CellRecord, TrackingStream, open_tracking};
use cellstudio_db::Project;
use cellstudio_db::queries::CellRow;

const APP_VERSION: &str = "9.9.9-test";

fn data(name: &str, artifact: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.data")
        .join(name)
        .join(artifact);
    assert!(path.exists(), "missing data {path:?} (run `mise run data`)");
    path
}

fn project(dir: &tempfile::TempDir, name: &str) -> Project {
    Project::create_or_open(&dir.path().join(name)).expect("project")
}

fn import(project: &Project, path: &Path) {
    let stream = open_tracking(path).expect("fixture header");
    project
        .db
        .stage_records(stream.records, &|_| {})
        .expect("stage");
    let offenders = project.db.validate_staged(false).expect("validate");
    assert_eq!(offenders, vec![], "the input graph is valid");
    project.db.materialize_staged().expect("materialize");
}

fn export(project: &Project) -> (cellstudio_db::export::ExportSummary, Vec<u8>) {
    let mut out = Vec::new();
    let summary = project
        .db
        .export_graph(APP_VERSION, &mut out, &|_| {})
        .expect("export");
    (summary, out)
}

fn reopen(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> TrackingStream {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write export");
    open_tracking(&path).expect("the export opens as a tracking file")
}

fn records(stream: TrackingStream) -> Vec<CellRecord> {
    stream
        .records
        .map(|record| record.expect("record parses"))
        .collect()
}

/// `seg_id` is omitted on export when it equals the id (the converter's convention), so
/// re-import stores NULL where the original stored the id itself; both resolve identically.
fn seg_resolved(mut rows: Vec<CellRow>) -> Vec<CellRow> {
    for row in &mut rows {
        row.seg_id = Some(row.seg_id.unwrap_or(row.id));
    }
    rows
}

#[test]
fn a_round_trip_through_the_importer_is_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = project(&dir, "first.zarr");
    import(&first, &data("tracking_valid", "tracks.json"));

    let (summary, exported) = export(&first);
    assert_eq!((summary.cells, summary.links), (24, 18));

    let exported_path = dir.path().join("export-a.json");
    std::fs::write(&exported_path, &exported).expect("write export");
    let second = project(&dir, "second.zarr");
    import(&second, &exported_path);

    // every cell field, both projects, seg_id compared as the importer resolves it
    let a = seg_resolved(first.db.cells_window(0, 3, None).expect("cells"));
    let b = seg_resolved(second.db.cells_window(0, 3, None).expect("cells"));
    assert_eq!(a.len(), 24);
    assert_eq!(a, b, "cells are identical field by field");

    // links, confidences, states, and the division survive: the second export is
    // byte-equal in records to the first
    let (_, exported_again) = export(&second);
    let a_records = records(reopen(&dir, "export-b.json", &exported));
    let b_records = records(reopen(&dir, "export-c.json", &exported_again));
    assert_eq!(a_records, b_records);

    let division: Vec<_> = a_records.iter().filter(|r| r.children.len() == 2).collect();
    assert_eq!(division.len(), 1, "one division");
    assert_eq!(
        division[0]
            .children
            .iter()
            .map(|c| (c.id, c.confidence))
            .collect::<Vec<_>>(),
        vec![(13, Some(0.822)), (18, Some(0.937))]
    );

    let head = &a_records[0];
    assert_eq!((head.id, head.t), (1, 0));
    assert_eq!(head.confidence, Some(0.901));
    assert_eq!(head.state.map(|s| s.as_str()), Some("normal"));
    assert_eq!(head.labels, vec!["ESI".to_owned(), "treated".to_owned()]);
    assert_eq!(
        head.features
            .get("area")
            .and_then(serde_json::Value::as_i64),
        Some(40)
    );
    assert!(
        head.parent.is_none(),
        "links are emitted children-side only"
    );
    assert!(head.seg_id.is_none(), "seg_id equal to the id is omitted");
}

#[test]
fn metadata_carries_created_and_the_app_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = project(&dir, "data.zarr");
    import(&source, &data("tracking_valid", "tracks.json"));

    let (summary, exported) = export(&source);
    let stream = reopen(&dir, "export.json", &exported);
    assert_eq!(stream.header.format, "cellstudio-tracking");
    assert_eq!(stream.header.version, 1);
    assert_eq!(
        stream.header.metadata.created.as_deref(),
        Some(summary.created.as_str())
    );
    assert!(
        summary.created.len() == 20 && summary.created.ends_with('Z'),
        "RFC3339 UTC seconds: {}",
        summary.created
    );
    assert_eq!(
        stream
            .header
            .metadata
            .extra
            .get("app_version")
            .and_then(serde_json::Value::as_str),
        Some(APP_VERSION)
    );
}

#[test]
fn has_graph_reports_link_presence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = project(&dir, "data.zarr");
    assert!(!source.db.has_graph().expect("fresh project"));
    import(&source, &data("tracking_valid", "tracks.json"));
    assert!(source.db.has_graph().expect("imported graph"));
}

#[test]
fn an_empty_graph_exports_an_empty_cells_array() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = project(&dir, "data.zarr");
    let (summary, exported) = export(&source);
    assert_eq!((summary.cells, summary.links), (0, 0));
    assert_eq!(records(reopen(&dir, "empty.json", &exported)), vec![]);
}

#[test]
#[ignore = "timing probe over the imported F00 graph; run with --release -- --ignored"]
fn f00_export_timing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = project(&dir, "f00.zarr");
    let tracking = data("F00", "tracking.json.gz");

    let start = std::time::Instant::now();
    import(&source, &tracking);
    let imported = start.elapsed();

    let start = std::time::Instant::now();
    let (summary, exported) = export(&source);
    eprintln!(
        "F00: import {imported:?}; export of {} cells / {} links -> {} bytes (uncompressed) in {:?}",
        summary.cells,
        summary.links,
        exported.len(),
        start.elapsed()
    );
    assert_eq!(summary.cells, 168_093);
}
