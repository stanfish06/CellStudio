use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

use crate::events::{Event, EventBus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobKind {
    Rechunk,
    Proxy,
    Inventory,
    ImportTracks,
    ImportLabels,
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn terminal(self) -> bool {
        !matches!(self, JobStatus::Running)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobState {
    pub id: String,
    pub kind: JobKind,
    pub progress: f32,
    pub status: JobStatus,
    pub message: Option<String>,
}

struct Entry {
    state: JobState,
    session: String,
    order: u64,
    cancel: Arc<AtomicBool>,
}

pub struct Jobs {
    entries: Mutex<HashMap<String, Entry>>,
    next_order: AtomicU64,
    events: EventBus,
}

impl Jobs {
    pub fn new(events: EventBus) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            next_order: AtomicU64::new(0),
            events,
        })
    }

    pub fn start(self: &Arc<Self>, session: &str, kind: JobKind) -> JobHandle {
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let state = JobState {
            id: id.clone(),
            kind,
            progress: 0.0,
            status: JobStatus::Running,
            message: None,
        };
        self.entries.lock().insert(
            id.clone(),
            Entry {
                state: state.clone(),
                session: session.to_owned(),
                order: self.next_order.fetch_add(1, Ordering::Relaxed),
                cancel: cancel.clone(),
            },
        );
        self.events.publish(Event::Job { job: state });
        JobHandle {
            id,
            jobs: self.clone(),
            cancel,
            last_percent: Mutex::new(u8::MAX),
        }
    }

    /// Creation order, so a client renders jobs in the sequence they were scheduled.
    pub fn list(&self) -> Vec<JobState> {
        let entries = self.entries.lock();
        let mut ordered: Vec<(u64, JobState)> = entries
            .values()
            .map(|e| (e.order, e.state.clone()))
            .collect();
        ordered.sort_by_key(|(order, _)| *order);
        ordered.into_iter().map(|(_, state)| state).collect()
    }

    pub fn get(&self, id: &str) -> Option<JobState> {
        self.entries.lock().get(id).map(|e| e.state.clone())
    }

    /// Marks every running job outside `session` cancelled and raises their cancel flags.
    /// Work already inside a blocking call cannot be interrupted, so the flag is what the
    /// job body checks before it publishes anything.
    pub fn cancel_other_sessions(&self, session: &str) -> Vec<JobState> {
        let mut cancelled = Vec::new();
        let mut entries = self.entries.lock();
        for entry in entries.values_mut() {
            if entry.session == session || entry.state.status.terminal() {
                continue;
            }
            entry.cancel.store(true, Ordering::SeqCst);
            entry.state.status = JobStatus::Cancelled;
            entry.state.message = Some("superseded by a new session".to_owned());
            cancelled.push(entry.state.clone());
        }
        drop(entries);
        for state in &cancelled {
            self.events.publish(Event::Job { job: state.clone() });
        }
        cancelled
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut JobState)) -> Option<JobState> {
        let mut entries = self.entries.lock();
        let entry = entries.get_mut(id)?;
        // a cancelled job never reports progress or completion again
        if entry.state.status == JobStatus::Cancelled {
            return None;
        }
        f(&mut entry.state);
        Some(entry.state.clone())
    }
}

/// The job body's handle: progress, completion, and the cancel flag.
pub struct JobHandle {
    pub id: String,
    jobs: Arc<Jobs>,
    cancel: Arc<AtomicBool>,
    last_percent: Mutex<u8>,
}

impl JobHandle {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Records progress; emits an event only when the whole-percent figure moves, so a
    /// per-chunk callback cannot flood the socket.
    pub fn progress(&self, fraction: f32) {
        let clamped = fraction.clamp(0.0, 1.0);
        let percent = (clamped * 100.0) as u8;
        {
            let mut last = self.last_percent.lock();
            if *last == percent {
                return;
            }
            *last = percent;
        }
        if let Some(state) = self.jobs.update(&self.id, |s| s.progress = clamped) {
            self.jobs.events.publish(Event::Job { job: state });
        }
    }

    pub fn finish(&self, message: Option<String>) {
        self.settle(JobStatus::Done, 1.0, message);
    }

    pub fn fail(&self, message: impl Into<String>) {
        self.settle(JobStatus::Failed, 0.0, Some(message.into()));
    }

    pub fn cancel(&self, message: impl Into<String>) {
        self.cancel.store(true, Ordering::SeqCst);
        let state = {
            let mut entries = self.jobs.entries.lock();
            match entries.get_mut(&self.id) {
                Some(entry) => {
                    entry.state.status = JobStatus::Cancelled;
                    entry.state.message = Some(message.into());
                    Some(entry.state.clone())
                }
                None => None,
            }
        };
        if let Some(state) = state {
            self.jobs.events.publish(Event::Job { job: state });
        }
    }

    fn settle(&self, status: JobStatus, progress: f32, message: Option<String>) {
        let updated = self.jobs.update(&self.id, |s| {
            s.status = status;
            s.progress = if status == JobStatus::Done {
                1.0
            } else {
                s.progress.max(progress)
            };
            s.message = message;
        });
        if let Some(state) = updated {
            self.jobs.events.publish(Event::Job { job: state });
        }
    }
}
