//! The JSON contract packages/api-client/src/schemas.ts parses; key sets are asserted literally.

mod support;

use serde_json::{Value, json};

use support::{Server, data_copy};

fn keys(value: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    names
}

/// Nothing in the payload may be snake_case: zod strips unknown keys silently, so a
/// `source_path` would reach the renderer as `undefined` rather than as an error.
fn assert_camel_case(value: &Value, path: &str) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                assert!(
                    !name.contains('_'),
                    "{path}.{name} is not camelCase; the TypeScript client reads camelCase"
                );
                assert_camel_case(child, &format!("{path}.{name}"));
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_camel_case(item, &format!("{path}[{index}]"));
            }
        }
        _ => {}
    }
}

#[test]
fn project_info_matches_the_typescript_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    let info = server.open_project(&dataset);
    assert_camel_case(&info, "ProjectInfo");

    assert_eq!(
        keys(&info),
        [
            "channels",
            "dims",
            "dtype",
            "hasLabels",
            "layout",
            "levels",
            "projectPath",
            "scale",
            "sessionId",
            "sourcePath",
            "versions",
        ]
    );
    assert_eq!(keys(&info["dims"]), ["c", "t", "x", "y", "z"]);
    assert_eq!(keys(&info["scale"]), ["x", "y", "z"]);
    assert_eq!(
        keys(&info["levels"][0]),
        ["chunks", "dims", "factor", "index"]
    );
    assert_eq!(keys(&info["channels"][0]), ["color", "name", "window"]);
    assert_eq!(
        keys(&info["versions"]),
        ["graph", "image", "labels", "sessionId", "settings"]
    );
    assert_eq!(
        keys(&info["layout"]),
        ["affectedViews", "amplification", "hostile"]
    );
    assert_eq!(keys(&info["layout"]["amplification"]), ["xy", "xz", "yz"]);

    // the value shapes zod validates
    assert!(
        info["levels"][0]["factor"]
            .as_array()
            .expect("factor")
            .len()
            == 3
    );
    assert!(
        info["channels"][0]["window"]
            .as_array()
            .expect("window")
            .len()
            == 2
    );
    assert!(
        matches!(info["dtype"].as_str(), Some("u8" | "u16" | "u32")),
        "dtype must be one of the three the client knows: {}",
        info["dtype"]
    );
    for view in info["layout"]["affectedViews"]
        .as_array()
        .expect("affectedViews")
    {
        assert!(matches!(view.as_str(), Some("xy" | "xz" | "yz")), "{view}");
    }
}

#[test]
fn a_dataset_without_metadata_still_answers_the_nullable_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "no_scale_metadata", "image.zarr");
    let server = Server::without_proxy();
    let info = server.open_project(&dataset);
    assert_camel_case(&info, "ProjectInfo");

    // `scale`, `color` and `window` are nullable in the schema, never absent
    assert!(info.get("scale").is_some(), "scale must be present: {info}");
    let channel = &info["channels"][0];
    assert!(
        channel.get("color").is_some(),
        "color must be present: {channel}"
    );
    assert!(
        channel.get("window").is_some(),
        "window must be present: {channel}"
    );
}

#[test]
fn health_and_pixel_and_ticket_bodies_match_the_typescript_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();

    let health = server.json("/health");
    assert_camel_case(&health, "HealthInfo");
    assert_eq!(keys(&health), ["reads", "session", "status", "version"]);
    assert_eq!(keys(&health["reads"]), ["inflight", "peak", "permits"]);

    let ticket = server.json("/ws-ticket");
    assert_eq!(keys(&ticket), ["ticket"]);

    server.open_project(&dataset);
    let pixel = server.json("/pixel?t=0&c=0&z=0&y=0&x=0");
    assert_eq!(keys(&pixel), ["value"]);
    assert!(pixel["value"].is_u64(), "{pixel}");
}

#[test]
fn the_histogram_body_carries_the_four_fields_the_client_parses() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "tiny_v2", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let histogram = server.json("/histogram?t=0&c=0");
    assert_camel_case(&histogram, "Histogram");
    assert_eq!(
        keys(&histogram),
        // `level` and `samples` are additive; the schema's four are the contract
        ["counts", "level", "max", "min", "sampled", "samples"]
    );
    assert!(
        histogram["counts"]
            .as_array()
            .expect("counts")
            .iter()
            .all(Value::is_u64)
    );
    assert!(histogram["sampled"].is_boolean());
}

#[test]
fn job_bodies_match_the_typescript_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let started = server
        .post("/rechunk")
        .json(&json!({ "z": 16, "y": 32, "x": 32 }))
        .send()
        .expect("rechunk");
    let job_ref: Value = started.json().expect("JobRef");
    assert_eq!(keys(&job_ref), ["id"]);

    let jobs = server.await_jobs(std::time::Duration::from_secs(120));
    let job = jobs.first().expect("one job");
    assert_camel_case(job, "JobState");
    assert_eq!(keys(job), ["id", "kind", "message", "progress", "status"]);
    assert!(
        matches!(
            job["kind"].as_str(),
            Some("rechunk" | "proxy" | "import-tracks" | "import-labels" | "export")
        ),
        "job kinds are kebab-case in the schema: {job}"
    );
    assert!(
        matches!(
            job["status"].as_str(),
            Some("running" | "done" | "failed" | "cancelled")
        ),
        "{job}"
    );
    let progress = job["progress"].as_f64().expect("progress");
    assert!((0.0..=1.0).contains(&progress), "{job}");
}

#[test]
fn event_frames_match_the_discriminated_union() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dataset = data_copy(&dir, "hostile_planes", "image.zarr");
    let server = Server::without_proxy();
    server.open_project(&dataset);

    let mut events = server
        .connect_events(&server.ws_ticket())
        .expect("connect /events");

    let versions = events.next_event(std::time::Duration::from_secs(10));
    assert_camel_case(&versions, "ServerEvent");
    assert_eq!(keys(&versions), ["type", "versions"]);
    assert_eq!(
        versions["type"], "versions",
        "the discriminant is camelCase"
    );
    assert_eq!(
        keys(&versions["versions"]),
        ["graph", "image", "labels", "sessionId", "settings"]
    );

    server
        .post("/rechunk")
        .json(&json!({ "z": 16, "y": 32, "x": 32 }))
        .send()
        .expect("rechunk");
    let job = events.next_event_of("job", std::time::Duration::from_secs(30));
    assert_camel_case(&job, "ServerEvent");
    assert_eq!(keys(&job), ["job", "type"]);
    assert_eq!(
        keys(&job["job"]),
        ["id", "kind", "message", "progress", "status"]
    );
}
