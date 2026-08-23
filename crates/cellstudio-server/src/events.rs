use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use cellstudio_core::LayerId;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::error::{ApiError, ApiResult};
use crate::jobs::JobState;
use crate::state::AppState;
use crate::wire::VersionsWire;

const CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    Versions {
        versions: VersionsWire,
    },
    Job {
        job: JobState,
    },
    Invalidate {
        /// Versions are not comparable across projects, so a renderer on another session
        /// drops the event instead of invalidating its own caches (design M20).
        session_id: String,
        layer: LayerId,
        chunks: Vec<String>,
        version: u64,
    },
    GraphChanged {
        graph_version: u64,
        tracks: Vec<u32>,
    },
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<Event>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(CHANNEL_CAPACITY).0,
        }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(Arc::new(event));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.tx.subscribe()
    }

    pub fn subscribers(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
pub struct TicketQuery {
    ticket: String,
}

/// The socket authenticates with a one-time ticket
pub async fn events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TicketQuery>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !state.tickets.redeem(&query.ticket) {
        return Err(ApiError::Unauthorized);
    }
    let receiver = state.events.subscribe();
    let opening = state.current().map(|active| Event::Versions {
        versions: VersionsWire::new(&active.session_id, active.versions().unwrap_or_default()),
    });
    Ok(upgrade.on_upgrade(move |socket| pump(socket, receiver, opening)))
}

async fn pump(
    socket: WebSocket,
    mut receiver: broadcast::Receiver<Arc<Event>>,
    opening: Option<Event>,
) {
    let (mut sink, mut incoming) = socket.split();
    if let Some(event) = opening
        && send(&mut sink, &event).await.is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            frame = receiver.recv() => match frame {
                Ok(event) => {
                    if send(&mut sink, &event).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    tracing::warn!(dropped, "event subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Close(_))) | None => return,
                Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

async fn send<S>(sink: &mut S, event: &Event) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let text = match serde_json::to_string(event) {
        Ok(text) => text,
        Err(e) => {
            tracing::error!("event is not serializable: {e}");
            return Ok(());
        }
    };
    sink.send(Message::text(text)).await.map_err(|_| ())
}
