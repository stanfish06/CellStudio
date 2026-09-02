//! Streaming `cellstudio-tracking` v1 export: one read transaction over `cells` + `links`
//! into a caller-supplied writer, so the server can gzip without materializing the document.

use std::io::Write;

use cellstudio_core::tracks::{CellRecord, LinkRef, TRACKING_FORMAT, TRACKING_VERSION};
use rusqlite::Statement;
use serde_json::json;

use crate::project::{Db, DbError};
use crate::queries::cell_row;

/// Rows between progress reports.
const PROGRESS_STRIDE: u64 = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub cells: u64,
    pub links: u64,
    /// `metadata.created`, RFC3339 UTC — also the source of the snapshot filename stamp.
    pub created: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("write: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<rusqlite::Error> for ExportError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(DbError::from(e))
    }
}

impl Db {
    /// Any `links` row exists. A snapshot at open/refetch time, like every version counter.
    pub fn has_graph(&self) -> Result<bool, DbError> {
        let conn = self.conn()?;
        Ok(
            conn.query_row("SELECT EXISTS(SELECT 1 FROM links)", [], |row| {
                row.get::<_, i64>(0)
            })? != 0,
        )
    }

    /// Streams the whole graph as `cellstudio-tracking` v1 JSON into `out` under one read
    /// transaction. Cells are ordered `(t, id)`, links are emitted on the `children` side
    /// only, `seg_id` is omitted when it equals the cell id, and `metadata` carries
    /// `created` plus `app_version`. `progress` receives fractions in [0, 1].
    pub fn export_graph(
        &self,
        app_version: &str,
        out: &mut dyn Write,
        progress: &dyn Fn(f32),
    ) -> Result<ExportSummary, ExportError> {
        let mut guard = self.conn()?;
        let tx = guard.transaction().map_err(DbError::from)?;

        let created: String =
            tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now')", [], |row| {
                row.get(0)
            })?;
        let total: i64 = tx.query_row("SELECT COUNT(*) FROM cells", [], |row| row.get(0))?;
        let total = total.max(1) as u64;

        write!(
            out,
            "{{\"format\":\"{TRACKING_FORMAT}\",\"version\":{TRACKING_VERSION},\"metadata\":"
        )?;
        let all = crate::project::label_definitions_in(&tx)?;
        let colors: serde_json::Map<String, serde_json::Value> = all
            .iter()
            .filter_map(|d| d.color.clone().map(|c| (d.name.clone(), json!(c))))
            .collect();
        let definitions: Vec<String> = all.into_iter().map(|d| d.name).collect();
        serde_json::to_writer(
            &mut *out,
            &json!({
                "created": created,
                "app_version": app_version,
                "label_definitions": definitions,
                "label_colors": colors,
            }),
        )?;
        out.write_all(b",\"cells\":[")?;

        let mut summary = ExportSummary {
            cells: 0,
            links: 0,
            created,
        };
        {
            let mut cells = tx.prepare(
                "SELECT id, t, z, y, x, area, detection_confidence, state, track_id,
                        src_id, seg_id, labels, features, reviewed,
                        (SELECT parent FROM links WHERE child = cells.id LIMIT 1), track_labels
                   FROM cells ORDER BY t, id",
            )?;
            let mut children =
                tx.prepare("SELECT child, confidence FROM links WHERE parent = ?1 ORDER BY child")?;
            let mut rows = cells.query([])?;
            while let Some(row) = rows.next()? {
                let record = record_of(cell_row(row)?, &mut children)?;
                summary.links += record.children.len() as u64;
                if summary.cells > 0 {
                    out.write_all(b",")?;
                }
                serde_json::to_writer(&mut *out, &record)?;
                summary.cells += 1;
                if summary.cells.is_multiple_of(PROGRESS_STRIDE) {
                    progress((summary.cells as f32 / total as f32).min(1.0));
                }
            }
        }
        out.write_all(b"]}")?;
        tx.commit().map_err(DbError::from)?;
        progress(1.0);
        Ok(summary)
    }
}

fn record_of(
    row: crate::queries::CellRow,
    children: &mut Statement<'_>,
) -> Result<CellRecord, ExportError> {
    let mut links = Vec::new();
    let mut found = children.query([row.id])?;
    while let Some(link) = found.next()? {
        let child: i64 = link.get(0)?;
        links.push(LinkRef {
            id: u32::try_from(child).map_err(|_| DbError::OutOfRange(child))?,
            confidence: link.get(1)?,
        });
    }
    Ok(CellRecord {
        id: row.id,
        t: row.t,
        // the converter's convention: an omitted seg_id resolves to the id itself
        seg_id: row.seg_id.filter(|seg| *seg != row.id),
        track_id: row.track_id,
        centroid: row.centroid,
        children: links,
        parent: None,
        confidence: row.detection_confidence,
        state: row.state,
        labels: row.labels,
        track_labels: row.track_labels,
        features: row.features,
    })
}
