use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::header::{
    ACCEPT_RANGES, AUTHORIZATION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ORIGIN, RANGE,
};
use axum::http::{HeaderName, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use axum::{Json, extract::rejection::JsonRejection};
use parking_lot::Mutex;
use rand::RngExt;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::wire::{
    DTYPE_HEADER, LEVEL_HEADER, SESSION_HEADER, SHAPE_HEADER, Ticket, VOLUME_SOURCE_HEADER,
};

const TICKET_TTL: Duration = Duration::from_secs(30);

const WS_PATH: &str = "/events";

pub struct Tickets {
    live: Mutex<Vec<(String, Instant)>>,
}

impl Tickets {
    pub fn new() -> Self {
        Self {
            live: Mutex::new(Vec::new()),
        }
    }

    pub fn issue(&self) -> String {
        let ticket = random_hex(32);
        let mut live = self.live.lock();
        let now = Instant::now();
        live.retain(|(_, issued)| now.duration_since(*issued) < TICKET_TTL);
        live.push((ticket.clone(), now));
        ticket
    }

    pub fn redeem(&self, ticket: &str) -> bool {
        let mut live = self.live.lock();
        let now = Instant::now();
        live.retain(|(_, issued)| now.duration_since(*issued) < TICKET_TTL);
        match live
            .iter()
            .position(|(value, _)| constant_time_eq(value, ticket))
        {
            Some(index) => {
                live.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn live_count(&self) -> usize {
        self.live.lock().len()
    }
}

impl Default for Tickets {
    fn default() -> Self {
        Self::new()
    }
}

pub fn random_hex(bytes: usize) -> String {
    let mut rng = rand::rng();
    (0..bytes)
        .map(|_| format!("{:02x}", rng.random::<u8>()))
        .collect()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub async fn ws_ticket(State(state): State<Arc<AppState>>) -> Json<Ticket> {
    Json(Ticket {
        ticket: state.tickets.issue(),
    })
}

pub async fn authenticate(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> ApiResult<Response> {
    if let Some(origin) = request.headers().get(ORIGIN) {
        let origin = origin.to_str().unwrap_or("");
        if !origin_allowed(origin) {
            return Err(ApiError::Origin(origin.to_owned()));
        }
    }
    if request.method() == Method::OPTIONS || request.uri().path() == WS_PATH {
        return Ok(next.run(request).await);
    }
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !constant_time_eq(presented, &state.config.token) {
        return Err(ApiError::Unauthorized);
    }
    Ok(next.run(request).await)
}

pub async fn stamp_session(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    if response.headers().contains_key(SESSION_HEADER) {
        return response;
    }
    if let Some(session) = state.session()
        && let Ok(value) = HeaderValue::from_str(&session)
    {
        response
            .headers_mut()
            .insert(HeaderName::from_static(SESSION_HEADER), value);
    }
    response
}

pub fn origin_allowed(origin: &str) -> bool {
    let origin = origin.trim();
    if origin.is_empty() || origin == "null" {
        return true;
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    match scheme {
        "file" => true,
        "http" | "https" => is_loopback_host(rest),
        _ => false,
    }
}

fn is_loopback_host(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

pub fn cors() -> CorsLayer {
    let exposed = [
        HeaderName::from_static(SESSION_HEADER),
        HeaderName::from_static(SHAPE_HEADER),
        HeaderName::from_static(DTYPE_HEADER),
        HeaderName::from_static(LEVEL_HEADER),
        HeaderName::from_static(VOLUME_SOURCE_HEADER),
        CONTENT_LENGTH,
        CONTENT_RANGE,
        ACCEPT_RANGES,
    ];
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            origin_allowed(origin.to_str().unwrap_or(""))
        }))
        .allow_methods([
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::OPTIONS,
        ])
        // the session header rides on every mutation, which makes it non-simple: without it
        // here the browser blocks the request at the preflight
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            RANGE,
            HeaderName::from_static(SESSION_HEADER),
        ])
        .expose_headers(exposed)
        .max_age(Duration::from_secs(600))
}

pub fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    match body {
        Ok(Json(value)) => Ok(value),
        Err(rejection) => Err(ApiError::BadRequest(rejection.body_text())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_file_origins_are_allowed_and_others_are_not() {
        for allowed in [
            "",
            "null",
            "file://",
            "http://localhost:5173",
            "http://127.0.0.1:1234",
            "https://localhost",
            "http://[::1]:5173",
        ] {
            assert!(origin_allowed(allowed), "{allowed} should be allowed");
        }
        for denied in [
            "http://evil.example",
            "https://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "ws://localhost:5173",
            "localhost:5173",
        ] {
            assert!(!origin_allowed(denied), "{denied} should be denied");
        }
    }

    #[test]
    fn a_ticket_is_redeemable_once() {
        let tickets = Tickets::new();
        let ticket = tickets.issue();
        assert!(tickets.redeem(&ticket));
        assert!(!tickets.redeem(&ticket), "a redeemed ticket is gone");
        assert!(!tickets.redeem("00000000"), "an unknown ticket is refused");
        assert_eq!(tickets.live_count(), 0);
    }
}
