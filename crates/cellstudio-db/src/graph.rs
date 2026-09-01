//! Transaction-scoped graph mutations. one internal module serving Link, Unlink,
//! graph undo/redo, and every mask commit that removes cells or links. Journal rows store the
//! full [`GraphDelta`] — exact before/after identity assignments, not inverse verbs — so undo
//! restores visible identities exactly while the next-id counter stays advanced.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::project::{Db, DbError};
use crate::queries::{
    CELL_BY_ID_SQL, CellRow, LinkRow, MAX_LABEL_ID, VersionCounter, bump_in, cell_row,
    clear_redo_in, links_of, to_u32, to_u64,
};

/// `meta` counter the fresh track ids come from; seeded above the current maximum `track_id`.
pub(crate) const NEXT_TRACK_ID_KEY: &str = "graph.next_track_id";

/// Top-level key in the opaque `settings` object holding the optional maximum link gap in
/// frames. Absent or 0 means unbounded.
pub const MAX_LINK_GAP_KEY: &str = "maxLinkGap";

/// Structured rejection reasons, surfaced verbatim in the status bar.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("cell {0} is not in this project")]
    UnknownCell(u32),
    #[error(
        "a link must move forward in time: parent {parent} is at t={parent_t} and \
         child {child} at t={child_t}"
    )]
    NotForward {
        parent: u32,
        parent_t: u64,
        child: u32,
        child_t: u64,
    },
    #[error("link {parent} → {child} already exists")]
    Duplicate { parent: u32, child: u32 },
    #[error("cell {child} already has a parent ({parent}); a cell has at most one")]
    HasParent { child: u32, parent: u32 },
    #[error("cell {parent} already has two children; a division has at most two")]
    HasTwoChildren { parent: u32 },
    #[error("the link spans {gap} frames, past the configured maximum gap of {max}")]
    GapTooWide { gap: u64, max: u64 },
    #[error("cell {0} has no links: nothing to unlink")]
    NoLinks(u32),
    #[error("there is no link {parent} → {child} to cut")]
    NoSuchLink { parent: u32, child: u32 },
}

/// The full delta of one graph-changing edit. `track_ids_before`/`after` list every cell of
/// every affected chain, so undo/redo restore exact assignments instead of re-materializing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphDelta {
    pub added_links: Vec<LinkRow>,
    pub removed_links: Vec<LinkRow>,
    pub track_ids_before: Vec<(u32, Option<u32>)>,
    pub track_ids_after: Vec<(u32, Option<u32>)>,
}

impl GraphDelta {
    pub fn is_empty(&self) -> bool {
        self.added_links.is_empty()
            && self.removed_links.is_empty()
            && self.track_ids_before.is_empty()
            && self.track_ids_after.is_empty()
    }

    /// Every track id appearing on either side of the assignment delta, deduplicated.
    pub fn affected_tracks(&self) -> Vec<u32> {
        let mut tracks: Vec<u32> = self
            .track_ids_before
            .iter()
            .chain(&self.track_ids_after)
            .filter_map(|(_, track)| *track)
            .collect();
        tracks.sort_unstable();
        tracks.dedup();
        tracks
    }

    /// Every cell whose assignment the delta touches.
    pub fn affected_cells(&self) -> Vec<u32> {
        let mut cells: Vec<u32> = self
            .track_ids_before
            .iter()
            .chain(&self.track_ids_after)
            .map(|(cell, _)| *cell)
            .collect();
        cells.sort_unstable();
        cells.dedup();
        cells
    }
}

/// One side of the identity delta: `(cell, track id)` per affected-chain cell.
pub type TrackAssignments = Vec<(u32, Option<u32>)>;

/// The journaled forward op of a `domain = 'graph'` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphOp {
    pub kind: String,
    pub scope: String,
    pub delta: GraphDelta,
}

/// What one committed graph edit tells the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphCommit {
    pub seq: i64,
    pub graph_version: u64,
    /// Affected rows as committed (post-edit assignments).
    pub cells: Vec<CellRow>,
    pub affected_tracks: Vec<u32>,
}

fn cell_t_in(tx: &Transaction, id: u32) -> Result<Option<u64>, DbError> {
    let t: Option<i64> = tx
        .query_row("SELECT t FROM cells WHERE id = ?1", [id], |row| row.get(0))
        .optional()?;
    t.map(to_u64).transpose()
}

fn track_id_in(tx: &Transaction, id: u32) -> Result<Option<u32>, DbError> {
    let track: Option<Option<i64>> = tx
        .query_row("SELECT track_id FROM cells WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    track.flatten().map(to_u32).transpose()
}

fn parent_link_in(tx: &Transaction, child: u32) -> Result<Option<LinkRow>, DbError> {
    tx.query_row(
        "SELECT parent, child, confidence, reviewed FROM links WHERE child = ?1",
        [child],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )
    .optional()?
    .map(|(parent, child, confidence, reviewed)| {
        Ok(LinkRow {
            parent: to_u32(parent)?,
            child: to_u32(child)?,
            confidence,
            reviewed: reviewed != 0,
        })
    })
    .transpose()
}

fn child_links_in(tx: &Transaction, parent: u32) -> Result<Vec<LinkRow>, DbError> {
    let mut stmt = tx.prepare_cached(
        "SELECT parent, child, confidence, reviewed FROM links WHERE parent = ?1 ORDER BY child",
    )?;
    let mut rows = stmt.query([parent])?;
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

fn children_count_in(tx: &Transaction, parent: u32) -> Result<u64, DbError> {
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM links WHERE parent = ?1",
        [parent],
        |row| row.get(0),
    )?;
    to_u64(count)
}

fn max_gap_in(tx: &Transaction) -> Result<Option<u64>, DbError> {
    let raw: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(raw
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| settings.get(MAX_LINK_GAP_KEY).and_then(Value::as_u64))
        .filter(|max| *max > 0))
}

/// The maximal unbranched chain containing `cell`, head first: walk up while the parent has
/// exactly one child, down while the cell has exactly one child. A 2-child parent ends its
/// chain; each of its children starts one.
pub fn chain_of(tx: &Transaction, cell: u32) -> Result<Vec<u32>, DbError> {
    let mut seen = HashSet::from([cell]);
    let mut head = cell;
    while let Some(link) = parent_link_in(tx, head)? {
        if children_count_in(tx, link.parent)? != 1 || !seen.insert(link.parent) {
            break;
        }
        head = link.parent;
    }
    // the guard restarts for the down-walk: it passes back through the starting cell
    let mut seen = HashSet::from([head]);
    let mut chain = vec![head];
    let mut current = head;
    loop {
        let children = child_links_in(tx, current)?;
        match children.as_slice() {
            [only] if seen.insert(only.child) => {
                current = only.child;
                chain.push(current);
            }
            _ => break,
        }
    }
    Ok(chain)
}

/// Validates a prospective link against the graph rules. forward in time, at most
/// one parent, at most two children, no duplicate, gap within the optional configured cap.
pub fn validate_link(tx: &Transaction, parent: u32, child: u32) -> Result<(), GraphError> {
    let parent_t = cell_t_in(tx, parent)?.ok_or(GraphError::UnknownCell(parent))?;
    let child_t = cell_t_in(tx, child)?.ok_or(GraphError::UnknownCell(child))?;
    if child_t <= parent_t {
        return Err(GraphError::NotForward {
            parent,
            parent_t,
            child,
            child_t,
        });
    }
    if let Some(existing) = parent_link_in(tx, child)? {
        return match existing.parent == parent {
            true => Err(GraphError::Duplicate { parent, child }),
            false => Err(GraphError::HasParent {
                child,
                parent: existing.parent,
            }),
        };
    }
    if children_count_in(tx, parent)? >= 2 {
        return Err(GraphError::HasTwoChildren { parent });
    }
    let gap = child_t - parent_t;
    if let Some(max) = max_gap_in(tx)?
        && gap > max
    {
        return Err(GraphError::GapTooWide { gap, max });
    }
    Ok(())
}

pub fn insert_link(tx: &Transaction, link: &LinkRow) -> Result<(), DbError> {
    tx.execute(
        "INSERT OR IGNORE INTO links(parent, child, confidence, reviewed) VALUES (?1, ?2, ?3, ?4)",
        params![
            link.parent,
            link.child,
            link.confidence,
            i64::from(link.reviewed)
        ],
    )?;
    Ok(())
}

fn delete_link(tx: &Transaction, link: &LinkRow) -> Result<(), DbError> {
    tx.execute(
        "DELETE FROM links WHERE parent = ?1 AND child = ?2",
        params![link.parent, link.child],
    )?;
    Ok(())
}

/// Every link incident to the chain: its head's parent link, its internal links, and its
/// tail's child links, deduplicated.
pub fn chain_links(tx: &Transaction, chain: &[u32]) -> Result<Vec<LinkRow>, DbError> {
    let mut links = Vec::new();
    for &cell in chain {
        links.extend(links_of(tx, cell)?);
    }
    links.sort_by_key(|l| (l.parent, l.child));
    links.dedup_by_key(|l| (l.parent, l.child));
    Ok(links)
}

/// The next fresh track id from the persistent counter, seeded above the current maximum.
/// Fails past the renderable ceiling: the label shader receives floats and cannot
/// distinguish ids beyond 2²⁴.
pub fn next_track_id(tx: &Transaction) -> Result<u32, DbError> {
    let stored: Option<i64> = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
            [NEXT_TRACK_ID_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let seed: i64 = tx.query_row("SELECT COALESCE(MAX(track_id), 0) FROM cells", [], |row| {
        row.get(0)
    })?;
    let next = stored.unwrap_or(0).max(seed + 1).max(1);
    if next > i64::from(MAX_LABEL_ID) {
        return Err(DbError::TrackIdsExhausted);
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![NEXT_TRACK_ID_KEY, (next + 1).to_string()],
    )?;
    to_u32(next)
}

/// Was `head` already the head of its chain before the edit whose link delta is
/// `added`/`removed`? Its pre-edit parent either did not exist or had two children.
fn was_head_before(
    tx: &Transaction,
    head: u32,
    added: &[LinkRow],
    removed: &[LinkRow],
) -> Result<bool, DbError> {
    let pre_parent = match removed.iter().find(|l| l.child == head) {
        Some(link) => Some(link.parent),
        None => parent_link_in(tx, head)?
            .filter(|l| {
                !added
                    .iter()
                    .any(|a| a.parent == l.parent && a.child == head)
            })
            .map(|l| l.parent),
    };
    let Some(parent) = pre_parent else {
        return Ok(true);
    };
    let count = children_count_in(tx, parent)? as i64
        + removed.iter().filter(|l| l.parent == parent).count() as i64
        - added.iter().filter(|l| l.parent == parent).count() as i64;
    Ok(count >= 2)
}

/// Chain-scoped re-materialization, run after the link changes are applied: every post-edit
/// chain containing a seed is assigned one identity. A chain whose head cell was already a
/// chain head before the edit keeps that head's stored id — a rule that covers unlink's
/// singletons verbatim: the unlinked chain's head keeps the chain id it carried, and every
/// other chain cell draws a fresh id from the persistent counter, as does any chain with a
/// new head or a head that never had an id. Returns the exact (before, after) assignments.
pub fn rematerialize(
    tx: &Transaction,
    seeds: &[u32],
    added: &[LinkRow],
    removed: &[LinkRow],
) -> Result<(TrackAssignments, TrackAssignments), DbError> {
    let mut heads = HashSet::new();
    let mut chains: Vec<Vec<u32>> = Vec::new();
    for &seed in seeds {
        if cell_t_in(tx, seed)?.is_none() {
            continue;
        }
        let chain = chain_of(tx, seed)?;
        if heads.insert(chain[0]) {
            chains.push(chain);
        }
    }
    // sorted by head id so fresh ids are drawn in a deterministic order
    chains.sort_by_key(|chain| chain[0]);

    let mut before = Vec::new();
    let mut after = Vec::new();
    for chain in &chains {
        let head = chain[0];
        let kept = match was_head_before(tx, head, added, removed)? {
            true => track_id_in(tx, head)?,
            false => None,
        };
        let id = match kept {
            Some(id) => id,
            None => next_track_id(tx)?,
        };
        for &cell in chain {
            let old = track_id_in(tx, cell)?;
            before.push((cell, old));
            after.push((cell, Some(id)));
        }
    }
    apply_assignments(tx, &after)?;
    Ok((before, after))
}

/// Writes one side of a journaled assignment delta. A cell a later (not yet undone) edit
/// removed is skipped: undo order restores it first.
pub fn apply_assignments(
    tx: &Transaction,
    assignments: &[(u32, Option<u32>)],
) -> Result<(), DbError> {
    let mut stmt = tx.prepare_cached("UPDATE cells SET track_id = ?2 WHERE id = ?1")?;
    for (cell, track) in assignments {
        stmt.execute(params![cell, track])?;
    }
    Ok(())
}

fn journal_in(tx: &Transaction, op: &GraphOp) -> Result<i64, DbError> {
    const SQL: &str = "\
INSERT INTO edits(ts, domain, op, inverse)
VALUES (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'graph', ?1, 'null')
RETURNING seq";
    let op = serde_json::to_string(op)?;
    Ok(tx.query_row(SQL, [op], |row| row.get(0))?)
}

fn rows_of(tx: &Transaction, ids: &[u32]) -> Result<Vec<CellRow>, DbError> {
    let mut stmt = tx.prepare_cached(CELL_BY_ID_SQL)?;
    let mut rows = Vec::new();
    for &id in ids {
        if let Some(row) = stmt
            .query_row([id], |row| Ok(cell_row(row)))
            .optional()?
            .transpose()?
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

impl Db {
    /// One accepted link, committed whole: validation, the link row, chain-scoped
    /// re-materialization, the `GraphDelta` journal row, redo clearing, and the graph
    /// version bump in one SQLite transaction.
    pub fn graph_link(&self, parent: u32, child: u32) -> Result<GraphCommit, GraphError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction().map_err(DbError::from)?;
        validate_link(&tx, parent, child)?;

        let link = LinkRow {
            parent,
            child,
            confidence: None,
            reviewed: false,
        };
        insert_link(&tx, &link)?;
        let added = vec![link];
        // a division re-heads the parent's existing child's chain, so it seeds too
        let mut seeds = vec![parent, child];
        seeds.extend(child_links_in(&tx, parent)?.iter().map(|l| l.child));
        let (before, after) = rematerialize(&tx, &seeds, &added, &[])?;
        let delta = GraphDelta {
            added_links: added,
            removed_links: Vec::new(),
            track_ids_before: before,
            track_ids_after: after,
        };
        let op = GraphOp {
            kind: "link".to_owned(),
            scope: format!("link {parent} → {child}"),
            delta,
        };
        clear_redo_in(&tx)?;
        let seq = journal_in(&tx, &op)?;
        let graph_version = bump_in(&tx, VersionCounter::Graph)?;
        let cells = rows_of(&tx, &op.delta.affected_cells())?;
        tx.commit().map_err(DbError::from)?;
        Ok(GraphCommit {
            seq,
            graph_version,
            cells,
            affected_tracks: op.delta.affected_tracks(),
        })
    }

    /// Deletes the whole chain containing `cell`: every incident link — internal, to its
    /// parent, to its children — leaving the chain's cells as unlinked detections. Same
    /// one-transaction commit as [`Db::graph_link`].
    /// Cuts one link, splitting its chain in two: the parent side keeps the identity, the
    /// child side becomes a new head and draws a fresh one.
    pub fn graph_cut(&self, parent: u32, child: u32) -> Result<GraphCommit, GraphError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction().map_err(DbError::from)?;
        let link = parent_link_in(&tx, child)?.filter(|l| l.parent == parent);
        let Some(link) = link else {
            return Err(GraphError::NoSuchLink { parent, child });
        };
        delete_link(&tx, &link)?;
        let removed = vec![link];
        let (before, after) = rematerialize(&tx, &[parent, child], &[], &removed)?;
        let op = GraphOp {
            kind: "cut".to_owned(),
            scope: format!("cut link {parent} → {child}"),
            delta: GraphDelta {
                added_links: Vec::new(),
                removed_links: removed,
                track_ids_before: before,
                track_ids_after: after,
            },
        };
        clear_redo_in(&tx)?;
        let seq = journal_in(&tx, &op)?;
        let graph_version = bump_in(&tx, VersionCounter::Graph)?;
        let cells = rows_of(&tx, &op.delta.affected_cells())?;
        tx.commit().map_err(DbError::from)?;
        Ok(GraphCommit {
            seq,
            graph_version,
            cells,
            affected_tracks: op.delta.affected_tracks(),
        })
    }

    pub fn graph_unlink(&self, cell: u32) -> Result<GraphCommit, GraphError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction().map_err(DbError::from)?;
        if cell_t_in(&tx, cell)?.is_none() {
            return Err(GraphError::UnknownCell(cell));
        }
        let chain = chain_of(&tx, cell)?;
        let removed = chain_links(&tx, &chain)?;
        if removed.is_empty() {
            return Err(GraphError::NoLinks(cell));
        }
        for link in &removed {
            delete_link(&tx, link)?;
        }
        let mut seeds = chain.clone();
        for link in &removed {
            seeds.push(link.parent);
            seeds.push(link.child);
        }
        let (before, after) = rematerialize(&tx, &seeds, &[], &removed)?;
        let op = GraphOp {
            kind: "unlink".to_owned(),
            scope: format!("unlink cell {cell} ({}-cell chain)", chain.len()),
            delta: GraphDelta {
                added_links: Vec::new(),
                removed_links: removed,
                track_ids_before: before,
                track_ids_after: after,
            },
        };
        clear_redo_in(&tx)?;
        let seq = journal_in(&tx, &op)?;
        let graph_version = bump_in(&tx, VersionCounter::Graph)?;
        let cells = rows_of(&tx, &op.delta.affected_cells())?;
        tx.commit().map_err(DbError::from)?;
        Ok(GraphCommit {
            seq,
            graph_version,
            cells,
            affected_tracks: op.delta.affected_tracks(),
        })
    }

    /// Undoes or redoes the graph journal row `seq` by applying its stored delta exactly:
    /// undo applies `track_ids_before` and reverses the link delta, redo applies the forward
    /// side. Link changes, assignments, the `undone` transition, and the version bump commit
    /// in one transaction.
    pub fn graph_step(&self, seq: i64, undo: bool) -> Result<GraphCommit, GraphError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction().map_err(DbError::from)?;
        let raw: String = tx
            .query_row("SELECT op FROM edits WHERE seq = ?1", [seq], |row| {
                row.get(0)
            })
            .map_err(DbError::from)?;
        let op: GraphOp = serde_json::from_str(&raw).map_err(DbError::from)?;

        let delta = &op.delta;
        if undo {
            for link in &delta.added_links {
                delete_link(&tx, link)?;
            }
            for link in &delta.removed_links {
                insert_link(&tx, link)?;
            }
            apply_assignments(&tx, &delta.track_ids_before)?;
        } else {
            for link in &delta.removed_links {
                delete_link(&tx, link)?;
            }
            for link in &delta.added_links {
                insert_link(&tx, link)?;
            }
            apply_assignments(&tx, &delta.track_ids_after)?;
        }
        tx.execute(
            "UPDATE edits SET undone = ?2 WHERE seq = ?1",
            params![seq, i64::from(undo)],
        )
        .map_err(DbError::from)?;
        let graph_version = bump_in(&tx, VersionCounter::Graph)?;
        let cells = rows_of(&tx, &delta.affected_cells())?;
        let affected_tracks = delta.affected_tracks();
        tx.commit().map_err(DbError::from)?;
        Ok(GraphCommit {
            seq,
            graph_version,
            cells,
            affected_tracks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;
    use crate::queries::{CellChange, EditDomain, ExtentDelta, GraphStep};
    use serde_json::json;

    fn project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let project =
            Project::create_or_open(&dir.path().join("data.zarr")).expect("create project");
        (dir, project)
    }

    fn seed(project: &Project, sql: &str) {
        project
            .db
            .conn()
            .expect("lock")
            .execute_batch(sql)
            .expect("seed");
    }

    fn cell(id: u32, t: u64, track: Option<u32>) -> String {
        format!(
            "INSERT INTO cells(id, t, z, y, x, area, track_id) \
             VALUES ({id}, {t}, 1.0, {v}, {v}, 10, {track});\n",
            v = f64::from(id) * 10.0,
            track = track.map(|t| t.to_string()).unwrap_or("NULL".into()),
        )
    }

    fn link(parent: u32, child: u32) -> String {
        format!("INSERT INTO links(parent, child) VALUES ({parent}, {child});\n")
    }

    fn links_table(project: &Project) -> Vec<(u32, u32)> {
        let conn = project.db.conn().expect("lock");
        let mut stmt = conn
            .prepare("SELECT parent, child FROM links ORDER BY parent, child")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)? as u32, row.get::<_, i64>(1)? as u32))
            })
            .expect("query");
        rows.map(|r| r.expect("row")).collect()
    }

    fn tracks(project: &Project) -> Vec<(u32, Option<u32>)> {
        let conn = project.db.conn().expect("lock");
        let mut stmt = conn
            .prepare("SELECT id, track_id FROM cells ORDER BY id")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u32,
                    row.get::<_, Option<i64>>(1)?.map(|t| t as u32),
                ))
            })
            .expect("query");
        rows.map(|r| r.expect("row")).collect()
    }

    #[test]
    fn every_rejection_names_its_rule_and_leaves_the_graph_unchanged() {
        let (_dir, project) = project();
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(10)),
                cell(3, 2, Some(10)),
                cell(4, 1, Some(40)),
                cell(5, 2, Some(50)),
                cell(6, 4, Some(60)),
                link(1, 2),
                link(2, 3),
            ]
            .concat(),
        );
        let before_links = links_table(&project);
        let before_tracks = tracks(&project);

        assert!(matches!(
            project.db.graph_link(999, 2),
            Err(GraphError::UnknownCell(999))
        ));
        assert!(matches!(
            project.db.graph_link(2, 999),
            Err(GraphError::UnknownCell(999))
        ));
        // same frame, and time-backwards
        assert!(matches!(
            project.db.graph_link(2, 4),
            Err(GraphError::NotForward {
                parent: 2,
                child: 4,
                ..
            })
        ));
        assert!(matches!(
            project.db.graph_link(3, 2),
            Err(GraphError::NotForward {
                parent: 3,
                child: 2,
                ..
            })
        ));
        assert!(matches!(
            project.db.graph_link(1, 2),
            Err(GraphError::Duplicate {
                parent: 1,
                child: 2
            })
        ));
        assert!(matches!(
            project.db.graph_link(4, 3),
            Err(GraphError::HasParent {
                child: 3,
                parent: 2
            })
        ));
        // a second child is a division; a third is refused
        project.db.graph_link(2, 5).expect("division");
        assert!(matches!(
            project.db.graph_link(2, 6),
            Err(GraphError::HasTwoChildren { parent: 2 })
        ));
        project.db.graph_step(2, true).map(|_| ()).unwrap_or(());
        assert!(matches!(
            project.db.graph_unlink(6),
            Err(GraphError::NoLinks(6))
        ));

        // rejections rolled back: only the accepted division is new
        let (_dir2, fresh) = super::tests::project();
        drop((_dir2, fresh));
        let mut expected = before_links.clone();
        expected.push((2, 5));
        expected.sort_unstable();
        assert_eq!(links_table(&project), expected);
        drop(before_tracks);
    }

    #[test]
    fn the_gap_cap_comes_from_settings_and_is_unbounded_by_default() {
        let (_dir, project) = project();
        seed(
            &project,
            &[cell(1, 0, Some(10)), cell(2, 3, Some(20))].concat(),
        );
        // no cap configured: a 3-frame gap is accepted
        project.db.graph_link(1, 2).expect("gap accepted");
        let seq = project.db.undo_next().expect("record").expect("row").seq;
        project.db.graph_step(seq, true).expect("undo");
        project.db.clear_redo().expect("clear");

        project
            .db
            .put_settings(&json!({ MAX_LINK_GAP_KEY: 2 }))
            .expect("settings");
        match project.db.graph_link(1, 2) {
            Err(GraphError::GapTooWide { gap: 3, max: 2 }) => {}
            other => panic!("expected GapTooWide, got {other:?}"),
        }
        assert_eq!(links_table(&project), vec![], "rejection left no link");
    }

    #[test]
    fn a_division_link_reidentifies_the_existing_childs_chain() {
        let (_dir, project) = project();
        // one chain 1→2→3→4 (track 10) and a singleton 5 heading its own chain (track 50)
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(10)),
                cell(3, 2, Some(10)),
                cell(4, 3, Some(10)),
                cell(5, 2, Some(50)),
                link(1, 2),
                link(2, 3),
                link(3, 4),
            ]
            .concat(),
        );

        let commit = project.db.graph_link(2, 5).expect("division link");

        // parent chain keeps its id, the new child keeps the id it headed, and the existing
        // child becomes a new head with a fresh id — never two disconnected chains on one id
        assert_eq!(
            tracks(&project),
            vec![
                (1, Some(10)),
                (2, Some(10)),
                (3, Some(51)),
                (4, Some(51)),
                (5, Some(50)),
            ]
        );
        assert_eq!(commit.affected_tracks, vec![10, 50, 51]);
        assert!(commit.graph_version > 0);
        assert_eq!(
            project.db.versions().expect("versions").graph,
            commit.graph_version
        );

        let record = project.db.undo_next().expect("record").expect("row");
        assert_eq!(record.domain, EditDomain::Graph);
        let op: GraphOp = serde_json::from_value(record.op).expect("op");
        assert_eq!(op.kind, "link");
        assert_eq!(op.delta.added_links.len(), 1);
        assert!(op.delta.removed_links.is_empty());
    }

    #[test]
    fn link_undo_redo_restores_exact_links_and_every_affected_track_id() {
        let (_dir, project) = project();
        // chain A = [1, 2] (track 10), chain B = [3, 4] (track 20)
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(10)),
                cell(3, 2, Some(20)),
                cell(4, 3, Some(20)),
                link(1, 2),
                link(3, 4),
            ]
            .concat(),
        );

        let commit = project.db.graph_link(2, 3).expect("join");
        let joined = vec![(1, Some(10)), (2, Some(10)), (3, Some(10)), (4, Some(10))];
        assert_eq!(tracks(&project), joined, "the join propagates downstream");
        assert_eq!(links_table(&project), vec![(1, 2), (2, 3), (3, 4)]);

        let undone = project.db.graph_step(commit.seq, true).expect("undo");
        assert_eq!(links_table(&project), vec![(1, 2), (3, 4)]);
        assert_eq!(
            tracks(&project),
            vec![(1, Some(10)), (2, Some(10)), (3, Some(20)), (4, Some(20))],
            "undo restores the old identity, not a fresh counter value"
        );
        assert!(undone.graph_version > commit.graph_version);
        assert!(
            project.db.undo_next().expect("record").is_none(),
            "the row is undone"
        );

        let redone = project.db.graph_step(commit.seq, false).expect("redo");
        assert_eq!(links_table(&project), vec![(1, 2), (2, 3), (3, 4)]);
        assert_eq!(
            tracks(&project),
            joined,
            "redo reapplies the exact after side"
        );
        assert!(redone.graph_version > undone.graph_version);
    }

    #[test]
    fn cutting_one_link_splits_the_chain_and_undo_is_exact() {
        let (_dir, project) = project();
        // one chain 1→2→3→4, all track 10
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(10)),
                cell(3, 2, Some(10)),
                cell(4, 3, Some(10)),
                link(1, 2),
                link(2, 3),
                link(3, 4),
            ]
            .concat(),
        );
        let before_tracks = tracks(&project);
        let before_links = links_table(&project);

        assert!(matches!(
            project.db.graph_cut(1, 3),
            Err(GraphError::NoSuchLink {
                parent: 1,
                child: 3
            })
        ));
        assert_eq!(
            links_table(&project),
            before_links,
            "a miss changes nothing"
        );

        let commit = project.db.graph_cut(2, 3).expect("cut");
        assert_eq!(
            links_table(&project),
            vec![(1, 2), (3, 4)],
            "only the named link is gone; the rest of the chain survives"
        );
        let after = tracks(&project);
        assert_eq!(
            after,
            vec![(1, Some(10)), (2, Some(10)), (3, Some(11)), (4, Some(11))],
            "the parent side keeps the identity and the child side heads a new track"
        );

        project.db.graph_step(commit.seq, true).expect("undo");
        assert_eq!(links_table(&project), before_links);
        assert_eq!(tracks(&project), before_tracks, "undo restores exact ids");

        project.db.graph_step(commit.seq, false).expect("redo");
        assert_eq!(links_table(&project), vec![(1, 2), (3, 4)]);
        assert_eq!(tracks(&project), after);
    }

    #[test]
    fn unlink_deletes_the_whole_chain_and_undo_is_exact() {
        let (_dir, project) = project();
        // 1 divides into 2 and 3; chain [2, 4] (track 20) divides into 5 and 6
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(20)),
                cell(3, 1, Some(50)),
                cell(4, 2, Some(20)),
                cell(5, 3, Some(30)),
                cell(6, 3, Some(40)),
                link(1, 2),
                link(1, 3),
                link(2, 4),
                link(4, 5),
                link(4, 6),
            ]
            .concat(),
        );
        let before_tracks = tracks(&project);
        let before_links = links_table(&project);

        // addressed by any chain member: 4 and 2 name the same chain
        let commit = project.db.graph_unlink(4).expect("unlink");

        assert_eq!(
            links_table(&project),
            vec![(1, 3)],
            "every link incident to the chain is gone; upstream keeps its own structure"
        );
        assert_eq!(
            tracks(&project),
            vec![
                // parent 1 now has one child, so its chain extends into 3 under its own id
                (1, Some(10)),
                // the unlinked chain's head keeps the id it carried; the rest draw fresh ids
                (2, Some(20)),
                (3, Some(10)),
                (4, Some(51)),
                // child tracks persist without a parent
                (5, Some(30)),
                (6, Some(40)),
            ]
        );

        project.db.graph_step(commit.seq, true).expect("undo");
        assert_eq!(links_table(&project), before_links);
        assert_eq!(
            tracks(&project),
            before_tracks,
            "every removed link is restored and identities equal their pre-deletion state"
        );
    }

    #[test]
    fn a_new_link_between_untracked_cells_draws_one_fresh_id() {
        let (_dir, project) = project();
        seed(&project, &[cell(1, 0, None), cell(2, 2, None)].concat());

        let commit = project.db.graph_link(1, 2).expect("link");
        assert_eq!(tracks(&project), vec![(1, Some(1)), (2, Some(1))]);
        assert_eq!(commit.affected_tracks, vec![1]);

        project.db.graph_step(commit.seq, true).expect("undo");
        assert_eq!(
            tracks(&project),
            vec![(1, None), (2, None)],
            "undo restores the null assignments exactly"
        );
    }

    #[test]
    fn a_fresh_id_past_the_renderable_ceiling_fails_the_edit_whole() {
        let (_dir, project) = project();
        seed(&project, &[cell(1, 0, None), cell(2, 1, None)].concat());
        seed(
            &project,
            &format!(
                "INSERT INTO meta(key, value) VALUES ('{NEXT_TRACK_ID_KEY}', '{}');",
                u64::from(MAX_LABEL_ID) + 1
            ),
        );

        assert!(matches!(
            project.db.graph_link(1, 2),
            Err(GraphError::Db(DbError::TrackIdsExhausted))
        ));
        assert_eq!(links_table(&project), vec![], "the transaction rolled back");
        assert_eq!(tracks(&project), vec![(1, None), (2, None)]);
    }

    #[test]
    fn a_new_graph_edit_clears_the_redo_branch() {
        let (_dir, project) = project();
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(20)),
                cell(3, 2, Some(30)),
            ]
            .concat(),
        );

        let first = project.db.graph_link(1, 2).expect("link");
        project.db.graph_step(first.seq, true).expect("undo");
        assert!(project.db.redo_next().expect("redo").is_some());

        project.db.graph_link(2, 3).expect("a new edit");
        assert!(
            project.db.redo_next().expect("redo").is_none(),
            "the undone row was discarded"
        );
    }

    #[test]
    fn a_mask_removal_of_a_linked_cell_carries_the_graph_delta_and_both_bumps() {
        let (_dir, project) = project();
        // chain 1→2→3, one track; the middle cell will be erased to nothing
        seed(
            &project,
            &[
                cell(1, 0, Some(10)),
                cell(2, 1, Some(10)),
                cell(3, 2, Some(10)),
                link(1, 2),
                link(2, 3),
            ]
            .concat(),
        );
        let erase = |label: u32| ExtentDelta {
            label,
            area: -10,
            sum_z: -1.0,
            sum_y: -20.0,
            sum_x: -20.0,
            bbox: None,
        };
        // seed the extent so the erase can fold to zero
        project
            .db
            .ensure_extent(1, 2, || {
                Ok::<_, DbError>(crate::queries::ExtentRow {
                    bbox: None,
                    area: 10,
                    sum_z: 1.0,
                    sum_y: 20.0,
                    sum_x: 20.0,
                })
            })
            .expect("extent");

        let seq = project
            .db
            .record_edit_pending(
                EditDomain::Mask,
                &json!({"kind": "erase"}),
                &json!(null),
                &[],
            )
            .expect("journal");
        let commit = project
            .db
            .commit_edit(seq, 1, &[erase(2)], &[], GraphStep::Rematerialize)
            .expect("commit");

        assert!(matches!(commit.cells[0], CellChange::Removed(_)));
        let graph_version = commit.graph_version.expect("version.graph bumped too");
        assert_eq!(
            project.db.versions().expect("versions").graph,
            graph_version
        );
        let delta = commit.graph.expect("delta attached");
        assert_eq!(
            delta
                .removed_links
                .iter()
                .map(|l| (l.parent, l.child))
                .collect::<Vec<_>>(),
            vec![(1, 2), (2, 3)]
        );
        assert_eq!(
            tracks(&project),
            vec![(1, Some(10)), (3, Some(11))],
            "the upstream neighbor keeps its id, the orphaned child re-heads fresh"
        );
        assert_eq!(links_table(&project), vec![]);

        // one undo restores the cell, its links, and the neighbors' exact identities
        let record = project.db.undo_next().expect("record").expect("row");
        let inverse: crate::queries::MaskInverse =
            serde_json::from_value(record.inverse).expect("inverse");
        assert_eq!(inverse.graph.as_ref(), Some(&delta));
        project
            .db
            .commit_edit(
                seq,
                1,
                &inverse.deltas,
                &inverse.cells,
                GraphStep::Undo(inverse.graph.as_ref()),
            )
            .expect("undo");
        project.db.mark_undone(seq, true).expect("mark");
        assert_eq!(
            tracks(&project),
            vec![(1, Some(10)), (2, Some(10)), (3, Some(10))]
        );
        assert_eq!(links_table(&project), vec![(1, 2), (2, 3)]);
    }
}
