use rusqlite::Connection;

use crate::project::DbError;

pub const SCHEMA_VERSION: u32 = 1;

const TABLES_V1: &str = r#"
CREATE TABLE cells (
  id INTEGER PRIMARY KEY,          -- global uint32 == voxel value (canonical)
  t INTEGER NOT NULL,
  z REAL, y REAL, x REAL,          -- centroid, pixel units, [z,y,x] order as in the JSON
  area INTEGER,
  detection_confidence REAL,
  state TEXT,                      -- NULL | 'normal' | 'dividing' | 'death'
  track_id INTEGER,                -- materialized, recomputed incrementally
  src_id INTEGER,                  -- cell id in the imported tracking file (round-trip)
  seg_id INTEGER,                  -- label value in the source mask at frame t (round-trip)
  labels TEXT,                     -- JSON array, user-defined tags (round-trip)
  features TEXT,                   -- JSON object, passthrough features (round-trip)
  reviewed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX cells_by_t ON cells(t);
CREATE INDEX cells_by_track ON cells(track_id);

CREATE TABLE mask_labels (         -- label values present per frame, written at mask import;
  t INTEGER NOT NULL,              -- tracking-import validation joins staging against this
  label INTEGER NOT NULL,
  PRIMARY KEY (t, label)
);

CREATE TABLE links (
  parent INTEGER NOT NULL REFERENCES cells(id),
  child  INTEGER NOT NULL REFERENCES cells(id),
  confidence REAL,
  reviewed INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (parent, child)
);
CREATE INDEX links_by_child ON links(child);

CREATE TABLE edits (                -- unified journal, both domains
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  domain TEXT NOT NULL,             -- 'graph' | 'mask'
  op TEXT NOT NULL,                 -- forward op (JSON)
  inverse TEXT NOT NULL,            -- inverse op (JSON; mask ops reference edit_blobs)
  undone INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE edit_blobs (           -- pre-edit chunk snapshots for mask undo
  seq INTEGER NOT NULL,
  chunk_key TEXT NOT NULL,
  before BLOB NOT NULL              -- zstd-compressed
);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
"#;

const SEED_META: &str = r#"
INSERT OR IGNORE INTO meta(key, value) VALUES
  ('version.image', '0'),
  ('version.labels', '0'),
  ('version.graph', '0'),
  ('version.settings', '0'),
  ('settings', '{}');
"#;

pub fn migrate(conn: &Connection) -> Result<(), DbError> {
    let found: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match found {
        0 => {}
        v if v == SCHEMA_VERSION => return Ok(()),
        found => {
            return Err(DbError::SchemaVersion {
                found,
                supported: SCHEMA_VERSION,
            });
        }
    }

    conn.execute_batch(&format!(
        "BEGIN;{}{}{}\nPRAGMA user_version = {SCHEMA_VERSION};\nCOMMIT;",
        TABLES_V1,
        crate::import::STAGING_TABLES,
        SEED_META,
    ))?;
    Ok(())
}
