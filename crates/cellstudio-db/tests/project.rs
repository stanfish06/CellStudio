//! Project container: source-store read-only, exclusive open, what a reopen restores.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cellstudio_core::axes::PhysicalScale;
use cellstudio_db::{DbError, OpenError, Project, VersionCounter, Versions};

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir {
        modified: SystemTime,
    },
    File {
        modified: SystemTime,
        bytes: Vec<u8>,
    },
}

/// Every path under `root` with its mtime (and contents for files), so a diff catches any change.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = std::fs::metadata(&dir).expect("stat directory");
        out.insert(
            dir.clone(),
            Entry::Dir {
                modified: meta.modified().expect("directory mtime"),
            },
        );
        for entry in std::fs::read_dir(&dir).expect("read directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            let meta = entry.metadata().expect("stat entry");
            if meta.is_dir() {
                stack.push(path);
            } else {
                out.insert(
                    path.clone(),
                    Entry::File {
                        modified: meta.modified().expect("file mtime"),
                        bytes: std::fs::read(&path).expect("read file"),
                    },
                );
            }
        }
    }
    out
}

/// A source store shaped like a zarr v2 array. Nothing in `cellstudio-db` reads it: the
/// caller validates the dataset, this crate only names the container after it.
fn source_store(parent: &Path) -> PathBuf {
    let root = parent.join("data.zarr");
    std::fs::create_dir_all(root.join("0")).expect("create store");
    std::fs::write(root.join(".zgroup"), br#"{"zarr_format":2}"#).expect("write .zgroup");
    std::fs::write(root.join(".zattrs"), br#"{"multiscales":[]}"#).expect("write .zattrs");
    std::fs::write(root.join("0/.zarray"), br#"{"shape":[1,1,1,2,2]}"#).expect("write .zarray");
    std::fs::write(root.join("0/0.0.0.0.0"), [1u8, 2, 3, 4]).expect("write chunk");
    root
}

#[test]
fn first_open_creates_the_container_without_touching_the_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = source_store(dir.path());
    let before = snapshot(&dataset);

    let project = Project::create_or_open(&dataset).expect("create project");

    assert_eq!(
        project.root,
        dir.path().join("data.cellstudio"),
        "the container sits next to the dataset, named without the .zarr extension"
    );
    assert!(project.root.join("project.json").is_file());
    assert!(project.root.join("tracks.sqlite").is_file());
    assert!(project.cache_dir().is_dir());
    assert!(
        !project.labels_store_path().exists(),
        "labels.zarr appears only at mask import"
    );
    assert!(!project.has_labels());

    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.root.join("project.json")).expect("read"))
            .expect("parse project.json");
    assert_eq!(meta["format"], "cellstudio-project");
    assert_eq!(meta["version"], 1);
    assert_eq!(meta["source"], dataset.to_string_lossy().as_ref());
    assert_eq!(project.meta.source, dataset);
    assert!(
        meta.get("voxel_size_override").is_none(),
        "mutable project state lives in the database, not project.json: {meta}"
    );

    drop(project);
    assert_eq!(
        snapshot(&dataset),
        before,
        "no file inside the source store was created, modified, or deleted"
    );
}

#[test]
fn reopen_restores_settings_and_versions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = source_store(dir.path());

    let settings = serde_json::json!({
        "activeView": "xz",
        "views": { "xz": { "index": 17, "camera": { "target": [512, 512], "zoom": 1.5 } } },
        "channels": [{ "name": "DAPI", "visible": true, "window": [120, 4096], "gamma": 0.8 }],
    });

    let project = Project::create_or_open(&dataset).expect("create project");
    assert_eq!(
        project.db.settings().expect("settings"),
        serde_json::json!({}),
        "a fresh project starts with an empty settings blob"
    );
    assert_eq!(
        project.db.versions().expect("versions"),
        Versions::default()
    );

    assert_eq!(
        project.db.put_settings(&settings).expect("put settings"),
        1,
        "a settings write bumps the settings counter"
    );
    project.db.bump(VersionCounter::Graph).expect("bump");
    project.db.bump(VersionCounter::Graph).expect("bump");
    project.db.bump(VersionCounter::Labels).expect("bump");
    let expected = Versions {
        image: 0,
        labels: 1,
        graph: 2,
        settings: 1,
    };
    assert_eq!(project.db.versions().expect("versions"), expected);
    drop(project);

    let reopened = Project::create_or_open(&dataset).expect("reopen project");
    assert_eq!(reopened.db.settings().expect("settings"), settings);
    assert_eq!(reopened.db.versions().expect("versions"), expected);
    assert_eq!(reopened.meta.source, dataset);
    assert_eq!(
        reopened
            .db
            .put_settings(&serde_json::json!({"activeView": "3d"}))
            .expect("put settings"),
        2,
        "counters continue across reopens"
    );
}

#[test]
fn the_container_name_drops_a_trailing_zarr_extension_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (dataset, container) in [
        ("embryo_04.zarr", "embryo_04.cellstudio"),
        ("embryo_05.ZARR", "embryo_05.cellstudio"),
        ("stack.ome.zarr", "stack.ome.cellstudio"),
        ("weird_name", "weird_name.cellstudio"),
        ("masks.tif", "masks.tif.cellstudio"),
    ] {
        let project = Project::create_or_open(&dir.path().join(dataset)).expect("create project");
        assert_eq!(
            project.root,
            dir.path().join(container),
            "container name for dataset {dataset}"
        );
    }
}

#[test]
fn the_voxel_size_override_round_trips_through_the_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = source_store(dir.path());
    let scale = PhysicalScale {
        z: 2.0,
        y: 0.603,
        x: 0.603,
    };

    let project = Project::create_or_open(&dataset).expect("create project");
    assert_eq!(project.db.voxel_size_override().expect("override"), None);
    assert_eq!(
        project
            .db
            .put_voxel_size_override(Some(scale))
            .expect("put override"),
        1,
        "the write bumps the settings counter"
    );
    assert_eq!(
        project.db.voxel_size_override().expect("override"),
        Some(scale)
    );
    drop(project);

    let reopened = Project::create_or_open(&dataset).expect("reopen project");
    assert_eq!(
        reopened.db.voxel_size_override().expect("override"),
        Some(scale)
    );
    assert_eq!(reopened.db.versions().expect("versions").settings, 1);
    assert_eq!(
        reopened.db.put_voxel_size_override(None).expect("clear"),
        2,
        "clearing is a write like any other"
    );
    assert_eq!(reopened.db.voxel_size_override().expect("override"), None);
    assert_eq!(
        reopened.db.settings().expect("settings"),
        serde_json::json!({}),
        "the override and the renderer blob are separate keys"
    );
}

/// Exclusive transactional data ownership: concurrent edits serialize
/// and every acknowledgment carries its own increasing version.
#[test]
fn concurrent_edits_serialize_into_distinct_increasing_versions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = source_store(dir.path());
    let project = Arc::new(Project::create_or_open(&dataset).expect("create project"));

    let writers: Vec<_> = (0..8)
        .map(|writer| {
            let project = Arc::clone(&project);
            std::thread::spawn(move || {
                project
                    .db
                    .put_settings(&serde_json::json!({ "writer": writer }))
                    .expect("put settings")
            })
        })
        .collect();
    let mut versions: Vec<u64> = writers
        .into_iter()
        .map(|w| w.join().expect("writer thread"))
        .collect();
    versions.sort_unstable();

    assert_eq!(
        versions,
        (1..=8).collect::<Vec<_>>(),
        "every write got its own version, so none was lost or duplicated"
    );
    assert_eq!(project.db.versions().expect("versions").settings, 8);
    let stored = project.db.settings().expect("settings");
    assert!(
        stored["writer"].as_u64().is_some_and(|w| w < 8),
        "the stored blob is one writer's, not a mix: {stored}"
    );
}

#[test]
fn only_one_connection_may_hold_the_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = source_store(dir.path());

    let first = Project::create_or_open(&dataset).expect("create project");
    let second = Project::create_or_open(&dataset);
    assert!(
        matches!(second, Err(OpenError::Db(DbError::AlreadyOpen(_)))),
        "expected an exclusive-open refusal, got {second:?}"
    );
    assert!(
        first.root.join("project.json").is_file() && first.root.join("tracks.sqlite").is_file(),
        "a refused open must leave the existing container intact"
    );

    drop(first);
    Project::create_or_open(&dataset).expect("reopen after the holder closed");
}

#[test]
fn reopening_after_the_project_moved_records_the_new_source_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old = dir.path().join("old");
    std::fs::create_dir(&old).expect("create parent");
    let dataset = source_store(&old);
    drop(Project::create_or_open(&dataset).expect("create project"));

    let new = dir.path().join("new");
    std::fs::rename(&old, &new).expect("move the dataset and its project together");
    let moved = new.join("data.zarr");

    let project = Project::create_or_open(&moved).expect("reopen at the new path");
    assert_eq!(project.meta.source, moved);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.root.join("project.json")).expect("read"))
            .expect("parse project.json");
    assert_eq!(meta["source"], moved.to_string_lossy().as_ref());
}

#[test]
fn create_or_open_does_not_validate_the_dataset() {
    // Validation is the caller's job: the server rejects an unreadable store before
    // reaching here, which is what keeps an invalid dataset from leaving a project behind.
    let dir = tempfile::tempdir().expect("tempdir");
    let project =
        Project::create_or_open(&dir.path().join("nothing-here.zarr")).expect("create project");
    assert!(project.root.join("tracks.sqlite").is_file());
}
