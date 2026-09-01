pub mod graph;
pub mod io;
pub mod mask;
pub mod pixels;
pub mod project;
pub mod store;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Router, middleware};

use crate::auth;
use crate::events;
use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(project::health))
        .route("/project", get(project::get_project))
        .route("/project/open", post(project::open_project))
        .route(
            "/settings",
            get(project::get_settings).put(project::put_settings),
        )
        .route("/store/{*path}", get(store::get_object))
        .route("/slice", get(pixels::slice))
        .route("/volume", get(pixels::volume))
        .route("/pixel", get(pixels::pixel))
        .route("/histogram", get(pixels::histogram))
        .route("/cells", get(graph::cells))
        .route("/lineage", get(graph::lineage))
        .route("/edits", get(graph::edits))
        .route("/edits/undo", post(graph::undo))
        .route("/edits/redo", post(graph::redo))
        .route("/graph/link", post(graph::link))
        .route("/graph/unlink", post(graph::unlink))
        .route("/graph/cut", post(graph::cut))
        .route("/mask/reserve", post(mask::reserve))
        .route("/mask/stroke", post(mask::stroke))
        .route("/mask/delete", post(mask::delete))
        .route("/jobs", get(io::list_jobs))
        .route("/import/{kind}", post(io::start_import))
        .route("/export/tracks", post(io::start_export))
        .route("/rechunk", post(io::start_rechunk))
        .route("/ws-ticket", get(auth::ws_ticket))
        .route("/events", get(events::events))
        // innermost first: the session stamp sees the handler's response, the token check
        // runs before any handler, and CORS answers preflights before either
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::stamp_session,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .layer(auth::cors())
        .with_state(state)
}
