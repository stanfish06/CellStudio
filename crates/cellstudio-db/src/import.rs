use cellstudio_core::tracks::{CellRecord, ParseError};
use rusqlite::{Connection, Transaction, params};

use crate::project::{Db, DbError};
use crate::queries::{MAX_LABEL_ID, VersionCounter, bump_in};

/// Staging mirrors the tracking file, not the project: `id` is the file's cell id, which
/// only becomes a project id at materialization.
///
/// Links are staged per declaring side. A file may state a link under the parent's
/// `children`, the child's `parent`, or both, so `validate_staged` can spot the two sides
/// disagreeing instead of silently merging them.
pub(crate) const STAGING_TABLES: &str = r#"
CREATE TABLE staging_cells (
  id INTEGER PRIMARY KEY,           -- cell id as written in the tracking file
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
  side TEXT NOT NULL,               -- 'children' | 'parent': which side of the file declared it
  PRIMARY KEY (parent, child, side)
);
CREATE INDEX staging_links_by_child ON staging_links(child);
"#;

/// Records staged per transaction.
const STAGE_BATCH: usize = 4096;
/// Offending ids reported per validation rule; enough to act on, bounded on huge files.
const OFFENDER_CAP: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StagingReport {
    pub cells: u64,
    pub links: u64,
    pub max_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    pub cell_id: u32,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphSummary {
    pub cells: u64,
    pub links: u64,
    pub tracks: u64,
    pub divisions: u64,
}

/// Why staging stopped. Either way the staging tables were cleared before returning: an
/// aborted stream never leaves rows for a later import to inherit.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("{0}")]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("cell id {0} appears more than once")]
    DuplicateCell(u32),
    #[error("cell {parent} states its link to {child} more than once on the same side")]
    DuplicateLink { parent: u32, child: u32 },
}

fn clear_staging_in(conn: &Connection) -> Result<(), DbError> {
    conn.execute("DELETE FROM staging_links", [])?;
    conn.execute("DELETE FROM staging_cells", [])?;
    Ok(())
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn stage_cell(stmt: &mut rusqlite::Statement<'_>, record: &CellRecord) -> Result<(), StageError> {
    let t = i64::try_from(record.t).map_err(|_| DbError::OutOfRange(i64::MAX))?;
    let [z, y, x] = match record.centroid {
        Some([z, y, x]) => [Some(z), Some(y), Some(x)],
        None => [None; 3],
    };
    let labels = match record.labels.is_empty() {
        true => None,
        false => Some(serde_json::to_string(&record.labels).map_err(DbError::from)?),
    };
    let features = match record.features.is_empty() {
        true => None,
        false => Some(serde_json::to_string(&record.features).map_err(DbError::from)?),
    };
    stmt.execute(params![
        record.id,
        t,
        z,
        y,
        x,
        record.seg_id,
        record.track_id,
        record.confidence,
        record.state.map(|s| s.as_str()),
        labels,
        features,
    ])
    .map_err(|e| match is_unique_violation(&e) {
        true => StageError::DuplicateCell(record.id),
        false => DbError::from(e).into(),
    })?;
    Ok(())
}

fn stage_links(stmt: &mut rusqlite::Statement<'_>, record: &CellRecord) -> Result<u64, StageError> {
    let mut staged = 0;
    let mut insert = |parent: u32, child: u32, confidence: Option<f64>, side: &str| {
        stmt.execute(params![parent, child, confidence, side])
            .map_err(|e| match is_unique_violation(&e) {
                true => StageError::DuplicateLink { parent, child },
                false => StageError::from(DbError::from(e)),
            })
    };
    for child in &record.children {
        insert(record.id, child.id, child.confidence, "children")?;
        staged += 1;
    }
    if let Some(parent) = &record.parent {
        insert(parent.id, record.id, parent.confidence, "parent")?;
        staged += 1;
    }
    Ok(staged)
}

impl Db {
    /// Streams parsed records into the staging tables in batched transactions. `progress`
    /// receives the running staged-cell count. The first parse error or duplicate aborts,
    /// and every abort path clears staging first.
    pub fn stage_records(
        &self,
        records: impl Iterator<Item = Result<CellRecord, ParseError>>,
        progress: &dyn Fn(u64),
    ) -> Result<StagingReport, StageError> {
        let mut guard = self.conn()?;
        clear_staging_in(&guard)?;

        let mut report = StagingReport::default();
        let mut records = records.peekable();
        let staged = (|| -> Result<(), StageError> {
            while records.peek().is_some() {
                let tx = guard.transaction().map_err(DbError::from)?;
                {
                    let mut cell = tx
                        .prepare_cached(
                            "INSERT INTO staging_cells(id, t, z, y, x, seg_id, track_id,
                                                       detection_confidence, state, labels,
                                                       features)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        )
                        .map_err(DbError::from)?;
                    let mut link = tx
                        .prepare_cached(
                            "INSERT INTO staging_links(parent, child, confidence, side)
                             VALUES (?1, ?2, ?3, ?4)",
                        )
                        .map_err(DbError::from)?;
                    for _ in 0..STAGE_BATCH {
                        let Some(record) = records.next() else { break };
                        let record = record?;
                        stage_cell(&mut cell, &record)?;
                        report.links += stage_links(&mut link, &record)?;
                        report.cells += 1;
                        report.max_id = report.max_id.max(record.id);
                    }
                }
                tx.commit().map_err(DbError::from)?;
                progress(report.cells);
            }
            Ok(())
        })();
        if let Err(e) = staged {
            clear_staging_in(&guard)?;
            return Err(e);
        }
        Ok(report)
    }

    /// Set-level validation in SQL over the staged rows: referential integrity,
    /// `child.t > parent.t`, at most 2 children, at most 1 parent, `parent`/`children`
    /// agreement where both declare one link, confidences in [0,1], `track_id` within the
    /// renderable range, and — when `masks_present` — `COALESCE(seg_id, id)` resolving in
    /// `mask_labels` at the cell's frame. Returns the offenders; empty means valid.
    pub fn validate_staged(&self, masks_present: bool) -> Result<Vec<ImportError>, DbError> {
        let conn = self.conn()?;
        let mut errors = Vec::new();
        let mut collect = |sql: &str, message: fn(&rusqlite::Row<'_>) -> (i64, String)| {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query([OFFENDER_CAP])?;
            while let Some(row) = rows.next()? {
                let (cell_id, message) = message(row);
                errors.push(ImportError {
                    cell_id: u32::try_from(cell_id).unwrap_or(u32::MAX),
                    message,
                });
            }
            Ok::<(), DbError>(())
        };

        collect(
            "SELECT DISTINCT l.parent, l.child FROM staging_links l
              WHERE l.side = 'children'
                AND NOT EXISTS (SELECT 1 FROM staging_cells c WHERE c.id = l.child)
              LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!("child {} has no record", row.get_unwrap::<_, i64>(1)),
                )
            },
        )?;
        collect(
            "SELECT DISTINCT l.child, l.parent FROM staging_links l
              WHERE l.side = 'parent'
                AND NOT EXISTS (SELECT 1 FROM staging_cells c WHERE c.id = l.parent)
              LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!("parent {} has no record", row.get_unwrap::<_, i64>(1)),
                )
            },
        )?;
        collect(
            "SELECT DISTINCT l.child, l.parent, p.t, c.t FROM staging_links l
              JOIN staging_cells p ON p.id = l.parent
              JOIN staging_cells c ON c.id = l.child
             WHERE c.t <= p.t
             LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!(
                        "at t={} is not strictly after its parent {} at t={}",
                        row.get_unwrap::<_, i64>(3),
                        row.get_unwrap::<_, i64>(1),
                        row.get_unwrap::<_, i64>(2),
                    ),
                )
            },
        )?;
        collect(
            "SELECT parent, COUNT(DISTINCT child) FROM staging_links
             GROUP BY parent HAVING COUNT(DISTINCT child) > 2
             LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!("has {} children (at most 2)", row.get_unwrap::<_, i64>(1)),
                )
            },
        )?;
        collect(
            "SELECT child, COUNT(DISTINCT parent) FROM staging_links
             GROUP BY child HAVING COUNT(DISTINCT parent) > 1
             LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!("has {} parents (at most 1)", row.get_unwrap::<_, i64>(1)),
                )
            },
        )?;
        collect(
            "SELECT DISTINCT a.child, a.parent FROM staging_links a
              JOIN staging_links b ON b.parent = a.parent AND b.child = a.child
             WHERE a.side = 'children' AND b.side = 'parent'
               AND a.confidence IS NOT b.confidence
             LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!(
                        "its `parent` entry disagrees with cell {}'s `children` entry",
                        row.get_unwrap::<_, i64>(1),
                    ),
                )
            },
        )?;
        collect(
            "SELECT DISTINCT parent, child, confidence FROM staging_links
             WHERE confidence NOT BETWEEN 0 AND 1
             LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!(
                        "link to {} has confidence {} outside [0, 1]",
                        row.get_unwrap::<_, i64>(1),
                        row.get_unwrap::<_, f64>(2),
                    ),
                )
            },
        )?;
        collect(
            "SELECT id, detection_confidence FROM staging_cells
             WHERE detection_confidence NOT BETWEEN 0 AND 1
             LIMIT ?1",
            |row| {
                (
                    row.get_unwrap(0),
                    format!(
                        "detection confidence {} is outside [0, 1]",
                        row.get_unwrap::<_, f64>(1),
                    ),
                )
            },
        )?;
        collect(
            &format!(
                "SELECT id, track_id FROM staging_cells WHERE track_id > {MAX_LABEL_ID} LIMIT ?1"
            ),
            |row| {
                (
                    row.get_unwrap(0),
                    format!(
                        "track_id {} is past the renderable ceiling {}",
                        row.get_unwrap::<_, i64>(1),
                        MAX_LABEL_ID,
                    ),
                )
            },
        )?;
        if masks_present {
            collect(
                "SELECT s.id, s.t, COALESCE(s.seg_id, s.id) FROM staging_cells s
                 WHERE NOT EXISTS (SELECT 1 FROM mask_labels m
                                    WHERE m.t = s.t AND m.label = COALESCE(s.seg_id, s.id))
                 LIMIT ?1",
                |row| {
                    (
                        row.get_unwrap(0),
                        format!(
                            "mask value {} does not exist at t={}",
                            row.get_unwrap::<_, i64>(2),
                            row.get_unwrap::<_, i64>(1),
                        ),
                    )
                },
            )?;
        }
        Ok(errors)
    }

    /// Publishes staging into `cells`/`links` in one short transaction: cell rows the
    /// inventory already created are updated in place by id (centroid only where the file
    /// provides one), rows without a mask are inserted, links carry their confidence,
    /// `version.graph` bumps, and staging clears.
    pub fn materialize_staged(&self) -> Result<GraphSummary, DbError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        let count = |tx: &Transaction<'_>, sql: &str| -> Result<u64, DbError> {
            Ok(tx.query_row(sql, [], |row| row.get::<_, i64>(0))?.max(0) as u64)
        };
        let summary = GraphSummary {
            cells: count(&tx, "SELECT COUNT(*) FROM staging_cells")?,
            links: count(
                &tx,
                "SELECT COUNT(*) FROM (SELECT DISTINCT parent, child FROM staging_links)",
            )?,
            tracks: count(
                &tx,
                "SELECT COUNT(DISTINCT track_id) FROM staging_cells WHERE track_id IS NOT NULL",
            )?,
            divisions: count(
                &tx,
                "SELECT COUNT(*) FROM (SELECT parent FROM
                   (SELECT DISTINCT parent, child FROM staging_links)
                 GROUP BY parent HAVING COUNT(*) = 2)",
            )?,
        };
        // the file's cell id becomes both the project id and src_id: the converter
        // guarantees id == voxel value, so no remapping happens here.
        tx.execute(
            "INSERT INTO cells(id, t, z, y, x, detection_confidence, state, track_id,
                               src_id, seg_id, labels, features)
             SELECT id, t, z, y, x, detection_confidence, state, track_id,
                    id, seg_id, labels, features
               FROM staging_cells WHERE true
             ON CONFLICT(id) DO UPDATE SET
               t = excluded.t,
               z = COALESCE(excluded.z, cells.z),
               y = COALESCE(excluded.y, cells.y),
               x = COALESCE(excluded.x, cells.x),
               detection_confidence = excluded.detection_confidence,
               state = excluded.state,
               track_id = excluded.track_id,
               src_id = excluded.src_id,
               seg_id = excluded.seg_id,
               labels = excluded.labels,
               features = excluded.features",
            [],
        )?;
        // both declaring sides agree by validation, so DISTINCT collapses them to one row
        tx.execute(
            "INSERT INTO links(parent, child, confidence)
             SELECT DISTINCT parent, child, confidence FROM staging_links",
            [],
        )?;
        clear_staging_in(&tx)?;
        bump_in(&tx, VersionCounter::Graph)?;
        tx.commit()?;
        Ok(summary)
    }

    /// Discards whatever an aborted import staged.
    pub fn clear_staging(&self) -> Result<(), DbError> {
        let conn = self.conn()?;
        clear_staging_in(&conn)
    }

    /// Staged row counts, `(cells, links)`.
    pub fn staged_counts(&self) -> Result<(u64, u64), DbError> {
        let conn = self.conn()?;
        let count = |sql: &str| -> Result<u64, DbError> {
            Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?.max(0) as u64)
        };
        Ok((
            count("SELECT COUNT(*) FROM staging_cells")?,
            count("SELECT COUNT(*) FROM staging_links")?,
        ))
    }

    /// The v1 import policy: only an empty graph accepts an import. Returns the reason to
    /// refuse, or `None` when the import may proceed.
    pub fn import_blocker(&self) -> Result<Option<String>, DbError> {
        let conn = self.conn()?;
        let links: i64 = conn.query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))?;
        if links > 0 {
            return Ok(Some(format!(
                "the project already has a track graph ({links} links); importing over an \
                 existing graph is not supported"
            )));
        }
        let edits: i64 = conn.query_row(
            "SELECT COUNT(*) FROM edits WHERE domain = 'graph'",
            [],
            |row| row.get(0),
        )?;
        if edits > 0 {
            return Ok(Some(format!(
                "the project has graph edit history ({edits} edits); importing over an \
                 edited graph is not supported"
            )));
        }
        Ok(None)
    }
}
