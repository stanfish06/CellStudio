//! Crash recovery at every journal boundary. A normal integration test cannot kill the
//! process mid-write, so each boundary is reproduced exactly: the journal row and its chunk
//! snapshots are written, the store is damaged as far as that boundary would have taken it,
//! and a fresh coordinator recovers the project the way `open_validated` does.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use cellstudio_core::axes::Axis;
use cellstudio_core::labels::{
    self, ChunkKey, EditFootprint, LabelStore, StrokeMode, StrokeSpec, VoxelBox, VoxelSet,
};
use cellstudio_core::reader::ImageReader;
use cellstudio_db::Project;
use cellstudio_db::queries::{ChunkSnapshot, EditDomain, ExtentDelta};
use cellstudio_server::edit::{MaskCommand, ProjectEditCoordinator, Stroke};
use cellstudio_server::events::EventBus;
use serde_json::{Value, json};
use support::{data_copy, store_snapshot};

const SESSION: &str = "test-session";
/// The frame the interrupted edit lands on: its level-0 chunk has no object beforehand,
/// which is the case whose inverse is an erase rather than a write of encoded zeros.
const CRASH_T: u64 = 2;

struct Harness {
    _dir: tempfile::TempDir,
    project: Arc<Project>,
    image: Arc<ImageReader>,
    labels: PathBuf,
}

impl Harness {
    /// A project with one committed stroke behind it, so recovery has a populated store to
    /// roll an interrupted second edit back into.
    fn open() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
        let project = Arc::new(Project::create_or_open(&dataset).expect("project"));
        let image = Arc::new(ImageReader::new(
            Arc::new(cellstudio_core::open(&dataset).expect("dataset")),
            8 << 20,
        ));
        let coordinator = ProjectEditCoordinator::new(project.clone(), EventBus::new());
        let labels = project.labels_store_path();

        let first = coordinator.reserve(SESSION, 8).expect("lease").0;
        coordinator
            .execute(
                &image,
                SESSION,
                MaskCommand::Stroke(Stroke {
                    t: 0,
                    label: first,
                    erase: false,
                    radius: 3.0,
                    plane: Some(Axis::Z),
                    stamps: vec![[1.5, 8.5, 8.5]],
                    only: None,
                }),
            )
            .expect("the first stroke commits");

        Self {
            _dir: dir,
            project,
            image,
            labels,
        }
    }

    fn store(&self) -> LabelStore {
        labels::open_store(&self.labels, self.image.dataset()).expect("label store")
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        store_snapshot(&self.labels)
    }

    /// What `open_validated` does on the next launch.
    fn reopen(&self) -> usize {
        ProjectEditCoordinator::new(self.project.clone(), EventBus::new())
            .recover(self.image.dataset())
            .expect("recovery")
    }
}

fn spec(label: u32) -> StrokeSpec {
    StrokeSpec {
        mode: StrokeMode::Paint { label },
        radius: 3.0,
        plane: Some((Axis::Z, 1)),
        centres: vec![[1.5, 20.5, 20.5]],
    }
}

fn op(label: u32, bbox: VoxelBox) -> Value {
    json!({
        "kind": "stroke",
        "t": CRASH_T,
        "label": label,
        "erase": false,
        "radius": 3.0,
        "plane": ["z", 1],
        "stamps": [[1.5, 20.5, 20.5]],
        "only": null,
        "bbox": {
            "z0": bbox.z0, "z1": bbox.z1,
            "y0": bbox.y0, "y1": bbox.y1,
            "x0": bbox.x0, "x1": bbox.x1,
        },
    })
}

/// The zarr store key of a chunk, which is what `edit_blobs.chunk_key` holds.
fn chunk_key(key: &ChunkKey) -> String {
    format!(
        "{}/c/{}/0/{}/{}/{}",
        key.level, key.t, key.grid[0], key.grid[1], key.grid[2]
    )
}

fn affected(store: &LabelStore, set: &VoxelSet) -> Vec<ChunkKey> {
    let chunks = store.chunks(0).expect("chunks");
    let mut keys: Vec<ChunkKey> = set
        .iter()
        .map(|v| ChunkKey {
            level: 0,
            t: CRASH_T,
            grid: [v[0] / chunks.z, v[1] / chunks.y, v[2] / chunks.x],
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Journals the interrupted edit exactly as the coordinator does, and returns its row.
fn journal(harness: &Harness, store: &LabelStore, label: u32) -> (i64, VoxelBox, StrokeSpec) {
    let spec = spec(label);
    let dims = store.dims(0).expect("dims");
    let set = spec.rasterize(store.scale(), [dims.z, dims.y, dims.x]);
    let bbox = set.bounds().expect("the stamp lands inside the volume");
    let keys = affected(store, &set);
    let snaps = labels::snapshot(store, &keys).expect("snapshot");
    assert!(
        snaps.iter().all(|snap| !snap.existed),
        "the interrupted edit is the first paint in its region"
    );
    let blobs: Vec<ChunkSnapshot> = snaps
        .iter()
        .map(|snap| ChunkSnapshot {
            chunk_key: chunk_key(&snap.key),
            existed: snap.existed,
            before: snap.bytes.clone(),
        })
        .collect();
    let seq = harness
        .project
        .db
        .record_edit_pending(
            EditDomain::Mask,
            &op(label, bbox),
            &json!({ "kind": "restore-chunks" }),
            &blobs,
        )
        .expect("journal row");
    (seq, bbox, spec)
}

fn deltas_of(footprint: &EditFootprint) -> Vec<ExtentDelta> {
    footprint
        .deltas
        .iter()
        .map(|delta| ExtentDelta {
            label: delta.label,
            area: delta.area,
            sum_z: delta.sum_z,
            sum_y: delta.sum_y,
            sum_x: delta.sum_x,
            bbox: None,
        })
        .collect()
}

#[test]
fn a_crash_between_the_journal_row_and_the_first_chunk_leaves_no_trace() {
    let harness = Harness::open();
    let store = harness.store();
    let baseline = harness.snapshot();
    let (seq, ..) = journal(&harness, &store, 900);

    assert_eq!(harness.reopen(), 1);
    assert_eq!(
        harness.snapshot(),
        baseline,
        "the store is untouched at every level"
    );
    assert!(
        harness
            .project
            .db
            .pending_edits()
            .expect("pending")
            .is_empty()
    );
    assert!(
        harness
            .project
            .db
            .take_blobs(seq)
            .expect("blobs")
            .is_empty(),
        "the rolled-back row and its snapshots are gone"
    );
}

#[test]
fn a_crash_after_the_level_zero_chunks_rolls_them_back_and_erases_the_absent_one() {
    let harness = Harness::open();
    let store = harness.store();
    let baseline = harness.snapshot();
    let (_seq, _bbox, spec) = journal(&harness, &store, 901);

    let footprint = labels::apply(&store, CRASH_T, &spec).expect("level-0 write");
    assert!(!footprint.chunks.is_empty());
    assert_ne!(harness.snapshot(), baseline, "the crash point is real");

    assert_eq!(harness.reopen(), 1);
    assert_eq!(harness.snapshot(), baseline);
    assert!(
        !harness.labels.join("0/c/2/0/0/0/0").exists(),
        "a chunk that did not exist before the edit is absent again, not a zero chunk"
    );
}

#[test]
fn a_crash_after_coarse_regeneration_rolls_back_every_level() {
    let harness = Harness::open();
    let store = harness.store();
    let baseline = harness.snapshot();
    let (_seq, bbox, spec) = journal(&harness, &store, 902);

    labels::apply(&store, CRASH_T, &spec).expect("level-0 write");
    let coarse = labels::regenerate_coarse(&store, CRASH_T, bbox).expect("coarse");
    assert!(!coarse.is_empty(), "the coarse levels hold the stroke now");
    let damaged = harness.snapshot();
    assert!(
        damaged.keys().any(|path| path.starts_with("1")),
        "a coarse object was written: {:?}",
        damaged.keys().collect::<Vec<_>>()
    );

    assert_eq!(harness.reopen(), 1);
    assert_eq!(
        harness.snapshot(),
        baseline,
        "restoring level 0 alone would leave the coarse levels holding the uncommitted stroke"
    );
}

#[test]
fn an_edit_whose_final_transaction_committed_survives_recovery() {
    let harness = Harness::open();
    let store = harness.store();
    let (seq, bbox, spec) = journal(&harness, &store, 903);

    let footprint = labels::apply(&store, CRASH_T, &spec).expect("level-0 write");
    labels::regenerate_coarse(&store, CRASH_T, bbox).expect("coarse");
    let commit = harness
        .project
        .db
        .commit_edit(seq, CRASH_T, &deltas_of(&footprint), &[])
        .expect("commit");
    let committed = harness.snapshot();

    assert_eq!(harness.reopen(), 0, "a cleared row is not rolled back");
    assert_eq!(harness.snapshot(), committed);
    assert_eq!(
        harness.project.db.versions().expect("versions").labels,
        commit.version,
        "and the version does not move under it"
    );
}

#[test]
fn recovery_bumps_the_label_version_so_a_client_refetches() {
    let harness = Harness::open();
    let store = harness.store();
    let before = harness.project.db.versions().expect("versions").labels;
    journal(&harness, &store, 904);

    assert_eq!(harness.reopen(), 1);
    assert!(
        harness.project.db.versions().expect("versions").labels > before,
        "a rolled-back edit changes what a client holds"
    );
}
