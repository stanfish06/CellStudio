pub mod auth;
pub mod error;
pub mod events;
pub mod jobs;
pub mod routes;
pub mod state;
pub mod wire;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

pub use state::{AppState, Config};

pub struct Bound {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    listener: TcpListener,
}

pub async fn bind(config: Config) -> std::io::Result<Bound> {
    let state = AppState::new(config);
    let listener = TcpListener::bind(SocketAddr::new(state.config.host, state.config.port)).await?;
    let addr = listener.local_addr()?;
    Ok(Bound {
        addr,
        state,
        listener,
    })
}

impl Bound {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub async fn serve<F>(self, shutdown: F) -> std::io::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let state = self.state.clone();
        let app = routes::router(self.state);
        let result = axum::serve(self.listener, app)
            .with_graceful_shutdown(shutdown)
            .await;
        if let Some(active) = state.take() {
            tracing::info!(session = %active.session_id, "closing project");
        }
        result
    }
}

pub async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => tracing::error!("cannot listen for SIGTERM: {e}"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown requested");
}
