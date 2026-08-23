use std::collections::{HashSet, VecDeque};

use cellstudio_core::tracks::CellState;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::project::{Db, DbError};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bbox {
    pub z: [f64; 2],
    pub y: [f64; 2],
    pub x: [f64; 2],
}

impl Bbox {
    pub const UNBOUNDED: Self = Self {
        z: [f64::NEG_INFINITY, f64::INFINITY],
        y: [f64::NEG_INFINITY, f64::INFINITY],
        x: [f64::NEG_INFINITY, f64::INFINITY],
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellRow {
    pub id: u32,
    pub t: u64,
    pub centroid: Option<[f64; 3]>,
    pub area: Option<u64>,
    pub detection_confidence: Option<f64>,
    pub state: Option<CellState>,
    pub track_id: Option<u32>,
    pub src_id: Option<u32>,
    pub seg_id: Option<u32>,
    pub labels: Vec<String>,
    pub features: Map<String, Value>,
    pub reviewed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkRow {
    pub parent: u32,
    pub child: u32,
    pub confidence: Option<f64>,
    pub reviewed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageTree {
    pub focus: u32,
    pub root: u32,
    /// Time-ordered, `(t, id)`.
    pub cells: Vec<CellRow>,
    pub links: Vec<LinkRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Versions {
    pub image: u64,
    pub labels: u64,
    pub graph: u64,
    pub settings: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionCounter {
    Image,
    Labels,
    Graph,
    Settings,
}

impl VersionCounter {
    pub const ALL: [Self; 4] = [Self::Image, Self::Labels, Self::Graph, Self::Settings];

    pub fn key(&self) -> &'static str {
        match self {
            Self::Image => "version.image",
            Self::Labels => "version.labels",
            Self::Graph => "version.graph",
            Self::Settings => "version.settings",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditDomain {
    Graph,
    Mask,
}

impl EditDomain {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Mask => "mask",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "graph" => Some(Self::Graph),
            "mask" => Some(Self::Mask),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditEntry {
    pub seq: i64,
    pub ts: String,
    pub domain: EditDomain,
    pub scope: Option<String>,
    pub undone: bool,
    /// False once the row's chunk snapshots have been pruned: the history shows it, undo
    /// declines it (design M7).
    pub undoable: bool,
}

/// Inclusive voxel bounds at level 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoxelBox {
    pub z: [u32; 2],
    pub y: [u32; 2],
    pub x: [u32; 2],
}

impl VoxelBox {
    pub fn union(self, other: Self) -> Self {
        let axis = |a: [u32; 2], b: [u32; 2]| [a[0].min(b[0]), a[1].max(b[1])];
        Self {
            z: axis(self.z, other.z),
            y: axis(self.y, other.y),
            x: axis(self.x, other.x),
        }
    }
}

/// A `mask_extent` row: the bbox is an upper bound that paint grows and erase never shrinks,
/// the area and sums are exact.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ExtentRow {
    pub bbox: Option<VoxelBox>,
    pub area: u64,
    pub sum_z: f64,
    pub sum_y: f64,
    pub sum_x: f64,
}

/// What one label gained or lost in one edit, from the rasterizer's exact voxel delta.
/// `bbox` covers the voxels *added*; an erase-only delta carries `None`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExtentDelta {
    pub label: u32,
    pub area: i64,
    pub sum_z: f64,
    pub sum_y: f64,
    pub sum_x: f64,
    pub bbox: Option<VoxelBox>,
}

/// A `cells` row with the links that reference it, as removed by an erase and as restored by
/// its undo. Serializes into the journal's inverse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellSnapshot {
    pub cell: CellRow,
    pub links: Vec<LinkRow>,
}

/// What a delta did to a cell, for the mask response and for the journal's inverse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CellChange {
    /// Row created or rewritten from the folded extent.
    Updated(CellRow),
    /// Last voxel on the frame erased: the row, its links, and its `mask_labels` entry are gone.
    Removed(CellSnapshot),
}

/// One journaled chunk object. `before` is `None` exactly when the key held no object, whose
/// inverse is an erase rather than a write of encoded zeros.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkSnapshot {
    pub chunk_key: String,
    pub existed: bool,
    pub before: Option<Vec<u8>>,
}

/// A journal row with its payload, for undo, redo, and recovery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditRecord {
    pub seq: i64,
    pub domain: EditDomain,
    pub op: Value,
    pub inverse: Value,
    /// False once the row's chunk snapshots have been pruned.
    pub undoable: bool,
}

/// What `commit_edit` records in `edits.inverse` so the edit can be undone: the forward
/// deltas negated, and the cells it removed with their links.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaskInverse {
    pub deltas: Vec<ExtentDelta>,
    pub cells: Vec<CellSnapshot>,
}

fn negate_delta(delta: &ExtentDelta) -> ExtentDelta {
    ExtentDelta {
        label: delta.label,
        area: -delta.area,
        sum_z: -delta.sum_z,
        sum_y: -delta.sum_y,
        sum_x: -delta.sum_x,
        // undo never widens the bound the forward edit already widened
        bbox: None,
    }
}

/// The result of the commit transaction of design M6 step 5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditCommit {
    pub version: u64,
    pub cells: Vec<CellChange>,
}

const CELLS_WINDOW_SQL: &str = "\
SELECT id, t, z, y, x, area, detection_confidence, state, track_id, src_id, seg_id,
       labels, features, reviewed
  FROM cells
 WHERE t >= ?1 AND t <= ?2
   AND (?3 = 0 OR (z BETWEEN ?4 AND ?5 AND y BETWEEN ?6 AND ?7 AND x BETWEEN ?8 AND ?9))
 ORDER BY t, id";

const CELL_BY_ID_SQL: &str = "\
SELECT id, t, z, y, x, area, detection_confidence, state, track_id, src_id, seg_id,
       labels, features, reviewed
  FROM cells
 WHERE id = ?1";

impl Db {
    pub fn cells_window(
        &self,
        t0: u64,
        t1: u64,
        bbox: Option<Bbox>,
    ) -> Result<Vec<CellRow>, DbError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(CELLS_WINDOW_SQL)?;
        let b = bbox.unwrap_or(Bbox::UNBOUNDED);
        let mut rows = stmt.query(params![
            to_i64(t0),
            to_i64(t1),
            i64::from(bbox.is_some()),
            b.z[0],
            b.z[1],
            b.y[0],
            b.y[1],
            b.x[0],
            b.x[1],
        ])?;
        let mut cells = Vec::new();
        while let Some(row) = rows.next()? {
            cells.push(cell_row(row)?);
        }
        Ok(cells)
    }

    pub fn lineage(&self, cell_id: u32) -> Result<LineageTree, DbError> {
        let conn = self.conn()?;
        let mut by_id = conn.prepare_cached(CELL_BY_ID_SQL)?;
        if !by_id.exists([cell_id])? {
            return Err(DbError::UnknownCell(cell_id));
        }

        let mut parent_of = conn.prepare_cached("SELECT parent FROM links WHERE child = ?1")?;
        let mut root = cell_id;
        let mut climbed = HashSet::from([root]);
        while let Some(parent) = parent_of
            .query_row([root], |row| row.get::<_, i64>(0))
            .optional()?
        {
            let parent = to_u32(parent)?;
            if !climbed.insert(parent) {
                break;
            }
            root = parent;
        }

        let mut children_of =
            conn.prepare_cached("SELECT child, confidence, reviewed FROM links WHERE parent = ?1")?;
        let mut cells = Vec::new();
        let mut links = Vec::new();
        let mut visited = HashSet::from([root]);
        let mut queue = VecDeque::from([root]);
        while let Some(id) = queue.pop_front() {
            let cell = by_id.query_row([id], |row| Ok(cell_row(row)))?;
            cells.push(cell?);

            let mut rows = children_of.query([id])?;
            while let Some(row) = rows.next()? {
                let child = to_u32(row.get(0)?)?;
                links.push(LinkRow {
                    parent: id,
                    child,
                    confidence: row.get(1)?,
                    reviewed: row.get::<_, i64>(2)? != 0,
                });
                if visited.insert(child) {
                    queue.push_back(child);
                }
            }
        }
        cells.sort_by_key(|c| (c.t, c.id));
        links.sort_by_key(|l| (l.parent, l.child));
        Ok(LineageTree {
            focus: cell_id,
            root,
            cells,
            links,
        })
    }

    pub fn review_queue(&self, limit: u32) -> Result<Vec<LinkRow>, DbError> {
        const SQL: &str = "\
SELECT parent, child, confidence, reviewed
  FROM links
 ORDER BY reviewed ASC, confidence IS NULL ASC, confidence ASC, parent ASC, child ASC
 LIMIT ?1";
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(SQL)?;
        let mut rows = stmt.query([limit])?;
        let mut links = Vec::new();
        while let Some(row) = rows.next()? {
            links.push(LinkRow {
                parent: to_u32(row.get(0)?)?,
                child: to_u32(row.get(1)?)?,
                confidence: row.get(2)?,
                reviewed: row.get::<_, i64>(3)? != 0,
            });
        }
        Ok(links)
    }

    pub fn edits(&self, limit: u32) -> Result<Vec<EditEntry>, DbError> {
        const SQL: &str = "\
SELECT seq, ts, domain, op, undone,
       EXISTS(SELECT 1 FROM edit_blobs b WHERE b.seq = edits.seq) OR domain <> 'mask'
  FROM edits ORDER BY seq DESC LIMIT ?1";
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(SQL)?;
        let mut rows = stmt.query([limit])?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            let domain: String = row.get(2)?;
            let op: String = row.get(3)?;
            entries.push(EditEntry {
                seq: row.get(0)?,
                ts: row.get(1)?,
                domain: EditDomain::parse(&domain).ok_or(DbError::UnknownDomain(domain))?,
                scope: scope_of(&op),
                undone: row.get::<_, i64>(4)? != 0,
                undoable: row.get::<_, i64>(5)? != 0,
            });
        }
        Ok(entries)
    }

    pub fn record_edit(
        &self,
        domain: EditDomain,
        op: &Value,
        inverse: &Value,
    ) -> Result<i64, DbError> {
        const SQL: &str = "\
INSERT INTO edits(ts, domain, op, inverse)
VALUES (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?1, ?2, ?3)
RETURNING seq";
        let op = serde_json::to_string(op)?;
        let inverse = serde_json::to_string(inverse)?;
        let conn = self.conn()?;
        let seq = conn.query_row(SQL, params![domain.as_str(), op, inverse], |row| row.get(0))?;
        Ok(seq)
    }

    pub fn mark_undone(&self, seq: i64, undone: bool) -> Result<(), DbError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE edits SET undone = ?2 WHERE seq = ?1",
            params![seq, i64::from(undone)],
        )?;
        Ok(())
    }

    pub fn versions(&self) -> Result<Versions, DbError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT key, CAST(value AS INTEGER) FROM meta WHERE key LIKE 'version.%'",
        )?;
        let mut rows = stmt.query([])?;
        let mut versions = Versions::default();
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let value = row.get::<_, i64>(1)?.max(0) as u64;
            match VersionCounter::from_key(&key) {
                Some(VersionCounter::Image) => versions.image = value,
                Some(VersionCounter::Labels) => versions.labels = value,
                Some(VersionCounter::Graph) => versions.graph = value,
                Some(VersionCounter::Settings) => versions.settings = value,
                None => {}
            }
        }
        Ok(versions)
    }

    pub fn bump(&self, counter: VersionCounter) -> Result<u64, DbError> {
        let conn = self.conn()?;
        bump_in(&conn, counter)
    }
}

/// 0 is background; it never gets a `cells` row.
const BACKGROUND: u32 = 0;

/// viv hands the fragment shader a float, so ids stay distinguishable only to 2²⁴ (design M14).
pub const MAX_LABEL_ID: u32 = (1 << 24) - 1;

const NEXT_ID_KEY: &str = "labels.next_id";

const EXTENT_SQL: &str = "\
SELECT z0, z1, y0, y1, x0, x1, area, sum_z, sum_y, sum_x
  FROM mask_extent WHERE t = ?1 AND label = ?2";

const PUT_EXTENT_SQL: &str = "\
INSERT INTO mask_extent(t, label, z0, z1, y0, y1, x0, x1, area, sum_z, sum_y, sum_x)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(t, label) DO UPDATE SET
  z0 = excluded.z0, z1 = excluded.z1, y0 = excluded.y0, y1 = excluded.y1,
  x0 = excluded.x0, x1 = excluded.x1,
  area = excluded.area, sum_z = excluded.sum_z, sum_y = excluded.sum_y, sum_x = excluded.sum_x";

const SEED_EXTENT_SQL: &str = "\
INSERT INTO mask_extent(t, label, z0, z1, y0, y1, x0, x1, area, sum_z, sum_y, sum_x)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(t, label) DO NOTHING";

const RESTORE_CELL_SQL: &str = "\
INSERT INTO cells(id, t, z, y, x, area, detection_confidence, state, track_id, src_id, seg_id,
                  labels, features, reviewed)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
ON CONFLICT(id) DO UPDATE SET
  t = excluded.t, z = excluded.z, y = excluded.y, x = excluded.x, area = excluded.area,
  detection_confidence = excluded.detection_confidence, state = excluded.state,
  track_id = excluded.track_id, src_id = excluded.src_id, seg_id = excluded.seg_id,
  labels = excluded.labels, features = excluded.features, reviewed = excluded.reviewed";

impl Db {
    /// Journals a mask edit with `pending = 1` and its chunk snapshots in one transaction, so
    /// no chunk is ever written without a record of what it was (design M6 step 2). Clears the
    /// redo stack in the same transaction — a new edit invalidates every re-executable row (M7).
    pub fn record_edit_pending(
        &self,
        domain: EditDomain,
        op: &Value,
        inverse: &Value,
        blobs: &[ChunkSnapshot],
    ) -> Result<i64, DbError> {
        const SQL: &str = "\
INSERT INTO edits(ts, domain, op, inverse, pending)
VALUES (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?1, ?2, ?3, 1)
RETURNING seq";
        let op = serde_json::to_string(op)?;
        let inverse = serde_json::to_string(inverse)?;
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        clear_redo_in(&tx)?;
        let seq: i64 =
            tx.query_row(SQL, params![domain.as_str(), op, inverse], |row| row.get(0))?;
        for blob in blobs {
            put_blob_in(&tx, seq, blob)?;
        }
        tx.commit()?;
        Ok(seq)
    }

    pub fn clear_pending(&self, seq: i64) -> Result<(), DbError> {
        let conn = self.conn()?;
        conn.execute("UPDATE edits SET pending = 0 WHERE seq = ?1", [seq])?;
        Ok(())
    }

    /// Adds one chunk snapshot to an existing journal row. Prefer passing the whole set to
    /// [`Db::record_edit_pending`]; this is for a caller that already committed the row.
    pub fn put_blob(&self, seq: i64, blob: &ChunkSnapshot) -> Result<(), DbError> {
        let conn = self.conn()?;
        put_blob_in(&conn, seq, blob)
    }

    /// The chunk snapshots of `seq`, in insertion order. They survive the read: redo
    /// re-executes the forward op onto the same prior state, so the same blobs undo it again.
    /// [`Db::delete_edit`] and [`Db::prune_blobs`] are what remove them.
    pub fn take_blobs(&self, seq: i64) -> Result<Vec<ChunkSnapshot>, DbError> {
        const SQL: &str =
            "SELECT chunk_key, existed, before FROM edit_blobs WHERE seq = ?1 ORDER BY rowid";
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(SQL)?;
        let mut rows = stmt.query([seq])?;
        let mut blobs = Vec::new();
        while let Some(row) = rows.next()? {
            blobs.push(ChunkSnapshot {
                chunk_key: row.get(0)?,
                existed: row.get::<_, i64>(1)? != 0,
                before: row.get(2)?,
            });
        }
        Ok(blobs)
    }

    /// Drops the snapshots of every mask edit older than the newest `keep`. Their history rows
    /// stay and report `undoable = false` (design M7).
    pub fn prune_blobs(&self, keep: u32) -> Result<u64, DbError> {
        const SQL: &str = "\
DELETE FROM edit_blobs WHERE seq NOT IN (
  SELECT seq FROM edits WHERE domain = 'mask' ORDER BY seq DESC LIMIT ?1)";
        let conn = self.conn()?;
        Ok(conn.execute(SQL, [keep])? as u64)
    }

    /// Rows left `pending = 1` by a crash, oldest first — the order recovery replays them in.
    pub fn pending_edits(&self) -> Result<Vec<EditRecord>, DbError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(&edit_record_sql("pending = 1", "seq ASC", None))?;
        let mut rows = stmt.query([])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(edit_record(row)?);
        }
        Ok(records)
    }

    /// The newest committed edit that has not been undone, whatever its domain (design M7).
    pub fn undo_next(&self) -> Result<Option<EditRecord>, DbError> {
        self.edit_at("pending = 0 AND undone = 0", "seq DESC")
    }

    /// The oldest undone edit — the one the last undo removed.
    pub fn redo_next(&self) -> Result<Option<EditRecord>, DbError> {
        self.edit_at("pending = 0 AND undone = 1", "seq ASC")
    }

    fn edit_at(&self, filter: &str, order: &str) -> Result<Option<EditRecord>, DbError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare_cached(&edit_record_sql(filter, order, Some(1)))?;
        stmt.query_row([], |row| Ok(edit_record(row)))
            .optional()?
            .transpose()
    }

    /// Discards every undone edit and its snapshots. Called inside
    /// [`Db::record_edit_pending`]; public for a domain that journals its own way.
    pub fn clear_redo(&self) -> Result<u64, DbError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        let dropped = clear_redo_in(&tx)?;
        tx.commit()?;
        Ok(dropped)
    }

    /// Removes a journal row and its snapshots — what recovery does once a `pending` row has
    /// been rolled back.
    pub fn delete_edit(&self, seq: i64) -> Result<(), DbError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        tx.execute("DELETE FROM edit_blobs WHERE seq = ?1", [seq])?;
        tx.execute("DELETE FROM edits WHERE seq = ?1", [seq])?;
        tx.commit()?;
        Ok(())
    }

    /// The recorded extent of `(t, label)`, whose bbox bounds the delete scan (design M18).
    pub fn extent_of(&self, t: u64, label: u32) -> Result<Option<ExtentRow>, DbError> {
        let conn = self.conn()?;
        extent_in(&conn, t, label)
    }

    /// Seeds `mask_extent` for `(t, label)` from `scan` when the row is missing, so a label
    /// this session did not paint gets its exact area and centroid once instead of counting
    /// only this session's voxels (design M17). Returns whether `scan` ran.
    ///
    /// `scan` is the caller's `labels::scan_label`: the database never reads the label store.
    /// The connection is not held while it runs.
    pub fn ensure_extent<E, F>(&self, t: u64, label: u32, scan: F) -> Result<bool, E>
    where
        E: From<DbError>,
        F: FnOnce() -> Result<ExtentRow, E>,
    {
        {
            let conn = self.conn()?;
            check_frame(&conn, t, label)?;
            if extent_in(&conn, t, label)?.is_some() {
                return Ok(false);
            }
        }
        let row = scan()?;
        let conn = self.conn()?;
        write_extent_in(&conn, SEED_EXTENT_SQL, t, label, &row)?;
        Ok(true)
    }

    /// Folds the rasterizer's voxel delta into `mask_extent` and rewrites the affected
    /// `cells` rows from the resulting sums (design M17).
    pub fn apply_extent_delta(
        &self,
        t: u64,
        deltas: &[ExtentDelta],
    ) -> Result<Vec<CellChange>, DbError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        let changes = apply_deltas_in(&tx, t, deltas)?;
        tx.commit()?;
        Ok(changes)
    }

    /// Design M6 step 5, as one transaction: the restored rows of an undo, the stats and cell
    /// changes, `pending = 0`, and the `version.labels` bump. Bumping outside it would
    /// advertise an edit whose derived rows are not committed.
    pub fn commit_edit(
        &self,
        seq: i64,
        t: u64,
        deltas: &[ExtentDelta],
        restore: &[CellSnapshot],
    ) -> Result<EditCommit, DbError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        for snapshot in restore {
            restore_cell_in(&tx, snapshot)?;
        }
        let cells = apply_deltas_in(&tx, t, deltas)?;
        // written here, in the commit transaction: a durable edit is always undoable
        let inverse = MaskInverse {
            deltas: deltas.iter().map(negate_delta).collect(),
            cells: cells
                .iter()
                .filter_map(|change| match change {
                    CellChange::Removed(snapshot) => Some(snapshot.clone()),
                    CellChange::Updated(_) => None,
                })
                .collect(),
        };
        tx.execute(
            "UPDATE edits SET inverse = ?2, pending = 0 WHERE seq = ?1",
            rusqlite::params![seq, serde_json::to_string(&inverse).map_err(DbError::Json)?],
        )?;
        let version = bump_in(&tx, VersionCounter::Labels)?;
        tx.commit()?;
        Ok(EditCommit { version, cells })
    }

    /// Reserves `count` consecutive label ids and returns the first. The counter starts past
    /// every id already in `cells` or `mask_labels`, so a store this session did not write
    /// never has one of its ids handed out again (design M10).
    pub fn reserve_label_ids(&self, count: u32) -> Result<u32, DbError> {
        const IN_USE_SQL: &str = "\
SELECT id FROM cells WHERE id BETWEEN ?1 AND ?2
UNION ALL
SELECT label FROM mask_labels WHERE label BETWEEN ?1 AND ?2
LIMIT 1";
        const SEED_SQL: &str = "\
SELECT MAX(m) FROM (SELECT COALESCE(MAX(id), 0) AS m FROM cells
                    UNION ALL SELECT COALESCE(MAX(label), 0) FROM mask_labels)";

        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        let seed: i64 = tx.query_row(SEED_SQL, [], |row| row.get(0))?;
        let stored: Option<i64> = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
                [NEXT_ID_KEY],
                |row| row.get(0),
            )
            .optional()?;
        // 0 is background, and a counter left behind by an older store must not hand out a
        // live id
        let first = stored.unwrap_or(0).max(seed + 1).max(1);
        let last = first + i64::from(count.max(1)) - 1;
        if last > i64::from(MAX_LABEL_ID) {
            return Err(DbError::LabelIdsExhausted { first, last });
        }
        if let Some(taken) = tx
            .query_row(IN_USE_SQL, params![first, last], |row| row.get::<_, i64>(0))
            .optional()?
        {
            return Err(DbError::LabelIdTaken(to_u32(taken)?));
        }
        if count > 0 {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![NEXT_ID_KEY, (last + 1).to_string()],
            )?;
        }
        tx.commit()?;
        to_u32(first)
    }
}

fn edit_record_sql(filter: &str, order: &str, limit: Option<u32>) -> String {
    let limit = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
    format!(
        "SELECT seq, domain, op, inverse,
                EXISTS(SELECT 1 FROM edit_blobs b WHERE b.seq = edits.seq) OR domain <> 'mask'
           FROM edits WHERE {filter} ORDER BY {order}{limit}"
    )
}

fn edit_record(row: &Row<'_>) -> Result<EditRecord, DbError> {
    let domain: String = row.get(1)?;
    Ok(EditRecord {
        seq: row.get(0)?,
        domain: EditDomain::parse(&domain).ok_or(DbError::UnknownDomain(domain))?,
        op: serde_json::from_str(&row.get::<_, String>(2)?)?,
        inverse: serde_json::from_str(&row.get::<_, String>(3)?)?,
        undoable: row.get::<_, i64>(4)? != 0,
    })
}

fn put_blob_in(conn: &Connection, seq: i64, blob: &ChunkSnapshot) -> Result<(), DbError> {
    const SQL: &str =
        "INSERT INTO edit_blobs(seq, chunk_key, existed, before) VALUES (?1, ?2, ?3, ?4)";
    conn.execute(
        SQL,
        params![
            seq,
            blob.chunk_key,
            i64::from(blob.existed),
            // an absent object stores no bytes: its inverse is an erase, not a zero write
            if blob.existed {
                blob.before.as_deref()
            } else {
                None
            },
        ],
    )?;
    Ok(())
}

fn clear_redo_in(conn: &Connection) -> Result<u64, DbError> {
    conn.execute(
        "DELETE FROM edit_blobs WHERE seq IN (SELECT seq FROM edits WHERE undone = 1)",
        [],
    )?;
    Ok(conn.execute("DELETE FROM edits WHERE undone = 1", [])? as u64)
}

fn extent_in(conn: &Connection, t: u64, label: u32) -> Result<Option<ExtentRow>, DbError> {
    let mut stmt = conn.prepare_cached(EXTENT_SQL)?;
    let found = stmt
        .query_row(params![to_i64(t), label], |row| {
            Ok((
                [
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ],
                row.get::<_, i64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, f64>(8)?,
                row.get::<_, f64>(9)?,
            ))
        })
        .optional()?;
    let Some((raw, area, sum_z, sum_y, sum_x)) = found else {
        return Ok(None);
    };

    let mut edges = [0u32; 6];
    let mut bounded = true;
    for (edge, raw) in edges.iter_mut().zip(raw) {
        match raw {
            Some(value) => *edge = to_u32(value)?,
            // a label seeded with no voxels has no bbox yet
            None => bounded = false,
        }
    }
    Ok(Some(ExtentRow {
        bbox: bounded.then_some(VoxelBox {
            z: [edges[0], edges[1]],
            y: [edges[2], edges[3]],
            x: [edges[4], edges[5]],
        }),
        area: to_u64(area)?,
        sum_z,
        sum_y,
        sum_x,
    }))
}

/// `sql` is [`PUT_EXTENT_SQL`] to fold a delta in, or [`SEED_EXTENT_SQL`] to write a scan
/// only where no row has been folding deltas already.
fn write_extent_in(
    conn: &Connection,
    sql: &str,
    t: u64,
    label: u32,
    row: &ExtentRow,
) -> Result<(), DbError> {
    let b = row.bbox;
    conn.execute(
        sql,
        params![
            to_i64(t),
            label,
            b.map(|b| b.z[0]),
            b.map(|b| b.z[1]),
            b.map(|b| b.y[0]),
            b.map(|b| b.y[1]),
            b.map(|b| b.x[0]),
            b.map(|b| b.x[1]),
            to_i64(row.area),
            row.sum_z,
            row.sum_y,
            row.sum_x,
        ],
    )?;
    Ok(())
}

/// `cells.id` is the primary key and equals the voxel value, so a row that exists at another
/// `t` may not be reused: an upsert would move the cell and orphan its voxels (design M17).
fn check_frame(conn: &Connection, t: u64, label: u32) -> Result<(), DbError> {
    let existing: Option<i64> = conn
        .query_row("SELECT t FROM cells WHERE id = ?1", [label], |row| {
            row.get(0)
        })
        .optional()?;
    match existing {
        Some(existing) if to_u64(existing)? != t => Err(DbError::LabelFrameConflict {
            label,
            existing: to_u64(existing)?,
            requested: t,
        }),
        _ => Ok(()),
    }
}

fn apply_deltas_in(
    conn: &Connection,
    t: u64,
    deltas: &[ExtentDelta],
) -> Result<Vec<CellChange>, DbError> {
    let mut changes = Vec::new();
    for delta in deltas {
        // an erase writes 0, and background is not a cell
        if delta.label == BACKGROUND {
            continue;
        }
        check_frame(conn, t, delta.label)?;

        let before = extent_in(conn, t, delta.label)?.unwrap_or_default();
        let area = i64::try_from(before.area).unwrap_or(i64::MAX) + delta.area;
        let area = u64::try_from(area).map_err(|_| DbError::ExtentUnderflow {
            t,
            label: delta.label,
            area,
        })?;
        let row = ExtentRow {
            // an upper bound the delete scan reads: paint grows it, erase never shrinks it
            bbox: match (before.bbox, delta.bbox) {
                (Some(a), Some(b)) => Some(a.union(b)),
                (a, b) => a.or(b),
            },
            area,
            // exactly zero rather than a residue of cancelling sums
            sum_z: if area == 0 {
                0.0
            } else {
                before.sum_z + delta.sum_z
            },
            sum_y: if area == 0 {
                0.0
            } else {
                before.sum_y + delta.sum_y
            },
            sum_x: if area == 0 {
                0.0
            } else {
                before.sum_x + delta.sum_x
            },
        };
        write_extent_in(conn, PUT_EXTENT_SQL, t, delta.label, &row)?;

        match area {
            0 => changes.extend(remove_cell_in(conn, t, delta.label)?),
            _ => changes.push(CellChange::Updated(upsert_cell_in(
                conn,
                t,
                delta.label,
                &row,
            )?)),
        }
    }
    Ok(changes)
}

fn upsert_cell_in(
    conn: &Connection,
    t: u64,
    label: u32,
    extent: &ExtentRow,
) -> Result<CellRow, DbError> {
    const SQL: &str = "\
INSERT INTO cells(id, t, z, y, x, area) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(id) DO UPDATE SET
  t = excluded.t, z = excluded.z, y = excluded.y, x = excluded.x, area = excluded.area";
    let n = extent.area as f64;
    conn.execute(
        SQL,
        params![
            label,
            to_i64(t),
            extent.sum_z / n,
            extent.sum_y / n,
            extent.sum_x / n,
            to_i64(extent.area),
        ],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO mask_labels(t, label) VALUES (?1, ?2)",
        params![to_i64(t), label],
    )?;
    conn.query_row(CELL_BY_ID_SQL, [label], |row| Ok(cell_row(row)))?
}

fn remove_cell_in(conn: &Connection, t: u64, label: u32) -> Result<Option<CellChange>, DbError> {
    conn.execute(
        "DELETE FROM mask_labels WHERE t = ?1 AND label = ?2",
        params![to_i64(t), label],
    )?;
    let Some(cell) = conn
        .query_row(CELL_BY_ID_SQL, [label], |row| Ok(cell_row(row)))
        .optional()?
        .transpose()?
    else {
        return Ok(None);
    };
    let links = links_of(conn, label)?;
    conn.execute("DELETE FROM links WHERE parent = ?1 OR child = ?1", [label])?;
    conn.execute("DELETE FROM cells WHERE id = ?1", [label])?;
    Ok(Some(CellChange::Removed(CellSnapshot { cell, links })))
}

fn links_of(conn: &Connection, id: u32) -> Result<Vec<LinkRow>, DbError> {
    const SQL: &str = "\
SELECT parent, child, confidence, reviewed FROM links WHERE parent = ?1 OR child = ?1
 ORDER BY parent, child";
    let mut stmt = conn.prepare_cached(SQL)?;
    let mut rows = stmt.query([id])?;
    let mut links = Vec::new();
    while let Some(row) = rows.next()? {
        links.push(LinkRow {
            parent: to_u32(row.get(0)?)?,
            child: to_u32(row.get(1)?)?,
            confidence: row.get(2)?,
            reviewed: row.get::<_, i64>(3)? != 0,
        });
    }
    Ok(links)
}

fn restore_cell_in(conn: &Connection, snapshot: &CellSnapshot) -> Result<(), DbError> {
    let cell = &snapshot.cell;
    let centroid = cell.centroid;
    conn.execute(
        RESTORE_CELL_SQL,
        params![
            cell.id,
            to_i64(cell.t),
            centroid.map(|c| c[0]),
            centroid.map(|c| c[1]),
            centroid.map(|c| c[2]),
            cell.area.map(to_i64),
            cell.detection_confidence,
            cell.state.map(|s| s.as_str()),
            cell.track_id,
            cell.src_id,
            cell.seg_id,
            serde_json::to_string(&cell.labels)?,
            serde_json::to_string(&cell.features)?,
            i64::from(cell.reviewed),
        ],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO mask_labels(t, label) VALUES (?1, ?2)",
        params![to_i64(cell.t), cell.id],
    )?;
    for link in &snapshot.links {
        conn.execute(
            "INSERT OR IGNORE INTO links(parent, child, confidence, reviewed)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                link.parent,
                link.child,
                link.confidence,
                i64::from(link.reviewed)
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn bump_in(conn: &Connection, counter: VersionCounter) -> Result<u64, DbError> {
    const SQL: &str = "\
INSERT INTO meta(key, value) VALUES (?1, '1')
ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + 1
RETURNING CAST(value AS INTEGER)";
    let value: i64 = conn.query_row(SQL, [counter.key()], |row| row.get(0))?;
    Ok(value.max(0) as u64)
}

fn cell_row(row: &Row<'_>) -> Result<CellRow, DbError> {
    let z: Option<f64> = row.get(2)?;
    let y: Option<f64> = row.get(3)?;
    let x: Option<f64> = row.get(4)?;
    let state: Option<String> = row.get(7)?;
    Ok(CellRow {
        id: to_u32(row.get(0)?)?,
        t: to_u64(row.get(1)?)?,
        centroid: match (z, y, x) {
            (Some(z), Some(y), Some(x)) => Some([z, y, x]),
            _ => None,
        },
        area: row.get::<_, Option<i64>>(5)?.map(to_u64).transpose()?,
        detection_confidence: row.get(6)?,
        state: state
            .map(|s| CellState::parse(&s).ok_or(DbError::UnknownState(s)))
            .transpose()?,
        track_id: row.get::<_, Option<i64>>(8)?.map(to_u32).transpose()?,
        src_id: row.get::<_, Option<i64>>(9)?.map(to_u32).transpose()?,
        seg_id: row.get::<_, Option<i64>>(10)?.map(to_u32).transpose()?,
        labels: parse_labels(row.get::<_, Option<String>>(11)?)?,
        features: parse_features(row.get::<_, Option<String>>(12)?)?,
        reviewed: row.get::<_, i64>(13)? != 0,
    })
}

fn parse_labels(raw: Option<String>) -> Result<Vec<String>, DbError> {
    match raw.as_deref() {
        None | Some("") => Ok(Vec::new()),
        Some(text) => Ok(serde_json::from_str(text)?),
    }
}

fn parse_features(raw: Option<String>) -> Result<Map<String, Value>, DbError> {
    match raw.as_deref() {
        None | Some("") => Ok(Map::new()),
        Some(text) => Ok(serde_json::from_str(text)?),
    }
}

fn scope_of(op: &str) -> Option<String> {
    let value: Value = serde_json::from_str(op).ok()?;
    ["scope", "kind"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::to_owned)
}

fn to_u32(value: i64) -> Result<u32, DbError> {
    u32::try_from(value).map_err(|_| DbError::OutOfRange(value))
}

fn to_u64(value: i64) -> Result<u64, DbError> {
    u64::try_from(value).map_err(|_| DbError::OutOfRange(value))
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    const SEED: &str = r#"
INSERT INTO cells(id, t, z, y, x, area, detection_confidence, state, track_id, src_id, seg_id,
                  labels, features, reviewed) VALUES
  (1, 0, 1.0,  10.0,  10.0, 100, 0.99, 'normal',   1, 1001, 5, '["ESI"]', '{"area":100}', 0),
  (2, 1, 1.5,  20.0,  20.0, 110, 0.98, NULL,       1, 1002, 5, NULL,      NULL,           0),
  (3, 2, 2.0,  30.0,  30.0, 120, 0.97, 'dividing', 1, 1003, 5, NULL,      NULL,           1),
  (4, 3, 2.5,  40.0,  40.0, 60,  0.90, NULL,       2, 1004, 6, NULL,      NULL,           0),
  (5, 3, 2.5, 400.0, 400.0, 55,  0.80, NULL,       3, 1005, 7, NULL,      NULL,           0),
  (6, 4, 3.0,  50.0,  50.0, 65,  0.85, 'death',    2, 1006, 6, NULL,      NULL,           0),
  (7, 2, NULL, NULL,  NULL, NULL, NULL, NULL,      9, 1007, 8, NULL,      NULL,           0);
INSERT INTO links(parent, child, confidence, reviewed) VALUES
  (1, 2, 0.90, 1),
  (2, 3, 0.20, 0),
  (3, 4, NULL, 0),
  (3, 5, 0.50, 0),
  (4, 6, 0.99, 0);
"#;

    fn seeded() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let project =
            Project::create_or_open(&dir.path().join("data.zarr")).expect("create project");
        project
            .db
            .conn()
            .expect("lock")
            .execute_batch(SEED)
            .expect("seed");
        (dir, project)
    }

    #[test]
    fn cells_window_clips_by_time_and_viewport() {
        let (_dir, project) = seeded();

        let all = project.db.cells_window(1, 2, None).expect("window");
        assert_eq!(
            all.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![2, 3, 7],
            "time window is inclusive and ordered by (t, id)"
        );

        let clipped = project
            .db
            .cells_window(
                0,
                4,
                Some(Bbox {
                    z: [0.0, 10.0],
                    y: [0.0, 100.0],
                    x: [0.0, 100.0],
                }),
            )
            .expect("window");
        assert_eq!(
            clipped.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 6],
            "cell 5 is outside the viewport and cell 7 has no centroid to place"
        );
    }

    #[test]
    fn cell_rows_carry_the_round_trip_columns() {
        let (_dir, project) = seeded();
        let cells = project.db.cells_window(0, 0, None).expect("window");
        let cell = cells.first().expect("cell 1");
        assert_eq!(cell.id, 1);
        assert_eq!(cell.centroid, Some([1.0, 10.0, 10.0]));
        assert_eq!(cell.area, Some(100));
        assert_eq!(cell.state, Some(CellState::Normal));
        assert_eq!(cell.src_id, Some(1001));
        assert_eq!(cell.seg_id, Some(5));
        assert_eq!(cell.labels, vec!["ESI".to_string()]);
        assert_eq!(cell.features.get("area").and_then(Value::as_i64), Some(100));
        assert!(!cell.reviewed);
    }

    #[test]
    fn lineage_spans_track_ids_from_any_member() {
        let (_dir, project) = seeded();
        let tree = project.db.lineage(4).expect("lineage");
        assert_eq!(tree.focus, 4);
        assert_eq!(tree.root, 1);
        assert_eq!(
            tree.cells.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        let tracks: HashSet<_> = tree.cells.iter().filter_map(|c| c.track_id).collect();
        assert_eq!(tracks, HashSet::from([1, 2, 3]));
        assert_eq!(tree.links.len(), 5);
        let division: Vec<_> = tree.links.iter().filter(|l| l.parent == 3).collect();
        assert_eq!(division.len(), 2, "out-degree 2 is a division");
    }

    #[test]
    fn lineage_of_an_isolated_cell_is_itself() {
        let (_dir, project) = seeded();
        let tree = project.db.lineage(7).expect("lineage");
        assert_eq!(tree.root, 7);
        assert_eq!(tree.cells.len(), 1);
        assert!(tree.links.is_empty());
    }

    #[test]
    fn lineage_of_an_unknown_cell_is_an_error() {
        let (_dir, project) = seeded();
        assert!(matches!(
            project.db.lineage(999),
            Err(DbError::UnknownCell(999))
        ));
    }

    #[test]
    fn review_queue_puts_unreviewed_low_confidence_first() {
        let (_dir, project) = seeded();
        let queue = project.db.review_queue(10).expect("queue");
        assert_eq!(
            queue
                .iter()
                .map(|l| (l.parent, l.child, l.confidence, l.reviewed))
                .collect::<Vec<_>>(),
            vec![
                (2, 3, Some(0.20), false),
                (3, 5, Some(0.50), false),
                (4, 6, Some(0.99), false),
                (3, 4, None, false),
                (1, 2, Some(0.90), true),
            ]
        );
        assert_eq!(project.db.review_queue(2).expect("queue").len(), 2);
    }

    #[test]
    fn links_require_existing_cells() {
        let (_dir, project) = seeded();
        let err = project
            .db
            .conn()
            .expect("lock")
            .execute("INSERT INTO links(parent, child) VALUES (1, 4242)", [])
            .expect_err("foreign key must be enforced");
        assert!(
            err.to_string().to_lowercase().contains("foreign key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn version_counters_move_independently() {
        let (_dir, project) = seeded();
        assert_eq!(
            project.db.versions().expect("versions"),
            Versions::default()
        );

        assert_eq!(project.db.bump(VersionCounter::Graph).expect("bump"), 1);
        assert_eq!(project.db.bump(VersionCounter::Graph).expect("bump"), 2);
        assert_eq!(project.db.bump(VersionCounter::Labels).expect("bump"), 1);

        assert_eq!(
            project.db.versions().expect("versions"),
            Versions {
                image: 0,
                labels: 1,
                graph: 2,
                settings: 0,
            }
        );
    }

    #[test]
    fn journal_lists_domain_scope_and_undone_state() {
        let (_dir, project) = seeded();
        let stroke = project
            .db
            .record_edit(
                EditDomain::Mask,
                &serde_json::json!({"kind": "brush", "scope": "3 chunks @ t=12"}),
                &serde_json::json!({"kind": "restore_blobs"}),
            )
            .expect("record");
        let link = project
            .db
            .record_edit(
                EditDomain::Graph,
                &serde_json::json!({"kind": "link", "scope": "1017 → 1093"}),
                &serde_json::json!({"kind": "unlink"}),
            )
            .expect("record");
        let fill = project
            .db
            .record_edit(
                EditDomain::Mask,
                &serde_json::json!({"kind": "fill", "scope": "label 1842 @ t=13"}),
                &serde_json::json!({"kind": "restore_blobs"}),
            )
            .expect("record");
        assert!(stroke < link && link < fill, "seq is monotonic");
        project.db.mark_undone(fill, true).expect("undo");

        let entries = project.db.edits(10).expect("edits");
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.seq, e.domain, e.scope.as_deref(), e.undone))
                .collect::<Vec<_>>(),
            vec![
                (fill, EditDomain::Mask, Some("label 1842 @ t=13"), true),
                (link, EditDomain::Graph, Some("1017 → 1093"), false),
                (stroke, EditDomain::Mask, Some("3 chunks @ t=12"), false),
            ],
            "newest first, undone state as stored"
        );
        assert!(
            entries.iter().all(|e| !e.ts.is_empty()),
            "timestamps come from the database clock"
        );
        assert_eq!(project.db.edits(2).expect("edits").len(), 2);
    }

    #[test]
    fn journal_falls_back_to_the_op_kind_for_scope() {
        let (_dir, project) = seeded();
        project
            .db
            .record_edit(
                EditDomain::Graph,
                &serde_json::json!({"kind": "cut"}),
                &Value::Null,
            )
            .expect("record");
        let entries = project.db.edits(1).expect("edits");
        assert_eq!(entries[0].scope.as_deref(), Some("cut"));
    }
}
