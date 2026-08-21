use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cellstudio_core::bricks::DEFAULT_CAPACITY_BYTES;
use cellstudio_core::dataset::{Dataset, LayoutReport, analyze_layout};
use cellstudio_core::reader::ImageReader;
use cellstudio_core::volume::DEFAULT_VOLUME_BUDGET_BYTES;
use cellstudio_db::queries::Versions;
use cellstudio_db::{DbError, Project};
use parking_lot::RwLock;
use tokio::sync::Semaphore;

use crate::auth::Tickets;
use crate::error::{ApiError, ApiResult};
use crate::events::EventBus;
use crate::jobs::Jobs;

pub const WORKING_COPY_NAME: &str = "bricks.zarr";

#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub host: IpAddr,
    pub port: u16,
    pub brick_cache_bytes: usize,
    pub decode_permits: usize,
    pub volume_budget_bytes: u64,
    pub build_proxy: bool,
}

impl Config {
    pub fn new(token: String) -> Self {
        Self {
            token,
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            brick_cache_bytes: DEFAULT_CAPACITY_BYTES,
            decode_permits: default_decode_permits(),
            volume_budget_bytes: DEFAULT_VOLUME_BUDGET_BYTES,
            build_proxy: true,
        }
    }
}

fn default_decode_permits() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 32)
}

/// One open project.
pub struct ActiveProject {
    pub session_id: String,
    pub image: Arc<ImageReader>,
    pub project: Arc<Project>,
    pub source: PathBuf,
    pub assembled_root: PathBuf,
    pub layout: LayoutReport,
}

impl ActiveProject {
    pub fn dataset(&self) -> &Dataset {
        self.image.dataset()
    }

    pub fn versions(&self) -> Result<Versions, DbError> {
        self.project.db.versions()
    }
}

/// Decode-pool occupancy.
#[derive(Debug, Default)]
pub struct ReadGauge {
    inflight: AtomicU64,
    peak: AtomicU64,
}

impl ReadGauge {
    pub fn enter(self: &Arc<Self>) -> ReadGuard {
        let inflight = self.inflight.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak.fetch_max(inflight, Ordering::AcqRel);
        ReadGuard(self.clone())
    }

    pub fn inflight(&self) -> u64 {
        self.inflight.load(Ordering::Acquire)
    }

    pub fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }
}

pub struct ReadGuard(Arc<ReadGauge>);

impl Drop for ReadGuard {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct AppState {
    pub config: Config,
    active: RwLock<Option<Arc<ActiveProject>>>,
    pub jobs: Arc<Jobs>,
    pub events: EventBus,
    pub tickets: Tickets,
    pub decode: Arc<Semaphore>,
    pub reads: Arc<ReadGauge>,
}

impl AppState {
    pub fn new(config: Config) -> Arc<Self> {
        let events = EventBus::new();
        let permits = config.decode_permits.max(2);
        Arc::new(Self {
            active: RwLock::new(None),
            jobs: Jobs::new(events.clone()),
            events,
            tickets: Tickets::new(),
            decode: Arc::new(Semaphore::new(permits)),
            reads: Arc::new(ReadGauge::default()),
            config,
        })
    }

    pub fn current(&self) -> Option<Arc<ActiveProject>> {
        self.active.read().clone()
    }

    pub fn require(&self) -> ApiResult<Arc<ActiveProject>> {
        self.current().ok_or(ApiError::NoProject)
    }

    pub fn session(&self) -> Option<String> {
        self.active.read().as_ref().map(|a| a.session_id.clone())
    }

    pub fn publish(&self, active: Arc<ActiveProject>) {
        let session = active.session_id.clone();
        *self.active.write() = Some(active);
        self.jobs.cancel_other_sessions(&session);
    }

    pub fn is_current(&self, session: &str) -> bool {
        self.active
            .read()
            .as_ref()
            .is_some_and(|a| a.session_id == session)
    }

    pub fn take(&self) -> Option<Arc<ActiveProject>> {
        self.active.write().take()
    }

    pub async fn io<T, F>(&self, body: F) -> ApiResult<T>
    where
        F: FnOnce() -> ApiResult<T> + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(body).await?
    }

    pub async fn read<T, F>(&self, body: F) -> ApiResult<T>
    where
        F: FnOnce() -> ApiResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .decode
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ApiError::Internal("decode pool closed".to_owned()))?;
        let gauge = self.reads.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _entered = gauge.enter();
            body()
        })
        .await?
    }
}

pub fn assembled_layer(project: &Project, source: &Path) -> ApiResult<(PathBuf, Dataset)> {
    let working = project.cache_dir().join(WORKING_COPY_NAME);
    if working.exists() {
        match cellstudio_core::open(&working) {
            Ok(dataset) => return Ok((working, dataset)),
            Err(e) => tracing::warn!("ignoring unreadable working copy at {working:?}: {e}"),
        }
    }
    Ok((source.to_path_buf(), cellstudio_core::open(source)?))
}

pub fn layout_of(dataset: &Dataset) -> ApiResult<LayoutReport> {
    Ok(analyze_layout(dataset.level(0)?, dataset.dtype))
}
