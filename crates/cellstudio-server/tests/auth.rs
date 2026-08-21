//! Loopback-only authenticated transport: bearer token, Origin validation, CORS preflight, /events tickets.

mod support;

use reqwest::Method;
use reqwest::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ORIGIN,
};
use serde_json::Value;

use support::Server;

/// Every route reachable before a project is open, so a refusal cannot be confused with
/// "no project".
const PATHS: [&str; 6] = [
    "/health",
    "/project",
    "/jobs",
    "/ws-ticket",
    "/store/.zattrs",
    "/slice?axis=xz&t=0&cs=0&pos=0",
];

#[test]
fn health_answers_before_any_project_is_open() {
    let server = Server::without_proxy();
    let health = server.json("/health");
    assert_eq!(health["status"], "ok", "{health}");
    assert!(
        health["session"].is_null(),
        "no session exists before the first open: {health}"
    );
    assert!(
        health["reads"]["permits"]
            .as_u64()
            .expect("reads.permits is a number")
            >= 2,
        "the decode pool never drops below two permits: {health}"
    );
}

#[test]
fn a_request_without_a_token_is_rejected_with_401() {
    let server = Server::without_proxy();
    for path in PATHS {
        let response = server.get_anonymous(path).send().expect("request");
        assert_eq!(response.status(), 401, "GET {path} without a token");
        let body: Value = response.json().expect("an error body, not data");
        assert_eq!(
            body["error"], "missing or invalid session token",
            "GET {path} must answer with the auth error and no data"
        );
    }
}

#[test]
fn a_request_with_the_wrong_token_is_rejected_with_401() {
    let server = Server::without_proxy();
    let wrong = ["".to_owned(), "not-the-token".to_owned(), "0".repeat(32)];
    for token in &wrong {
        for path in PATHS {
            let response = server
                .client()
                .get(server.url(path))
                .bearer_auth(token)
                .send()
                .expect("request");
            assert_eq!(response.status(), 401, "GET {path} with token {token:?}");
        }
    }
}

#[test]
fn a_disallowed_origin_is_rejected_with_403() {
    let server = Server::without_proxy();
    for origin in [
        "http://evil.example",
        "https://localhost.evil.example",
        "http://127.0.0.1.evil.example",
        "ws://localhost:5173",
    ] {
        let response = server
            .get("/health")
            .header(ORIGIN, origin)
            .send()
            .expect("request");
        assert_eq!(response.status(), 403, "Origin {origin} must be refused");
        let body: Value = response.json().expect("an error body, not data");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|message| message.contains("is not allowed")),
            "Origin {origin} refusal must name the origin: {body}"
        );
    }
}

#[test]
fn the_renderers_own_origins_are_accepted() {
    let server = Server::without_proxy();
    for origin in [
        "null",
        "file://",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://[::1]:5173",
    ] {
        let response = server
            .get("/health")
            .header(ORIGIN, origin)
            .send()
            .expect("request");
        assert_eq!(response.status(), 200, "Origin {origin} must be accepted");
    }
}

#[test]
fn the_cors_preflight_for_authorization_succeeds() {
    let server = Server::without_proxy();
    let response = server
        .client()
        .request(Method::OPTIONS, server.url("/project"))
        .header(ORIGIN, "http://localhost:5173")
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "authorization")
        .send()
        .expect("preflight");
    assert!(
        response.status().is_success(),
        "the preflight a browser sends before an Authorization request -> {}",
        response.status()
    );
    let headers = response.headers().clone();
    let value = |name: reqwest::header::HeaderName| {
        headers
            .get(&name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_else(|| panic!("preflight is missing {name}"))
            .to_ascii_lowercase()
    };

    assert_eq!(
        value(ACCESS_CONTROL_ALLOW_ORIGIN),
        "http://localhost:5173",
        "the preflight echoes the allowed origin"
    );
    let allowed_headers = value(ACCESS_CONTROL_ALLOW_HEADERS);
    for header in ["authorization", "content-type", "range"] {
        assert!(
            allowed_headers.contains(header),
            "{header} must survive the preflight: {allowed_headers}"
        );
    }
    let allowed_methods = value(ACCESS_CONTROL_ALLOW_METHODS);
    for method in ["get", "head", "post", "put"] {
        assert!(
            allowed_methods.contains(method),
            "{method} must survive the preflight: {allowed_methods}"
        );
    }
}

/// A browser hides every response header the server does not expose, so the binary
/// contract dies silently without these. They ride on the real response, not the preflight.
#[test]
fn the_binary_response_headers_are_exposed_to_the_renderer() {
    let server = Server::without_proxy();
    let response = server
        .get("/health")
        .header(ORIGIN, "http://localhost:5173")
        .send()
        .expect("request");
    assert!(response.status().is_success());
    let exposed = response
        .headers()
        .get(ACCESS_CONTROL_EXPOSE_HEADERS)
        .and_then(|value| value.to_str().ok())
        .expect("the response exposes headers")
        .to_ascii_lowercase();
    for header in [
        "x-cellstudio-session",
        "x-cellstudio-shape",
        "x-cellstudio-dtype",
        "x-cellstudio-level",
        "x-cellstudio-volume-source",
        "content-length",
        "content-range",
        "accept-ranges",
    ] {
        assert!(
            exposed.contains(header),
            "{header} must be exposed to the renderer: {exposed}"
        );
    }
}

#[test]
fn a_ws_ticket_is_issued_only_to_authenticated_callers() {
    let server = Server::without_proxy();
    assert_eq!(
        server
            .get_anonymous("/ws-ticket")
            .send()
            .expect("request")
            .status(),
        401,
        "an unauthenticated caller cannot mint a ticket"
    );

    let ticket = server.ws_ticket();
    assert_eq!(ticket.len(), 64, "32 random bytes as hex: {ticket}");
    assert!(
        ticket.chars().all(|c| c.is_ascii_hexdigit()),
        "{ticket} must be hex"
    );
    assert_ne!(server.ws_ticket(), ticket, "every call mints a new ticket");
}

#[test]
fn a_ticket_opens_the_event_socket_exactly_once() {
    let server = Server::without_proxy();
    let ticket = server.ws_ticket();

    let first = server
        .connect_events(&ticket)
        .expect("the first connect redeems the ticket");
    drop(first);

    match server.connect_events(&ticket) {
        Err(status) => assert_eq!(status, 401, "a redeemed ticket must be refused"),
        Ok(_) => panic!("a redeemed ticket opened a second event socket"),
    }
}

#[test]
fn an_unknown_ticket_never_opens_the_event_socket() {
    let server = Server::without_proxy();
    // one live ticket exists, so the refusals below are about the value, not an empty table
    let live = server.ws_ticket();
    let mut forged = live.clone();
    forged.replace_range(0..1, if live.starts_with('0') { "1" } else { "0" });
    let all_f = "f".repeat(64);
    for unknown in ["", "00", all_f.as_str(), forged.as_str()] {
        match server.connect_events(unknown) {
            Err(status) => assert_eq!(status, 401, "ticket {unknown:?} must be refused"),
            Ok(_) => panic!("unknown ticket {unknown:?} opened the event socket"),
        }
    }
}
