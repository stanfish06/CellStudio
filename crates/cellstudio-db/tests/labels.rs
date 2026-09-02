//! Cell- and track-scope labels: per-cell storage, journaled edits, undo/redo, survival
//! across topology edits, and the definition set.

use std::path::Path;

use cellstudio_db::{GraphError, LabelScope, Project, StoredLabel};
use rusqlite::Connection;

fn dataset(dir: &Path) -> std::path::PathBuf {
    dir.join("data.zarr")
}

/// Raw writes only while no `Project` holds the file exclusively.
fn raw(dir: &Path, sql: &str) {
    let conn = Connection::open(dir.join("data.cellstudio").join("tracks.sqlite")).expect("open");
    conn.execute_batch(sql).expect("seed");
}

fn seeded(sql: &str) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().expect("tempdir");
    drop(Project::create_or_open(&dataset(dir.path())).expect("create project"));
    raw(dir.path(), sql);
    let project = Project::create_or_open(&dataset(dir.path())).expect("reopen");
    (dir, project)
}

/// Closes `project`, runs `sql` on the file, and reopens.
fn reopen_with(dir: &Path, project: Project, sql: &str) -> Project {
    drop(project);
    raw(dir, sql);
    Project::create_or_open(&dataset(dir)).expect("reopen")
}

/// Two chains: 1→2→3 (track 1) and 4→5 (track 2), plus lone cell 6.
const GRAPH: &str = r#"
INSERT INTO cells(id, t, z, y, x, area, track_id) VALUES
  (1, 0, 1.0, 10.0, 10.0, 10, 1),
  (2, 1, 1.0, 11.0, 11.0, 10, 1),
  (3, 2, 1.0, 12.0, 12.0, 10, 1),
  (4, 3, 1.0, 13.0, 13.0, 10, 2),
  (5, 4, 1.0, 14.0, 14.0, 10, 2),
  (6, 0, 1.0, 50.0, 50.0, 10, 3);
INSERT INTO links(parent, child) VALUES (1, 2), (2, 3), (4, 5);
"#;

fn s(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

fn sets(project: &Project, id: u32) -> (Vec<String>, Vec<String>) {
    let row = project
        .db
        .cells_window(0, 99, None)
        .expect("cells")
        .into_iter()
        .find(|c| c.id == id)
        .expect("cell present");
    (row.labels, row.track_labels)
}

#[test]
fn cell_scope_touches_one_row() {
    let (_dir, project) = seeded(GRAPH);
    let commit = project
        .db
        .graph_set_labels(2, LabelScope::Cell, &s(&["verified"]), &[])
        .expect("set");
    assert_eq!(
        commit.cells.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![2]
    );
    assert!(commit.affected_tracks.is_empty(), "identities do not move");
    assert_eq!(sets(&project, 2), (s(&["verified"]), vec![]));
    assert_eq!(sets(&project, 1), (vec![], vec![]));
    assert_eq!(sets(&project, 3), (vec![], vec![]));
}

#[test]
fn track_scope_touches_every_chain_cell_and_no_neighbour() {
    let (_dir, project) = seeded(GRAPH);
    let commit = project
        .db
        .graph_set_labels(2, LabelScope::Track, &s(&["cell type 1"]), &[])
        .expect("set");
    let mut touched: Vec<u32> = commit.cells.iter().map(|c| c.id).collect();
    touched.sort_unstable();
    assert_eq!(touched, vec![1, 2, 3]);
    for id in [1, 2, 3] {
        assert_eq!(sets(&project, id), (vec![], s(&["cell type 1"])));
    }
    for id in [4, 5, 6] {
        assert_eq!(sets(&project, id), (vec![], vec![]));
    }
    let entries = project.db.edits(10).expect("edits");
    assert_eq!(
        entries[0].scope.as_deref(),
        Some("+cell type 1 · track 1 (3 cells)")
    );
}

#[test]
fn a_partial_chain_fills_on_add_and_clears_on_remove() {
    let (_dir, project) = seeded(&format!(
        "{GRAPH}UPDATE cells SET track_labels = '[\"cell type 1\"]' WHERE id = 1;"
    ));
    let commit = project
        .db
        .graph_set_labels(3, LabelScope::Track, &s(&["cell type 1"]), &[])
        .expect("fill");
    let mut touched: Vec<u32> = commit.cells.iter().map(|c| c.id).collect();
    touched.sort_unstable();
    assert_eq!(
        touched,
        vec![2, 3],
        "the already-labeled head is not rewritten"
    );
    project
        .db
        .graph_set_labels(1, LabelScope::Track, &[], &s(&["cell type 1"]))
        .expect("clear");
    for id in [1, 2, 3] {
        assert_eq!(sets(&project, id), (vec![], vec![]));
    }
}

#[test]
fn a_no_op_is_rejected_without_a_journal_row() {
    let (_dir, project) = seeded(GRAPH);
    let err = project
        .db
        .graph_set_labels(2, LabelScope::Cell, &[], &s(&["never set"]))
        .expect_err("nothing to do");
    assert!(matches!(err, GraphError::NoChange(2)), "{err}");
    assert!(project.db.edits(10).expect("edits").is_empty());
    let err = project
        .db
        .graph_set_labels(2, LabelScope::Cell, &s(&["  "]), &[])
        .expect_err("blank name");
    assert!(err.to_string().contains("label needs a name"), "{err}");
}

#[test]
fn undo_and_redo_restore_exact_arrays() {
    let (_dir, project) = seeded(&format!(
        "{GRAPH}UPDATE cells SET labels = '[\"ESI\"]', track_labels = '[\"old\"]' WHERE id = 2;"
    ));
    let commit = project
        .db
        .graph_set_labels(2, LabelScope::Track, &s(&["cell type 1"]), &s(&["old"]))
        .expect("set");
    assert_eq!(sets(&project, 2), (s(&["ESI"]), s(&["cell type 1"])));
    assert_eq!(sets(&project, 1), (vec![], s(&["cell type 1"])));

    project.db.graph_step(commit.seq, true).expect("undo");
    assert_eq!(sets(&project, 2), (s(&["ESI"]), s(&["old"])));
    assert_eq!(sets(&project, 1), (vec![], vec![]));
    assert_eq!(sets(&project, 3), (vec![], vec![]));

    project.db.graph_step(commit.seq, false).expect("redo");
    assert_eq!(sets(&project, 2), (s(&["ESI"]), s(&["cell type 1"])));
    assert_eq!(sets(&project, 3), (vec![], s(&["cell type 1"])));
}

#[test]
fn a_journal_row_without_label_fields_still_steps() {
    let (dir, project) = seeded(GRAPH);
    let commit = project.db.graph_cut(1, 2).expect("cut");
    // rewrite the row the way a pre-labels build wrote it
    let project = reopen_with(
        dir.path(),
        project,
        &format!(
            r#"UPDATE edits SET op = json_remove(json_remove(op, '$.delta.labels_before'), '$.delta.labels_after') WHERE seq = {seq};"#,
            seq = commit.seq
        ),
    );
    project
        .db
        .graph_step(commit.seq, true)
        .expect("undo old row");
    assert_eq!(
        project
            .db
            .cells_window(0, 99, None)
            .expect("cells")
            .iter()
            .filter(|c| c.track_id == Some(1))
            .count(),
        3,
        "the cut is undone from a row that predates label deltas"
    );
}

#[test]
fn strip_removes_a_name_from_both_scopes_and_undo_restores_it() {
    let (_dir, project) = seeded(&format!(
        r#"{GRAPH}
UPDATE cells SET labels = '["verified","ESI"]' WHERE id = 1;
UPDATE cells SET track_labels = '["verified"]' WHERE id = 4;
UPDATE cells SET labels = '["verified"]', track_labels = '["verified","x"]' WHERE id = 6;
"#
    ));
    let commit = project
        .db
        .graph_strip_label("verified")
        .expect("strip")
        .expect("some cell carried it");
    let mut touched: Vec<u32> = commit.cells.iter().map(|c| c.id).collect();
    touched.sort_unstable();
    assert_eq!(touched, vec![1, 4, 6]);
    assert_eq!(sets(&project, 1), (s(&["ESI"]), vec![]));
    assert_eq!(sets(&project, 4), (vec![], vec![]));
    assert_eq!(sets(&project, 6), (vec![], s(&["x"])));
    assert_eq!(
        project.db.edits(10).expect("edits")[0].scope.as_deref(),
        Some("strip verified (3 cells)")
    );

    project.db.graph_step(commit.seq, true).expect("undo");
    assert_eq!(
        sets(&project, 1),
        (s(&["verified", "ESI"]), vec![]),
        "exact original order"
    );
    assert_eq!(sets(&project, 4), (vec![], s(&["verified"])));
    assert_eq!(sets(&project, 6), (s(&["verified"]), s(&["verified", "x"])));

    assert!(
        project
            .db
            .graph_strip_label("nobody")
            .expect("strip")
            .is_none(),
        "an unused name journals nothing"
    );
}

#[test]
fn labels_survive_link_cut_and_unlink_and_their_undo() {
    let (_dir, project) = seeded(GRAPH);
    project
        .db
        .graph_set_labels(2, LabelScope::Track, &s(&["cell type 1"]), &[])
        .expect("label chain 1");
    project
        .db
        .graph_set_labels(5, LabelScope::Cell, &s(&["verified"]), &[])
        .expect("label cell 5");
    let snapshot = |p: &Project| (1..=6).map(|id| (id, sets(p, id))).collect::<Vec<_>>();
    let expected = snapshot(&project);

    let link = project.db.graph_link(3, 4).expect("link joins the chains");
    assert_eq!(snapshot(&project), expected, "after link");
    project.db.graph_step(link.seq, true).expect("undo link");
    assert_eq!(snapshot(&project), expected, "after undo link");

    let cut = project.db.graph_cut(1, 2).expect("cut");
    assert_eq!(snapshot(&project), expected, "after cut");
    project.db.graph_step(cut.seq, true).expect("undo cut");
    assert_eq!(snapshot(&project), expected, "after undo cut");

    let unlink = project.db.graph_unlink(4).expect("unlink");
    assert_eq!(snapshot(&project), expected, "after unlink");
    project
        .db
        .graph_step(unlink.seq, true)
        .expect("undo unlink");
    assert_eq!(snapshot(&project), expected, "after undo unlink");
}

#[test]
fn definitions_union_stored_and_in_use_names() {
    let (_dir, project) = seeded(&format!(
        r#"{GRAPH}
UPDATE cells SET labels = '["treated"]' WHERE id = 1;
UPDATE cells SET labels = '["treated"]', track_labels = '["treated","cell type 1"]' WHERE id = 2;
"#
    ));
    let before = project.db.versions().expect("versions").settings;
    project
        .db
        .put_label_definitions(&[
            StoredLabel {
                name: "verified".into(),
                color: Some("#FFAA00".into()),
            },
            StoredLabel {
                name: "cell type 1".into(),
                color: None,
            },
        ])
        .expect("put");
    assert!(project.db.versions().expect("versions").settings > before);

    let defs = project.db.label_definitions().expect("definitions");
    let listed: Vec<(&str, u64, Option<&str>)> = defs
        .iter()
        .map(|d| (d.name.as_str(), d.uses, d.color.as_deref()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("cell type 1", 1, None),
            ("treated", 2, None),
            ("verified", 0, Some("#ffaa00")),
        ],
        "a cell carrying a name in both scopes counts once; colours normalise to lower case"
    );

    let plain = |name: &str| StoredLabel {
        name: name.into(),
        color: None,
    };
    let err = project
        .db
        .put_label_definitions(&[plain("a"), plain(" a ")])
        .expect_err("duplicate");
    assert!(err.to_string().contains("listed twice"), "{err}");
    let err = project
        .db
        .put_label_definitions(&[plain("")])
        .expect_err("empty");
    assert!(err.to_string().contains("needs a name"), "{err}");
    let err = project
        .db
        .put_label_definitions(&[StoredLabel {
            name: "a".into(),
            color: Some("red".into()),
        }])
        .expect_err("bad colour");
    assert!(err.to_string().contains("#rrggbb"), "{err}");
}
