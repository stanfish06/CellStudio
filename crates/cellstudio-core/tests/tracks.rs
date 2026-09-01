//! `open_tracking`: magic-byte compression detection, header validation before any record,
//! and streaming records that fail cleanly mid-array.

use std::path::PathBuf;

use cellstudio_core::tracks::{CellRecord, TRACKING_FORMAT, TrackingOpenError, open_tracking};

fn data(name: &str, artifact: &str) -> PathBuf {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.data")
        .join(name)
        .join(artifact);
    assert!(path.exists(), "missing data {path:?} (run `mise run data`)");
    path
}

fn write(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("write");
    path
}

#[test]
fn plain_and_gzipped_forms_yield_identical_headers_and_records() {
    let plain = open_tracking(&data("tracking_valid", "tracks.json")).expect("open .json");
    let gzipped = open_tracking(&data("tracking_valid", "tracks.json.gz")).expect("open .json.gz");
    assert_eq!(plain.header.format, TRACKING_FORMAT);
    assert_eq!(plain.header.version, 1);
    assert_eq!(
        plain.header.metadata.shape_tczyx,
        Some([4, 2, 4, 32, 32]),
        "metadata is parsed before any record"
    );
    assert_eq!(plain.header, gzipped.header);

    let plain: Vec<CellRecord> = plain
        .records
        .collect::<Result<_, _>>()
        .expect("every record parses");
    let gzipped: Vec<CellRecord> = gzipped
        .records
        .collect::<Result<_, _>>()
        .expect("every record parses");
    assert_eq!(plain.len(), 24);
    assert_eq!(plain, gzipped, "compression never changes content");

    let first = &plain[0];
    assert_eq!((first.id, first.t, first.seg_id), (1, 0, Some(1)));
    assert_eq!(first.children.len(), 1);
    assert_eq!(first.children[0].confidence, Some(0.955));
    assert_eq!(first.labels, vec!["ESI".to_owned(), "treated".to_owned()]);
}

#[test]
fn progress_probe_reaches_one_after_the_stream_drains() {
    let stream = open_tracking(&data("tracking_valid", "tracks.json.gz")).expect("open");
    let probe = stream.progress_probe();
    let count = stream.records.count();
    assert_eq!(count, 24);
    // a tiny file is buffered whole at the header; the bound that matters is the cap
    let fraction = probe.fraction();
    assert!((0.9..=1.0).contains(&fraction), "{fraction}");
}

#[test]
fn a_bad_header_fails_before_any_record_is_yielded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        (
            "wrong-format.json",
            r#"{"format": "other-tracking", "version": 1, "metadata": {}, "cells": [{"id": 1, "t": 0}]}"#,
            "other-tracking",
        ),
        (
            "wrong-version.json",
            r#"{"format": "cellstudio-tracking", "version": 2, "metadata": {}, "cells": []}"#,
            "version 2",
        ),
        (
            "no-metadata.json",
            r#"{"format": "cellstudio-tracking", "version": 1, "cells": []}"#,
            "`metadata` must precede `cells`",
        ),
        (
            "no-cells.json",
            r#"{"format": "cellstudio-tracking", "version": 1, "metadata": {}}"#,
            "no `cells` array",
        ),
        ("not-an-object.json", r#"[1, 2, 3]"#, "not a JSON object"),
    ];
    for (name, content, expected) in cases {
        let Err(err) = open_tracking(&write(&dir, name, content)) else {
            panic!("{name}: opened despite its header");
        };
        assert!(
            matches!(&err, TrackingOpenError::Header(m) if m.contains(expected)),
            "{name}: {err}"
        );
    }
}

#[test]
fn a_record_failure_mid_stream_surfaces_after_the_good_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    // record 2 has an id that does not fit u32; the two before it stream fine
    let path = write(
        &dir,
        "bad-record.json",
        r#"{"format": "cellstudio-tracking", "version": 1, "metadata": {},
            "cells": [{"id": 1, "t": 0}, {"id": 2, "t": 1}, {"id": 4294967296, "t": 2}, {"id": 3, "t": 3}]}"#,
    );
    let stream = open_tracking(&path).expect("header is fine");
    let mut records = stream.records;
    assert_eq!(records.next().expect("first").expect("parses").id, 1);
    assert_eq!(records.next().expect("second").expect("parses").id, 2);
    let error = records
        .next()
        .expect("third")
        .expect_err("does not fit u32");
    assert_eq!(error.index, 2);
    assert!(
        records.next().is_none(),
        "the first error fuses the iterator"
    );

    // a file truncated mid-record errors instead of hanging or looping
    let truncated = write(
        &dir,
        "truncated.json",
        r#"{"format": "cellstudio-tracking", "version": 1, "metadata": {},
            "cells": [{"id": 1, "t": 0}, {"id": 2, "#,
    );
    let stream = open_tracking(&truncated).expect("header is fine");
    let results: Vec<_> = stream.records.collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok());
    assert!(
        results[1].is_err(),
        "the truncation is an error, not an end"
    );
}
