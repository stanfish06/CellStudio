use cellstudio_core::tracks::CellRecord;

use crate::project::{Db, DbError};

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
    pub tracks: u32,
    pub divisions: u64,
}

impl Db {
    /// Streams parsed records into the staging tables. `progress` receives the running
    /// staged-cell count.
    pub fn stage_records(
        &self,
        records: impl Iterator<Item = CellRecord>,
        progress: &dyn Fn(u64),
    ) -> Result<StagingReport, DbError> {
        let _ = (records, progress);
        todo!("tracking-import phase (design Migration Plan step 2)")
    }

    /// Set-level validation in SQL: referential integrity, `child.t > parent.t`, at most 2
    /// children, at most 1 parent, `parent`/`children` agreement, confidences in [0,1], and
    /// `seg_id` present in `mask_labels` at the cell's frame when masks are loaded.
    pub fn validate_staged(&self) -> Result<(), Vec<ImportError>> {
        todo!("tracking-import phase (design Migration Plan step 2)")
    }

    /// Publishes staging into `cells`/`links` in one transaction, assigning project ids and
    /// track ids (maximal unbranched paths), then clears staging.
    pub fn materialize_staged(&self) -> Result<GraphSummary, DbError> {
        todo!("tracking-import phase (design Migration Plan step 2)")
    }
}
