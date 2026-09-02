use rusqlite::Connection;

use crate::project::DbError;

pub const SCHEMA_VERSION: u32 = 3;

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

/// v1 → v2, and the tail of a fresh create, so both paths land on one schema.
const TABLES_V2: &str = r#"
CREATE TABLE mask_extent (         -- per (t, label): conservative bbox, exact area and sums
  t INTEGER NOT NULL,
  label INTEGER NOT NULL,
  z0 INTEGER, z1 INTEGER, y0 INTEGER, y1 INTEGER, x0 INTEGER, x1 INTEGER,
  area INTEGER NOT NULL,
  sum_z REAL NOT NULL, sum_y REAL NOT NULL, sum_x REAL NOT NULL,
  PRIMARY KEY (t, label)
);

ALTER TABLE edits ADD COLUMN pending INTEGER NOT NULL DEFAULT 0;

-- rebuild: `before` becomes nullable beside `existed`, because the inverse of a chunk that
-- was absent is an erase, not a write of encoded zeros
CREATE TABLE edit_blobs_v2 (
  seq INTEGER NOT NULL,
  chunk_key TEXT NOT NULL,
  existed INTEGER NOT NULL,        -- 0: no object at this key before the edit
  before BLOB                      -- zstd-compressed; NULL exactly when existed = 0
);
INSERT INTO edit_blobs_v2(seq, chunk_key, existed, before)
  SELECT seq, chunk_key, 1, before FROM edit_blobs;
DROP TABLE edit_blobs;
ALTER TABLE edit_blobs_v2 RENAME TO edit_blobs;
CREATE INDEX edit_blobs_by_seq ON edit_blobs(seq);
"#;

/// v2 → v3: track-scope labels live per cell, like cell-scope ones.
const TABLES_V3: &str = r#"
ALTER TABLE cells ADD COLUMN track_labels TEXT;   -- JSON array, NULL when empty
ALTER TABLE staging_cells ADD COLUMN track_labels TEXT;
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
    let statements = match found {
        0 => format!(
            "{TABLES_V1}{}{SEED_META}{TABLES_V2}{TABLES_V3}",
            crate::import::STAGING_TABLES
        ),
        // a project already at 1 keeps its rows: v2 only adds tables and columns
        1 => format!("{TABLES_V2}{TABLES_V3}"),
        2 => TABLES_V3.to_string(),
        v if v == SCHEMA_VERSION => return Ok(()),
        found => {
            return Err(DbError::SchemaVersion {
                found,
                supported: SCHEMA_VERSION,
            });
        }
    };

    conn.execute_batch(&format!(
        "BEGIN;{statements}\nPRAGMA user_version = {SCHEMA_VERSION};\nCOMMIT;"
    ))?;
    Ok(())
}
