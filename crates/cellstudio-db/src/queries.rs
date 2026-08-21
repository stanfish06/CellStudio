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
        const SQL: &str =
            "SELECT seq, ts, domain, op, undone FROM edits ORDER BY seq DESC LIMIT ?1";
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
