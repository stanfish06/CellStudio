use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cellstudio_core::axes::PhysicalScale;
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::queries::VersionCounter;
use crate::schema;

const PROJECT_SUFFIX: &str = ".cellstudio";
const META_FILE: &str = "project.json";
const DB_FILE: &str = "tracks.sqlite";
const CACHE_DIR: &str = "cache";
const LABELS_STORE: &str = "labels.zarr";

const PROJECT_FORMAT: &str = "cellstudio-project";
const PROJECT_FORMAT_VERSION: u32 = 1;

/// `meta` keys for project state.
const SETTINGS_KEY: &str = "settings";
const VOXEL_OVERRIDE_KEY: &str = "voxel_size_override";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub source: PathBuf,
}

/// On-disk shape of `project.json`.
#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    meta: ProjectMeta,
}

#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub meta: ProjectMeta,
    pub db: Db,
}

impl Project {
    /// Creates or reopens `<dataset>.cellstudio/` next to `dataset`.
    pub fn create_or_open(dataset: &Path) -> Result<Self, OpenError> {
        let source = std::path::absolute(dataset).map_err(|source| OpenError::Io {
            path: dataset.to_path_buf(),
            source,
        })?;
        let root = project_root(&source)?;
        let existed = root.exists();

        Self::open_container(&root, &source).inspect_err(|_| {
            // roll back only a container this call created; an existing project survives a
            // failed open untouched
            if !existed {
                let _ = std::fs::remove_dir_all(&root);
            }
        })
    }

    fn open_container(root: &Path, source: &Path) -> Result<Self, OpenError> {
        let cache = root.join(CACHE_DIR);
        std::fs::create_dir_all(&cache).map_err(|source| OpenError::Io {
            path: cache,
            source,
        })?;

        let db = Db::open(&root.join(DB_FILE))?;
        let meta = load_or_write_meta(root, source)?;
        Ok(Self {
            root: root.to_path_buf(),
            meta,
            db,
        })
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(CACHE_DIR)
    }

    pub fn labels_store_path(&self) -> PathBuf {
        self.root.join(LABELS_STORE)
    }

    pub fn has_labels(&self) -> bool {
        self.labels_store_path().exists()
    }
}

fn project_root(source: &Path) -> Result<PathBuf, OpenError> {
    let name = match source.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("zarr") => source.file_stem(),
        _ => source.file_name(),
    };
    let mut dir = name
        .ok_or_else(|| OpenError::DatasetPath(source.to_path_buf()))?
        .to_os_string();
    dir.push(PROJECT_SUFFIX);
    Ok(source.with_file_name(dir))
}

fn load_or_write_meta(root: &Path, source: &Path) -> Result<ProjectMeta, OpenError> {
    let path = root.join(META_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let file: ProjectFile =
                serde_json::from_slice(&bytes).map_err(|source| OpenError::Meta {
                    path: path.clone(),
                    source,
                })?;
            if file.format != PROJECT_FORMAT || file.version != PROJECT_FORMAT_VERSION {
                return Err(OpenError::Format {
                    path,
                    format: file.format,
                    version: file.version,
                });
            }
            let mut meta = file.meta;
            // the dataset may have moved since the last open; record where it is now
            if meta.source != source {
                meta.source = source.to_path_buf();
                write_meta(&path, &meta)?;
            }
            Ok(meta)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let meta = ProjectMeta {
                source: source.to_path_buf(),
            };
            write_meta(&path, &meta)?;
            Ok(meta)
        }
        Err(source) => Err(OpenError::Io { path, source }),
    }
}

fn write_meta(path: &Path, meta: &ProjectMeta) -> Result<(), OpenError> {
    let file = ProjectFile {
        format: PROJECT_FORMAT.to_string(),
        version: PROJECT_FORMAT_VERSION,
        meta: meta.clone(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(|source| OpenError::Meta {
        path: path.to_path_buf(),
        source,
    })?;
    // write then rename so an interrupted write never leaves a truncated project.json
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|source| OpenError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| OpenError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// `tracks.sqlite`
#[derive(Debug)]
pub struct Db {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl Db {
    /// Opens the database exclusively and migrates it. Fails with
    /// [`DbError::AlreadyOpen`] when another connection holds it.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        Self::open_inner(path).map_err(|e| e.into_open_conflict(path))
    }

    fn open_inner(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::ZERO)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        let journal: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !journal.eq_ignore_ascii_case("wal") {
            return Err(DbError::JournalMode(journal));
        }
        let locking: String =
            conn.query_row("PRAGMA locking_mode = EXCLUSIVE", [], |row| row.get(0))?;
        if !locking.eq_ignore_ascii_case("exclusive") {
            return Err(DbError::LockingMode(locking));
        }

        let db = Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        };
        db.migrate()?;
        db.conn()?.execute(
            "UPDATE meta SET value = value WHERE key = 'version.image'",
            [],
        )?;
        Ok(db)
    }

    pub fn migrate(&self) -> Result<(), DbError> {
        let conn = self.conn()?;
        schema::migrate(&conn)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn settings(&self) -> Result<Value, DbError> {
        Ok(self
            .meta_json(SETTINGS_KEY)?
            .unwrap_or_else(|| Value::Object(serde_json::Map::new())))
    }

    pub fn put_settings(&self, settings: &Value) -> Result<u64, DbError> {
        self.put_meta_json(SETTINGS_KEY, settings)
    }

    /// Voxel size set bg user
    pub fn voxel_size_override(&self) -> Result<Option<PhysicalScale>, DbError> {
        match self.meta_json(VOXEL_OVERRIDE_KEY)? {
            None | Some(Value::Null) => Ok(None),
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
        }
    }

    pub fn put_voxel_size_override(&self, scale: Option<PhysicalScale>) -> Result<u64, DbError> {
        self.put_meta_json(VOXEL_OVERRIDE_KEY, &serde_json::to_value(scale)?)
    }

    fn meta_json(&self, key: &str) -> Result<Option<Value>, DbError> {
        let conn = self.conn()?;
        let raw: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        match raw.as_deref() {
            None | Some("") => Ok(None),
            Some(text) => Ok(Some(serde_json::from_str(text)?)),
        }
    }

    fn put_meta_json(&self, key: &str, value: &Value) -> Result<u64, DbError> {
        let text = serde_json::to_string(value)?;
        let mut guard = self.conn()?;
        let tx = guard.transaction()?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, text],
        )?;
        let version = crate::queries::bump_in(&tx, VersionCounter::Settings)?;
        tx.commit()?;
        Ok(version)
    }

    pub(crate) fn conn(&self) -> Result<MutexGuard<'_, Connection>, DbError> {
        self.conn.lock().map_err(|_| DbError::Poisoned)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("dataset path {0} has no final component to name a project after")]
    DatasetPath(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not readable project metadata: {source}")]
    Meta {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "{path} declares format {format:?} version {version}; \
         this build reads {PROJECT_FORMAT:?} version {PROJECT_FORMAT_VERSION}"
    )]
    Format {
        path: PathBuf,
        format: String,
        version: u32,
    },
    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} is already open: the backend owns the project database exclusively")]
    AlreadyOpen(PathBuf),
    #[error("could not enable WAL: journal mode is {0:?}")]
    JournalMode(String),
    #[error("could not take the database exclusively: locking mode is {0:?}")]
    LockingMode(String),
    #[error("database schema version {found} is not supported (this build reads {supported})")]
    SchemaVersion { found: u32, supported: u32 },
    #[error("database mutex poisoned by an earlier panic")]
    Poisoned,
    #[error("cell {0} is not in this project")]
    UnknownCell(u32),
    #[error("stored integer {0} is out of range for its column")]
    OutOfRange(i64),
    #[error("stored cell state {0:?} is not a known state")]
    UnknownState(String),
    #[error("stored edit domain {0:?} is not a known domain")]
    UnknownDomain(String),
    #[error(
        "label {label} is the cell on frame {existing}; a mask edit at t={requested} would \
         move the cell and orphan its voxels — one id, one frame"
    )]
    LabelFrameConflict {
        label: u32,
        existing: u64,
        requested: u64,
    },
    #[error(
        "label {label} at t={t} would fall to {area} voxels: its extent was not seeded before \
         the edit"
    )]
    ExtentUnderflow { t: u64, label: u32, area: i64 },
    #[error("label ids {first}..={last} run past the {max} the label overlay can distinguish", max = crate::queries::MAX_LABEL_ID)]
    LabelIdsExhausted { first: i64, last: i64 },
    #[error("label id {0} is already in use")]
    LabelIdTaken(u32),
    #[error("track ids are exhausted: the next id would pass the {max} the label overlay can distinguish", max = crate::queries::MAX_LABEL_ID)]
    TrackIdsExhausted,
    #[error(
        "the adopted label store has not been inventoried yet; label-id reservation and mask \
         writes are refused until the inventory job completes"
    )]
    InventoryPending,
}

impl DbError {
    fn into_open_conflict(self, path: &Path) -> Self {
        match &self {
            DbError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if matches!(e.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) =>
            {
                DbError::AlreadyOpen(path.to_path_buf())
            }
            _ => self,
        }
    }
}
