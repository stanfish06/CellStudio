//! One-time inventory of an adopted label store A store the app did not write
//! meets an empty database, so `mask_labels`, `mask_extent`, missing `cells` rows, and the
//! id-reservation floor are seeded from a full level-0 scan — and until the completeness
//! marker commits, label-id reservation and mask writes are refused.

use std::path::Path;
use std::time::UNIX_EPOCH;

use cellstudio_core::labels::ExtentRow as ScanRow;
use rusqlite::{Connection, OptionalExtension, params};

use crate::project::{Db, DbError};
use crate::queries::NEXT_ID_KEY;

/// `meta` key holding the identity of the store whose inventory is complete.
pub const MARKER_KEY: &str = "inventory.complete";
/// `meta` key holding the identity of the store an open found awaiting inventory.
pub const REQUIRED_KEY: &str = "inventory.required";

/// The store's identity: its path plus the root metadata file's mtime in nanoseconds.
/// Chunk writes never touch the root metadata, while both the app's create path and the
/// conversion script write a fresh root file before renaming the store into place — so an
/// edited store keeps its identity and a replaced one presents a new one.
pub fn store_identity(root: &Path) -> Option<String> {
    let meta = ["zarr.json", ".zgroup", ".zattrs"]
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())?;
    let nanos = std::fs::metadata(&meta)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}|{nanos}", root.display()))
}

fn meta_in(conn: &Connection, key: &str) -> Result<Option<String>, DbError> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

fn upsert_in(conn: &Connection, key: &str, value: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// The gate: an inventory is owed when an open recorded a store identity no completed
/// inventory covers. Checked inside `reserve_label_ids` and by the mask-write coordinator.
pub(crate) fn pending_in(conn: &Connection) -> Result<bool, DbError> {
    match meta_in(conn, REQUIRED_KEY)? {
        Some(required) => Ok(meta_in(conn, MARKER_KEY)?.as_deref() != Some(required.as_str())),
        None => Ok(false),
    }
}

impl Db {
    pub fn inventory_marker(&self) -> Result<Option<String>, DbError> {
        let conn = self.conn()?;
        meta_in(&conn, MARKER_KEY)
    }

    /// Records the store an open found, closing the gate until [`Db::publish_inventory`]
    /// commits the matching marker.
    pub fn require_inventory(&self, identity: &str) -> Result<(), DbError> {
        let conn = self.conn()?;
        upsert_in(&conn, REQUIRED_KEY, identity)
    }

    /// Drops a stale requirement — the store it named no longer exists.
    pub fn clear_inventory_requirement(&self) -> Result<(), DbError> {
        self.conn()?
            .execute("DELETE FROM meta WHERE key = ?1", [REQUIRED_KEY])?;
        Ok(())
    }

    /// Marks a store complete without a scan — the app created it empty itself.
    pub fn set_inventory_marker(&self, identity: &str) -> Result<(), DbError> {
        let conn = self.conn()?;
        upsert_in(&conn, MARKER_KEY, identity)
    }

    pub fn inventory_pending(&self) -> Result<bool, DbError> {
        let conn = self.conn()?;
        pending_in(&conn)
    }

    /// Publishes one finished scan in a single transaction: `mask_labels` and `mask_extent`
    /// rewritten whole (an earlier partial run leaves only rows this rewrite owns), missing
    /// `cells` rows with tracking fields null, the reservation floor moved past `max_id`,
    /// the mask journal cleared (its snapshots hold bytes of whatever store preceded this
    /// inventory), and the completeness marker for `identity`.
    pub fn publish_inventory(
        &self,
        rows: &[ScanRow],
        max_id: u32,
        identity: &str,
    ) -> Result<(), DbError> {
        let edge = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        tx.execute("DELETE FROM mask_labels", [])?;
        tx.execute("DELETE FROM mask_extent", [])?;
        {
            let mut label = tx.prepare("INSERT INTO mask_labels(t, label) VALUES (?1, ?2)")?;
            let mut extent = tx.prepare(
                "INSERT INTO mask_extent(t, label, z0, z1, y0, y1, x0, x1, area,
                                         sum_z, sum_y, sum_x)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            let mut cell = tx.prepare(
                "INSERT INTO cells(id, t, z, y, x, area) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO NOTHING",
            )?;
            for row in rows {
                let t = edge(row.t);
                label.execute(params![t, row.label])?;
                let b = row.bbox;
                extent.execute(params![
                    t,
                    row.label,
                    b.map(|b| edge(b.z0)),
                    b.map(|b| edge(b.z1)),
                    b.map(|b| edge(b.y0)),
                    b.map(|b| edge(b.y1)),
                    b.map(|b| edge(b.x0)),
                    b.map(|b| edge(b.x1)),
                    edge(row.area),
                    row.sum_z,
                    row.sum_y,
                    row.sum_x,
                ])?;
                if row.area > 0 {
                    let n = row.area as f64;
                    cell.execute(params![
                        row.label,
                        t,
                        row.sum_z / n,
                        row.sum_y / n,
                        row.sum_x / n,
                        edge(row.area),
                    ])?;
                }
            }
        }
        // reservation starts above every id the store holds, never below a floor already set
        let stored: Option<i64> = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
                [NEXT_ID_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let floor = i64::from(max_id) + 1;
        if stored.unwrap_or(0) < floor {
            upsert_in(&tx, NEXT_ID_KEY, &floor.to_string())?;
        }
        // mask journal rows snapshot the pre-inventory store's chunk bytes; undoing them
        // onto this store would write another store's data
        tx.execute(
            "DELETE FROM edit_blobs WHERE seq IN (SELECT seq FROM edits WHERE domain = 'mask')",
            [],
        )?;
        tx.execute("DELETE FROM edits WHERE domain = 'mask'", [])?;
        upsert_in(&tx, MARKER_KEY, identity)?;
        tx.commit()?;
        Ok(())
    }
}
