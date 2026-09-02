//! The project's one writer of `labels.zarr`.
//!
//! [`ProjectEditCoordinator`] is `Arc`-held beside the `Project` and travels with it across a
//! same-store reopen, which builds a new [`crate::state::ActiveProject`] around the same
//! directory. It owns the write lock, the label-read lock, the store handle and
//! its registration, reservation leases, the journal protocol, invalidation, the version bump,
//! and event publication. Routes translate wire types and call [`ProjectEditCoordinator::execute`].

use std::collections::HashMap;
use std::sync::Arc;

use cellstudio_core::LayerId;
use cellstudio_core::axes::{Axis, Dims};
use cellstudio_core::bricks::BrickKey;
use cellstudio_core::dataset::Dataset;
use cellstudio_core::labels::{
    self, ChunkKey, EditFootprint, LabelDelta, LabelError, LabelStore, StrokeMode, StrokeSpec,
    VoxelBox, VoxelSet,
};
use cellstudio_core::reader::ImageReader;
use cellstudio_db::queries::{
    CellChange, CellRow, ChunkSnapshot, EditDomain, ExtentDelta, ExtentRow, GraphStep, MaskInverse,
};
use cellstudio_db::{DbError, GraphCommit, GraphError, LabelScope, Project};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{Event, EventBus};
use crate::wire::VersionsWire;

/// Ids handed out when the client names no count.
pub const RESERVE_DEFAULT: u32 = 64;
/// One request may not drain the id space.
const RESERVE_MAX: u32 = 4096;
/// Mask edits whose chunk snapshots stay undoable.
const BLOB_KEEP: u32 = 50;
/// Stamps per stroke; the client flushes at this bound.
const MAX_STAMPS: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error(transparent)]
    Labels(#[from] LabelError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Dataset(#[from] cellstudio_core::dataset::OpenError),
    #[error("{0}")]
    Invalid(String),
    #[error(
        "label {0} was neither reserved by this session nor present on that frame; \
         reserve a block with POST /mask/reserve first"
    )]
    Unreserved(u32),
    #[error("no project label store exists, so there is nothing to {0}")]
    NoStore(&'static str),
    #[error("nothing to {0}")]
    NothingTo(&'static str),
    #[error("edit {0} is past the undo window: its chunk snapshots were pruned")]
    Pruned(i64),
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("journal row {seq} is not readable: {source}")]
    Journal { seq: i64, source: serde_json::Error },
}

/// One stroke as the route parsed it: stamp centres in fractional level-0 voxels.
#[derive(Debug, Clone)]
pub struct Stroke {
    pub t: u64,
    pub label: u32,
    pub erase: bool,
    pub radius: f64,
    /// The axis a slice-view disk is pinned to; `None` is a 3D orb.
    pub plane: Option<Axis>,
    pub stamps: Vec<[f64; 3]>,
    /// Eraser scope: clear only this label, or whatever it touches when `None`.
    pub only: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum MaskCommand {
    Stroke(Stroke),
    Delete { t: u64, label: u32 },
}

#[derive(Debug, Clone)]
pub enum GraphCommand {
    Link {
        parent_id: u32,
        child_id: u32,
    },
    Unlink {
        cell_id: u32,
    },
    /// Cut one link, splitting its chain rather than deleting the whole track.
    Cut {
        parent_id: u32,
        child_id: u32,
    },
    /// Add and remove label names on one cell or on every cell of its chain.
    SetLabels {
        cell_id: u32,
        scope: LabelScope,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// Remove one name from every cell carrying it, in either scope.
    StripLabel {
        name: String,
    },
}

/// The coordinator's one external mutation interface Undo and redo dispatch on
/// the journal row's domain, so they are commands of their own rather than mask or graph ones.
#[derive(Debug, Clone)]
pub enum EditCommand {
    Mask(MaskCommand),
    Graph(GraphCommand),
    Undo,
    Redo,
}

/// One committed edit, discriminated the way the wire result is.
#[derive(Debug, Clone)]
pub enum EditOutcome {
    Mask(MaskCommit),
    Graph(GraphCommit),
}

/// What one committed edit tells the renderer.
#[derive(Debug, Clone, Default)]
pub struct MaskCommit {
    pub seq: i64,
    pub version: u64,
    pub has_labels: bool,
    pub cells: Vec<CellRow>,
    /// Cells that no longer exist: erased to nothing, or deleted.
    pub removed: Vec<u32>,
    pub chunks: Vec<String>,
    /// `version.graph` after the commit, when the mask edit changed the graph too.
    pub graph_version: Option<u64>,
    /// Track ids the graph step touched, when it did.
    pub affected_tracks: Option<Vec<u32>>,
}

/// The forward op, replayed by redo and read by recovery for the box its coarse levels
/// have to be regenerated over.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum MaskOp {
    Stroke {
        t: u64,
        label: u32,
        erase: bool,
        radius: f64,
        plane: Option<(Axis, u64)>,
        stamps: Vec<[f64; 3]>,
        only: Option<u32>,
        bbox: VoxelBox,
    },
    Delete {
        t: u64,
        label: u32,
        bbox: VoxelBox,
    },
}

impl MaskOp {
    fn t(&self) -> u64 {
        match self {
            MaskOp::Stroke { t, .. } | MaskOp::Delete { t, .. } => *t,
        }
    }

    fn bbox(&self) -> VoxelBox {
        match self {
            MaskOp::Stroke { bbox, .. } | MaskOp::Delete { bbox, .. } => *bbox,
        }
    }
}

/// Owns the mask write path for one project: the write lock, the label store handle and its
/// registration, id leases, the journal protocol, cache invalidation and event publication.
/// Held by the `Project`, so a same-store reopen keeps one lock.
pub struct ProjectEditCoordinator {
    project: Arc<Project>,
    events: EventBus,
    /// One writer at a time over `labels.zarr` and the journal.
    write: Mutex<()>,
    /// Label reads take the read side for the span of an assembly, so a plane is never mixed
    /// from pre- and post-edit bricks.
    reads: RwLock<()>,
    store: Mutex<Option<Arc<LabelStore>>>,
    /// Reserved id blocks `(first, count)` per session.
    leases: Mutex<HashMap<String, Vec<(u32, u32)>>>,
}

impl std::fmt::Debug for ProjectEditCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectEditCoordinator")
            .field("project", &self.project.root)
            .finish()
    }
}

impl ProjectEditCoordinator {
    pub fn new(project: Arc<Project>, events: EventBus) -> Arc<Self> {
        Arc::new(Self {
            project,
            events,
            write: Mutex::new(()),
            reads: RwLock::new(()),
            store: Mutex::new(None),
            leases: Mutex::new(HashMap::new()),
        })
    }

    /// Held by a label read for the span of the plane or volume assembly.
    pub fn read_labels(&self) -> RwLockReadGuard<'_, ()> {
        self.reads.read()
    }

    pub fn has_labels(&self) -> bool {
        self.project.has_labels()
    }

    /// A block of ids this session may paint with. Creates no label store: selecting the
    /// brush on a project the user never paints leaves nothing behind.
    pub fn reserve(&self, session: &str, count: u32) -> Result<(u32, u32), EditError> {
        let count = count.clamp(1, RESERVE_MAX);
        let first = self.project.db.reserve_label_ids(count)?;
        let mut leases = self.leases.lock();
        // only one session is ever current, so every other session's blocks are dead
        leases.retain(|held, _| held == session);
        leases
            .entry(session.to_owned())
            .or_default()
            .push((first, count));
        Ok((first, count))
    }

    pub fn execute(
        &self,
        image: &Arc<ImageReader>,
        session: &str,
        command: EditCommand,
    ) -> Result<EditOutcome, EditError> {
        // every write waits for the adopted store's inventory: until the marker commits,
        // any write races the scan and any undo touches unrecorded labels
        if self.project.db.inventory_pending()? {
            return Err(EditError::Db(DbError::InventoryPending));
        }
        let _write = self.write.lock();
        match command {
            EditCommand::Mask(MaskCommand::Stroke(stroke)) => {
                self.stroke(image, session, stroke).map(EditOutcome::Mask)
            }
            EditCommand::Mask(MaskCommand::Delete { t, label }) => {
                self.delete(image, session, t, label).map(EditOutcome::Mask)
            }
            EditCommand::Graph(command) => self.graph(session, command),
            EditCommand::Undo => self.step(image, session, true),
            EditCommand::Redo => self.step(image, session, false),
        }
    }

    /// Link and Unlink: one call into the graph module's one-transaction commit, then the
    /// session-scoped event fan-out.
    fn graph(&self, session: &str, command: GraphCommand) -> Result<EditOutcome, EditError> {
        let commit = match command {
            GraphCommand::Link {
                parent_id,
                child_id,
            } => self.project.db.graph_link(parent_id, child_id)?,
            GraphCommand::Unlink { cell_id } => self.project.db.graph_unlink(cell_id)?,
            GraphCommand::Cut {
                parent_id,
                child_id,
            } => self.project.db.graph_cut(parent_id, child_id)?,
            GraphCommand::SetLabels {
                cell_id,
                scope,
                add,
                remove,
            } => self
                .project
                .db
                .graph_set_labels(cell_id, scope, &add, &remove)?,
            GraphCommand::StripLabel { name } => self
                .project
                .db
                .graph_strip_label(&name)?
                .ok_or(EditError::NothingTo("strip: no cell carries that label"))?,
        };
        self.announce_graph(
            session,
            commit.graph_version,
            commit.affected_tracks.clone(),
        );
        self.announce_versions(session);
        Ok(EditOutcome::Graph(commit))
    }

    /// Rolls back every edit a crash left `pending`, oldest first: the level-0 objects, then
    /// the coarse levels regenerated from them, then the row.
    /// Restoring level 0 alone would leave the coarse levels holding the uncommitted stroke
    ///.
    pub fn recover(&self, image: &Dataset) -> Result<usize, EditError> {
        let pending = self.project.db.pending_edits()?;
        if pending.is_empty() {
            return Ok(0);
        }
        // a pending row snapshots the store it was journaled against; when the store on
        // disk is not the one the inventory marker covers, restoring those bytes would
        // write another store's data — drop the rows and let the inventory rebuild
        if self.project.has_labels() {
            let identity =
                cellstudio_db::inventory::store_identity(&self.project.labels_store_path());
            if identity.is_none() || identity != self.project.db.inventory_marker()? {
                for record in &pending {
                    self.project.db.delete_edit(record.seq)?;
                }
                tracing::warn!(
                    dropped = pending.len(),
                    "dropped pending mask edits journaled against a replaced label store"
                );
                return Ok(0);
            }
        }
        let _write = self.write.lock();
        let store = self.open(image)?.ok_or(EditError::NoStore("recover"))?;
        let mut rolled = 0;
        for record in pending {
            let op = parse_op(record.seq, &record.op)?;
            let snaps = journal_chunks(&self.project.db.take_blobs(record.seq)?)?;
            labels::restore(&store, &snaps)?;
            labels::regenerate_coarse(&store, op.t(), op.bbox())?;
            // a pending row means the commit transaction never ran, so the database was never
            // changed and only the version bump is owed
            self.project
                .db
                .commit_edit(record.seq, op.t(), &[], &[], GraphStep::Rematerialize)?;
            self.project.db.delete_edit(record.seq)?;
            rolled += 1;
            tracing::info!(seq = record.seq, "rolled back an interrupted mask edit");
        }
        Ok(rolled)
    }

    fn stroke(
        &self,
        image: &Arc<ImageReader>,
        session: &str,
        stroke: Stroke,
    ) -> Result<MaskCommit, EditError> {
        if stroke.stamps.is_empty() {
            return Err(EditError::Invalid(
                "a stroke needs at least one stamp".into(),
            ));
        }
        if stroke.stamps.len() > MAX_STAMPS {
            return Err(EditError::Invalid(format!(
                "a stroke carries at most {MAX_STAMPS} stamps, got {}",
                stroke.stamps.len()
            )));
        }
        if !stroke.radius.is_finite() || stroke.radius <= 0.0 {
            return Err(EditError::Invalid("radius must be positive".into()));
        }

        let store = self.ensure(image)?;
        let dims = store.dims(0)?;
        // the whole stroke shares one slice, so the pinned index comes from its first stamp
        let plane = stroke
            .plane
            .map(|axis| (axis, plane_index(axis, stroke.stamps[0])));
        let spec = StrokeSpec {
            mode: match stroke.erase {
                true => StrokeMode::Erase { only: stroke.only },
                false => StrokeMode::Paint {
                    label: stroke.label,
                },
            },
            radius: stroke.radius,
            plane,
            centres: stroke.stamps.clone(),
        };
        let set = spec.rasterize(store.scale(), [dims.z, dims.y, dims.x]);
        let Some(bbox) = set.bounds() else {
            return self.unchanged();
        };

        if let StrokeMode::Paint { label } = spec.mode {
            let leased = self.authorize(session, stroke.t, label)?;
            // seeds the target and rejects one id on two frames before anything is written
            self.seed(&store, stroke.t, label, leased)?;
        }

        let chunks = affected_chunks(&store, stroke.t, &set)?;
        let snaps = labels::snapshot(&store, &chunks)?;
        let op = MaskOp::Stroke {
            t: stroke.t,
            label: stroke.label,
            erase: stroke.erase,
            radius: stroke.radius,
            plane,
            stamps: stroke.stamps,
            only: stroke.only,
            bbox,
        };
        let seq = self.journal(&op, &snaps)?;
        let t = op.t();
        self.finish(
            image,
            session,
            &store,
            seq,
            &op,
            &snaps,
            true,
            GraphStep::Rematerialize,
            |store| labels::apply(store, t, &spec),
        )
    }

    fn delete(
        &self,
        image: &Arc<ImageReader>,
        session: &str,
        t: u64,
        label: u32,
    ) -> Result<MaskCommit, EditError> {
        if label == 0 {
            return Err(EditError::Invalid("0 is background, not a cell".into()));
        }
        let Some(store) = self.open(image.dataset())? else {
            return Err(EditError::NoStore("delete"));
        };
        // an adopted cell has no row until it is touched; the bbox below is the delete scan
        self.seed(&store, t, label, false)?;
        let Some(extent) = self.project.db.extent_of(t, label)? else {
            return self.unchanged();
        };
        let Some(bbox) = extent.bbox.map(from_db_box) else {
            return self.unchanged();
        };
        let bbox = clip(bbox, store.dims(0)?);

        let chunks = affected_chunks(&store, t, &VoxelSet::from_box(bbox))?;
        let snaps = labels::snapshot(&store, &chunks)?;
        let op = MaskOp::Delete { t, label, bbox };
        let seq = self.journal(&op, &snaps)?;
        self.finish(
            image,
            session,
            &store,
            seq,
            &op,
            &snaps,
            true,
            GraphStep::Rematerialize,
            |store| labels::clear_label(store, t, label, bbox),
        )
    }

    /// Design M6 steps 3-6 plus M8's coarse regeneration, shared by stroke, delete and redo.
    /// Any failure past the journal row rolls the level-0 objects back here rather than
    /// leaving a `pending` row to hold the store until the next open. `fresh` is false for a
    /// redo, whose row and inverse payload are already journaled and still exact.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        image: &Arc<ImageReader>,
        session: &str,
        store: &LabelStore,
        seq: i64,
        op: &MaskOp,
        snaps: &[labels::ChunkSnapshot],
        fresh: bool,
        graph: GraphStep<'_>,
        write: impl FnOnce(&LabelStore) -> Result<EditFootprint, LabelError>,
    ) -> Result<MaskCommit, EditError> {
        let t = op.t();
        // held through the cache invalidation and any rollback: a reader that took it earlier
        // is done, and one that takes it later cannot find a pre-edit brick resident
        let _reads = self.reads.write();
        let result = (|| -> Result<(MaskCommit, Vec<String>), EditError> {
            let footprint = write(store)?;
            let coarse = labels::regenerate_coarse(store, t, op.bbox())?;
            for delta in &footprint.deltas {
                // a label this edit overwrote may have no row yet: seed it from the store as
                // it was, which is the post-edit scan less this edit's own delta.
                self.seed_from_delta(store, t, delta)?;
            }
            let deltas = extent_deltas(&footprint);
            let commit = self.project.db.commit_edit(seq, t, &deltas, &[], graph)?;
            if fresh {
                self.project.db.prune_blobs(BLOB_KEEP)?;
            }

            let touched = [footprint.chunks.clone(), coarse].concat();
            let chunks = self.invalidate(image, &touched);
            Ok((
                MaskCommit {
                    seq,
                    version: commit.version,
                    has_labels: true,
                    cells: updated(&commit.cells),
                    removed: removed(&commit.cells),
                    chunks: chunks.clone(),
                    graph_version: commit.graph_version,
                    affected_tracks: commit.graph.as_ref().map(|d| d.affected_tracks()),
                },
                chunks,
            ))
        })();

        match result {
            Ok((commit, chunks)) => {
                drop(_reads);
                self.announce(session, LayerId::Labels, chunks, &commit);
                Ok(commit)
            }
            Err(e) => {
                self.roll_back(store, seq, t, op.bbox(), snaps, fresh);
                Err(e)
            }
        }
    }

    /// The unified undo/redo: the newest un-undone (or oldest undone) row whatever its
    /// domain, dispatched on it.
    fn step(
        &self,
        image: &Arc<ImageReader>,
        session: &str,
        undo: bool,
    ) -> Result<EditOutcome, EditError> {
        let next = match undo {
            true => self.project.db.undo_next()?,
            false => self.project.db.redo_next()?,
        };
        let Some(record) = next else {
            return Err(EditError::NothingTo(if undo { "undo" } else { "redo" }));
        };
        match record.domain {
            EditDomain::Mask => self
                .step_mask(image, session, record, undo)
                .map(EditOutcome::Mask),
            EditDomain::Graph => {
                let commit = self.project.db.graph_step(record.seq, undo)?;
                self.announce_graph(
                    session,
                    commit.graph_version,
                    commit.affected_tracks.clone(),
                );
                self.announce_versions(session);
                Ok(EditOutcome::Graph(commit))
            }
        }
    }

    /// Undo restores the journaled bytes; redo re-executes the forward op. Rasterization is
    /// deterministic and a new edit clears the redo stack The journaled graph
    /// delta, when present, is reapplied exactly rather than re-materialized.
    fn step_mask(
        &self,
        image: &Arc<ImageReader>,
        session: &str,
        record: cellstudio_db::EditRecord,
        undo: bool,
    ) -> Result<MaskCommit, EditError> {
        if !record.undoable {
            return Err(EditError::Pruned(record.seq));
        }
        let op = parse_op(record.seq, &record.op)?;
        let Some(store) = self.open(image.dataset())? else {
            return Err(EditError::NoStore(if undo { "undo" } else { "redo" }));
        };
        let snaps = journal_chunks(&self.project.db.take_blobs(record.seq)?)?;
        let inverse: MaskInverse =
            serde_json::from_value(record.inverse.clone()).map_err(|source| {
                EditError::Journal {
                    seq: record.seq,
                    source,
                }
            })?;

        let t = op.t();
        if !undo {
            let forward = replay(&op);
            let commit = self.finish(
                image,
                session,
                &store,
                record.seq,
                &op,
                &snaps,
                false,
                GraphStep::Redo(inverse.graph.as_ref()),
                |store| match &forward {
                    Replay::Stroke(spec) => labels::apply(store, t, spec),
                    Replay::Delete { label, bbox } => labels::clear_label(store, t, *label, *bbox),
                },
            )?;
            self.project.db.mark_undone(record.seq, false)?;
            return Ok(commit);
        }

        let (commit, chunks) = {
            let _reads = self.reads.write();
            labels::restore(&store, &snaps)?;
            let coarse = labels::regenerate_coarse(&store, t, op.bbox())?;
            let commit = self.project.db.commit_edit(
                record.seq,
                t,
                &inverse.deltas,
                &inverse.cells,
                GraphStep::Undo(inverse.graph.as_ref()),
            )?;
            self.project.db.mark_undone(record.seq, true)?;
            let touched = [restored_keys(&snaps), coarse].concat();
            let chunks = self.invalidate(image, &touched);
            (commit, chunks)
        };
        let commit = MaskCommit {
            seq: record.seq,
            version: commit.version,
            has_labels: true,
            cells: updated(&commit.cells),
            removed: removed(&commit.cells),
            chunks,
            graph_version: commit.graph_version,
            affected_tracks: commit.graph.as_ref().map(|d| d.affected_tracks()),
        };
        self.announce(session, LayerId::Labels, commit.chunks.clone(), &commit);
        Ok(commit)
    }

    /// Adopts the store the project already has, or creates one, registering it on the live
    /// reader so the very first stroke is readable without re-opening the project.
    fn ensure(&self, image: &Arc<ImageReader>) -> Result<Arc<LabelStore>, EditError> {
        if let Some(store) = self.store.lock().clone() {
            return Ok(store);
        }
        let creating = !self.project.has_labels();
        let store = Arc::new(labels::ensure_store(
            &self.project.labels_store_path(),
            image.dataset(),
        )?);
        // a store the app created is empty: its inventory is trivially complete
        if creating && let Some(identity) = cellstudio_db::inventory::store_identity(store.root()) {
            self.project.db.set_inventory_marker(&identity)?;
        }
        *self.store.lock() = Some(store.clone());
        if image.bricks().layer(LayerId::Labels).is_none() {
            image.register_layer(LayerId::Labels, Arc::new(store.open_readable()?));
        }
        Ok(store)
    }

    /// The store as it is, without creating one.
    fn open(&self, image: &Dataset) -> Result<Option<Arc<LabelStore>>, EditError> {
        if let Some(store) = self.store.lock().clone() {
            return Ok(Some(store));
        }
        if !self.project.has_labels() {
            return Ok(None);
        }
        let store = Arc::new(labels::open_store(
            &self.project.labels_store_path(),
            image,
        )?);
        *self.store.lock() = Some(store.clone());
        Ok(Some(store))
    }

    /// A stroke may name an id that already exists on that frame, or one from a block this
    /// session reserved. Returns whether the id came from a lease, which is the case that
    /// needs no seeding scan: a leased id is absent from the store.
    fn authorize(&self, session: &str, t: u64, label: u32) -> Result<bool, EditError> {
        if label == 0 {
            return Err(EditError::Invalid("0 is background, not a cell".into()));
        }
        let leased = self.leases.lock().get(session).is_some_and(|blocks| {
            blocks.iter().any(|(first, count)| {
                label >= *first && u64::from(label) < u64::from(*first) + u64::from(*count)
            })
        });
        if leased {
            return Ok(true);
        }
        let present = self
            .project
            .db
            .cells_window(t, t, None)?
            .iter()
            .any(|cell| cell.id == label);
        match present {
            true => Ok(false),
            false => Err(EditError::Unreserved(label)),
        }
    }

    /// One bounded frame scan per `(t, label)` the session did not paint, so an adopted
    /// cell's area and centroid stay exact rather than counting this session's voxels only
    /// A freshly reserved id has nothing to scan.
    fn seed(
        &self,
        store: &LabelStore,
        t: u64,
        label: u32,
        known_empty: bool,
    ) -> Result<(), EditError> {
        let scan = || -> Result<ExtentRow, EditError> {
            match known_empty {
                true => Ok(ExtentRow::default()),
                false => Ok(to_db_extent(&labels::scan_label(store, t, label)?)),
            }
        };
        self.project.db.ensure_extent(t, label, scan)?;
        Ok(())
    }

    fn seed_from_delta(
        &self,
        store: &LabelStore,
        t: u64,
        delta: &LabelDelta,
    ) -> Result<(), EditError> {
        let scan = || -> Result<ExtentRow, EditError> {
            let after = labels::scan_label(store, t, delta.label)?;
            Ok(before_edit(&after, delta))
        };
        self.project.db.ensure_extent(t, delta.label, scan)?;
        Ok(())
    }

    fn journal(&self, op: &MaskOp, snaps: &[labels::ChunkSnapshot]) -> Result<i64, EditError> {
        let blobs: Vec<ChunkSnapshot> = snaps.iter().map(to_db_snapshot).collect();
        let op = serde_json::to_value(op).map_err(DbError::Json)?;
        // `inverse` stays null until `commit_edit` writes it: the delta is only knowable
        // once the chunks are written, and the row has to exist before they are
        Ok(self
            .project
            .db
            .record_edit_pending(EditDomain::Mask, &op, &Value::Null, &blobs)?)
    }

    fn roll_back(
        &self,
        store: &LabelStore,
        seq: i64,
        t: u64,
        bbox: VoxelBox,
        snaps: &[labels::ChunkSnapshot],
        fresh: bool,
    ) {
        tracing::error!(
            seq,
            "mask edit failed after its journal row; rolling it back"
        );
        let restored = labels::restore(store, snaps)
            .and_then(|()| labels::regenerate_coarse(store, t, bbox).map(|_| ()));
        if let Err(e) = restored {
            // the row stays `pending`, so the next project open rolls it back
            return tracing::error!(seq, "rollback failed, leaving the journal row pending: {e}");
        }
        // a failed redo leaves its history row undone rather than dropping it
        if fresh && let Err(e) = self.project.db.delete_edit(seq) {
            tracing::error!(seq, "rolled back but could not drop the journal row: {e}");
        }
    }

    /// Drops the resident bricks and bumps their epochs, so an in-flight decode that read
    /// pre-edit bytes cannot publish them after the write.
    fn invalidate(&self, image: &Arc<ImageReader>, keys: &[ChunkKey]) -> Vec<String> {
        let bricks: Vec<BrickKey> = keys.iter().map(|key| key.brick(LayerId::Labels)).collect();
        image.bricks().invalidate(&bricks);
        keys.iter().map(encode_chunk).collect()
    }

    fn announce(&self, session: &str, layer: LayerId, chunks: Vec<String>, commit: &MaskCommit) {
        self.events.publish(Event::Invalidate {
            session_id: session.to_owned(),
            layer,
            chunks,
            version: commit.version,
        });
        // a topology-changing mask commit announces the graph too.
        if let Some(graph_version) = commit.graph_version {
            self.announce_graph(
                session,
                graph_version,
                commit.affected_tracks.clone().unwrap_or_default(),
            );
        }
        self.announce_versions(session);
    }

    fn announce_graph(&self, session: &str, graph_version: u64, tracks: Vec<u32>) {
        self.events.publish(Event::GraphChanged {
            session_id: session.to_owned(),
            graph_version,
            tracks,
        });
    }

    fn announce_versions(&self, session: &str) {
        match self.project.db.versions() {
            Ok(versions) => self.events.publish(Event::Versions {
                versions: VersionsWire::new(session, versions),
            }),
            Err(e) => tracing::error!("cannot read versions after an edit: {e}"),
        }
    }

    /// A stamp entirely outside the volume, or a delete of a label with no voxels: nothing is
    /// journaled, nothing is invalidated, and the version does not move.
    fn unchanged(&self) -> Result<MaskCommit, EditError> {
        Ok(MaskCommit {
            seq: 0,
            version: self.project.db.versions()?.labels,
            has_labels: self.project.has_labels(),
            ..MaskCommit::default()
        })
    }
}

fn plane_index(axis: Axis, centre: [f64; 3]) -> u64 {
    let value = match axis {
        Axis::Z => centre[0],
        Axis::Y => centre[1],
        _ => centre[2],
    };
    value.max(0.0).floor() as u64
}

/// Every level-0 chunk a voxel set falls in, whether or not the write ends up changing it:
/// snapshotting a superset is safe, and restoring an untouched chunk is its own value.
fn affected_chunks(
    store: &LabelStore,
    t: u64,
    set: &VoxelSet,
) -> Result<Vec<ChunkKey>, LabelError> {
    let chunks = store.chunks(0)?;
    let mut keys: Vec<ChunkKey> = set
        .runs()
        .iter()
        .flat_map(|run| {
            let grid = [run.z / chunks.z, run.y / chunks.y];
            (run.x0 / chunks.x..=run.x1 / chunks.x).map(move |gx| ChunkKey {
                level: 0,
                t,
                grid: [grid[0], grid[1], gx],
            })
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

fn clip(bbox: VoxelBox, dims: Dims) -> VoxelBox {
    VoxelBox {
        z0: bbox.z0.min(dims.z.saturating_sub(1)),
        z1: bbox.z1.min(dims.z.saturating_sub(1)),
        y0: bbox.y0.min(dims.y.saturating_sub(1)),
        y1: bbox.y1.min(dims.y.saturating_sub(1)),
        x0: bbox.x0.min(dims.x.saturating_sub(1)),
        x1: bbox.x1.min(dims.x.saturating_sub(1)),
    }
}

/// The forward write of a journaled op, re-derived for redo.
enum Replay {
    Stroke(StrokeSpec),
    Delete { label: u32, bbox: VoxelBox },
}

fn replay(op: &MaskOp) -> Replay {
    match op {
        MaskOp::Stroke {
            label,
            erase,
            radius,
            plane,
            stamps,
            only,
            ..
        } => Replay::Stroke(StrokeSpec {
            mode: match erase {
                true => StrokeMode::Erase { only: *only },
                false => StrokeMode::Paint { label: *label },
            },
            radius: *radius,
            plane: *plane,
            centres: stamps.clone(),
        }),
        MaskOp::Delete { label, bbox, .. } => Replay::Delete {
            label: *label,
            bbox: *bbox,
        },
    }
}

fn parse_op(seq: i64, op: &Value) -> Result<MaskOp, EditError> {
    serde_json::from_value(op.clone()).map_err(|source| EditError::Journal { seq, source })
}

fn journal_chunks(blobs: &[ChunkSnapshot]) -> Result<Vec<labels::ChunkSnapshot>, EditError> {
    let mut chunks = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let key = decode_chunk(&blob.chunk_key).ok_or_else(|| {
            EditError::Invalid(format!(
                "journal chunk key `{}` is not a key",
                blob.chunk_key
            ))
        })?;
        chunks.push(labels::ChunkSnapshot {
            key,
            existed: blob.existed,
            bytes: blob.before.clone(),
        });
    }
    Ok(chunks)
}

fn restored_keys(snaps: &[labels::ChunkSnapshot]) -> Vec<ChunkKey> {
    snaps.iter().map(|snap| snap.key).collect()
}

fn to_db_snapshot(snap: &labels::ChunkSnapshot) -> ChunkSnapshot {
    ChunkSnapshot {
        chunk_key: encode_chunk(&snap.key),
        existed: snap.existed,
        before: snap.bytes.clone(),
    }
}

/// The zarr store key of the chunk, which is also what the invalidation event names.
fn encode_chunk(key: &ChunkKey) -> String {
    format!(
        "{}/c/{}/0/{}/{}/{}",
        key.level, key.t, key.grid[0], key.grid[1], key.grid[2]
    )
}

fn decode_chunk(raw: &str) -> Option<ChunkKey> {
    let mut parts = raw.split('/');
    let level = parts.next()?.parse().ok()?;
    if parts.next()? != "c" {
        return None;
    }
    let t = parts.next()?.parse().ok()?;
    let _c = parts.next()?;
    let grid = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    match parts.next() {
        None => Some(ChunkKey { level, t, grid }),
        Some(_) => None,
    }
}

fn extent_deltas(footprint: &EditFootprint) -> Vec<ExtentDelta> {
    let added = footprint.bbox.map(to_db_box);
    footprint
        .deltas
        .iter()
        .map(|delta| ExtentDelta {
            label: delta.label,
            area: delta.area,
            sum_z: delta.sum_z,
            sum_y: delta.sum_y,
            sum_x: delta.sum_x,
            // the bbox is an upper bound the delete scan reads; only a gain can widen it
            bbox: (delta.area > 0).then_some(added).flatten(),
        })
        .collect()
}

fn updated(changes: &[CellChange]) -> Vec<CellRow> {
    changes
        .iter()
        .filter_map(|change| match change {
            CellChange::Updated(row) => Some(row.clone()),
            CellChange::Removed(_) => None,
        })
        .collect()
}

fn removed(changes: &[CellChange]) -> Vec<u32> {
    changes
        .iter()
        .filter_map(|change| match change {
            CellChange::Removed(snapshot) => Some(snapshot.cell.id),
            CellChange::Updated(_) => None,
        })
        .collect()
}

fn to_db_box(bbox: VoxelBox) -> cellstudio_db::VoxelBox {
    let edge = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    cellstudio_db::VoxelBox {
        z: [edge(bbox.z0), edge(bbox.z1)],
        y: [edge(bbox.y0), edge(bbox.y1)],
        x: [edge(bbox.x0), edge(bbox.x1)],
    }
}

fn from_db_box(bbox: cellstudio_db::VoxelBox) -> VoxelBox {
    VoxelBox {
        z0: u64::from(bbox.z[0]),
        z1: u64::from(bbox.z[1]),
        y0: u64::from(bbox.y[0]),
        y1: u64::from(bbox.y[1]),
        x0: u64::from(bbox.x[0]),
        x1: u64::from(bbox.x[1]),
    }
}

fn to_db_extent(row: &labels::ExtentRow) -> ExtentRow {
    ExtentRow {
        bbox: row.bbox.map(to_db_box),
        area: row.area,
        sum_z: row.sum_z,
        sum_y: row.sum_y,
        sum_x: row.sum_x,
    }
}

/// The store as it was before this edit, from a scan of it as it is now: exact, and it costs
/// one frame scan rather than a second one before the write.
fn before_edit(after: &labels::ExtentRow, delta: &LabelDelta) -> ExtentRow {
    let area = (after.area as i64 - delta.area).max(0) as u64;
    ExtentRow {
        bbox: after.bbox.map(to_db_box),
        area,
        sum_z: match area {
            0 => 0.0,
            _ => after.sum_z - delta.sum_z,
        },
        sum_y: match area {
            0 => 0.0,
            _ => after.sum_y - delta.sum_y,
        },
        sum_x: match area {
            0 => 0.0,
            _ => after.sum_x - delta.sum_x,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_key_round_trips_through_its_store_key() {
        let key = ChunkKey {
            level: 2,
            t: 7,
            grid: [1, 30, 4],
        };
        assert_eq!(encode_chunk(&key), "2/c/7/0/1/30/4");
        assert_eq!(decode_chunk("2/c/7/0/1/30/4"), Some(key));
        for bad in [
            "",
            "2/7/0/1/30/4",
            "2/c/7/0/1/30",
            "2/c/7/0/1/30/4/5",
            "x/c/0/0/0/0/0",
        ] {
            assert_eq!(decode_chunk(bad), None, "{bad} is not a chunk key");
        }
    }

    #[test]
    fn the_pre_edit_row_is_the_scan_less_this_edits_delta() {
        let after = labels::ExtentRow {
            t: 0,
            label: 5,
            bbox: Some(VoxelBox {
                z0: 0,
                z1: 1,
                y0: 0,
                y1: 1,
                x0: 0,
                x1: 1,
            }),
            area: 10,
            sum_z: 20.0,
            sum_y: 30.0,
            sum_x: 40.0,
        };
        let painted = LabelDelta {
            label: 5,
            area: 4,
            sum_z: 8.0,
            sum_y: 12.0,
            sum_x: 16.0,
        };
        let before = before_edit(&after, &painted);
        assert_eq!(
            (before.area, before.sum_z, before.sum_y, before.sum_x),
            (6, 12.0, 18.0, 24.0)
        );

        let erased = LabelDelta {
            label: 5,
            area: -4,
            sum_z: -8.0,
            sum_y: -12.0,
            sum_x: -16.0,
        };
        let before = before_edit(&after, &erased);
        assert_eq!((before.area, before.sum_z), (14, 28.0));
    }
}
