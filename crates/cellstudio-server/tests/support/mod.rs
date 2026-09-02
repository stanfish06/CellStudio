//! Integration-test harness: spawns the real sidecar binary and hands back an authenticated client.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

/// Env var naming the real development dataset; real-data tests skip without it.
pub const DEV_DATASET_ENV: &str = "CELLSTUDIO_DEV_DATASET";

const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Server {
    child: Child,
    pub port: u16,
    pub base_url: String,
    pub token: String,
    client: reqwest::blocking::Client,
}

impl Server {
    pub fn start() -> Self {
        Self::with_args(&[])
    }

    /// No proxy job: tests that only read pixels should not race a background build.
    pub fn without_proxy() -> Self {
        Self::with_args(&["--no-proxy"])
    }

    pub fn with_args(extra: &[&str]) -> Self {
        let token = cellstudio_server::auth::random_hex(16);
        let mut child = Command::new(env!("CARGO_BIN_EXE_cellstudio-server"))
            .arg("--token")
            .arg(&token)
            .args(extra)
            .env(
                "CELLSTUDIO_LOG",
                std::env::var("CELLSTUDIO_LOG").unwrap_or("warn".into()),
            )
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn cellstudio-server");

        let stdout = child.stdout.take().expect("piped stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read the handshake line");
        let handshake: Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("{line:?}: {e}"));
        let port = handshake
            .get("port")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| panic!("handshake {handshake} has no numeric port"))
            as u16;

        let server = Self {
            child,
            port,
            base_url: format!("http://127.0.0.1:{port}"),
            token,
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("client"),
        };
        server.await_health();
        server
    }

    fn await_health(&self) {
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        loop {
            if let Ok(res) = self.get("/health").send()
                && res.status().is_success()
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "/health did not answer within {HEALTH_TIMEOUT:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    pub fn client(&self) -> reqwest::blocking::Client {
        self.client.clone()
    }

    /// Authenticated GET, as the renderer's `ApiClient` sends it.
    pub fn get(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client.get(self.url(path)).bearer_auth(&self.token)
    }

    pub fn post(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client.post(self.url(path)).bearer_auth(&self.token)
    }

    /// POST carrying the session fence every project mutation requires.
    pub fn post_as(&self, path: &str, session: &str, body: &Value) -> reqwest::blocking::Response {
        self.post(path)
            .header(cellstudio_server::wire::SESSION_HEADER, session)
            .json(body)
            .send()
            .expect("request")
    }

    /// A mutation under the current session, asserted to succeed.
    pub fn mutate(&self, path: &str, body: Value) -> Value {
        let response = self.post_as(path, &self.session(), &body);
        let status = response.status();
        let text = response.text().expect("body");
        assert!(status.is_success(), "POST {path} -> {status}: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("POST {path} body {text:?}: {e}"))
    }

    pub fn put(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client.put(self.url(path)).bearer_auth(&self.token)
    }

    pub fn delete(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client.delete(self.url(path)).bearer_auth(&self.token)
    }

    /// A session-fenced non-POST mutation, asserted to succeed.
    pub fn mutate_with(&self, request: reqwest::blocking::RequestBuilder) -> Value {
        let response = request
            .header(cellstudio_server::wire::SESSION_HEADER, self.session())
            .send()
            .expect("request");
        let status = response.status();
        let text = response.text().expect("body");
        assert!(status.is_success(), "{status}: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("body {text:?}: {e}"))
    }

    /// `HEAD`, which a zarr client uses to size an object before ranging into it.
    pub fn head(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(reqwest::Method::HEAD, self.url(path))
            .bearer_auth(&self.token)
    }

    /// Unauthenticated GET, for the 401 path.
    pub fn get_anonymous(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client.get(self.url(path))
    }

    pub fn ws_ticket(&self) -> String {
        self.json("/ws-ticket")["ticket"]
            .as_str()
            .expect("ws-ticket body carries a `ticket` string")
            .to_owned()
    }

    /// Opens `/events` with `ticket`. `Err(status)` is the status the handshake was
    /// refused with, before any event could be delivered.
    pub fn connect_events(&self, ticket: &str) -> Result<Events, u16> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime for the event socket");
        let url = format!("ws://127.0.0.1:{}/events?ticket={ticket}", self.port);
        match runtime.block_on(tokio_tungstenite::connect_async(url)) {
            Ok((socket, _)) => Ok(Events { runtime, socket }),
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                Err(response.status().as_u16())
            }
            Err(e) => panic!("connect /events: {e}"),
        }
    }

    pub fn json(&self, path: &str) -> Value {
        let res = self.get(path).send().expect("request");
        let status = res.status();
        let body = res.text().expect("body");
        assert!(status.is_success(), "GET {path} -> {status}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {path} body {body:?}: {e}"))
    }

    pub fn open_project(&self, dataset: &Path) -> Value {
        let res = self
            .post("/project/open")
            .json(&serde_json::json!({ "path": dataset }))
            .send()
            .expect("open");
        let status = res.status();
        let body = res.text().expect("body");
        assert!(status.is_success(), "open {dataset:?} -> {status}: {body}");
        serde_json::from_str(&body).expect("ProjectInfo")
    }

    pub fn try_open_project(&self, dataset: &Path) -> reqwest::blocking::Response {
        self.post("/project/open")
            .json(&serde_json::json!({ "path": dataset }))
            .send()
            .expect("open")
    }

    pub fn session(&self) -> String {
        self.json("/project")["sessionId"]
            .as_str()
            .expect("sessionId")
            .to_owned()
    }

    pub fn jobs(&self) -> Vec<Value> {
        self.json("/jobs").as_array().cloned().unwrap_or_default()
    }

    /// Waits until every job reaches a terminal status, returning the final list.
    pub fn await_jobs(&self, timeout: Duration) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let jobs = self.jobs();
            let running = jobs.iter().any(|job| job["status"] == "running");
            if !running && !jobs.is_empty() {
                return jobs;
            }
            if Instant::now() >= deadline {
                return jobs;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A connected `/events` socket, read synchronously so tests stay blocking.
pub struct Events {
    runtime: tokio::runtime::Runtime,
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl Events {
    /// The next event frame, or `None` when the socket stays quiet for `timeout`.
    pub fn try_next(&mut self, timeout: Duration) -> Option<Value> {
        use futures::StreamExt;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Self { runtime, socket } = self;
            // the timer is built inside block_on: `timeout` needs the runtime's reactor
            let frame =
                runtime.block_on(async { tokio::time::timeout(remaining, socket.next()).await });
            match frame {
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Some(
                        serde_json::from_str(text.as_str())
                            .unwrap_or_else(|e| panic!("event frame {text:?}: {e}")),
                    );
                }
                // control frames are not events
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(e))) => panic!("event socket failed: {e}"),
                Ok(None) | Err(_) => return None,
            }
        }
    }

    pub fn next_event(&mut self, timeout: Duration) -> Value {
        self.try_next(timeout)
            .unwrap_or_else(|| panic!("no event arrived within {timeout:?}"))
    }

    /// The next frame whose `type` is `kind`, skipping the ones that are not.
    pub fn next_event_of(&mut self, kind: &str, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.try_next(remaining) {
                Some(event) if event["type"] == kind => return event,
                Some(event) => seen.push(event["type"].clone()),
                None => panic!("no {kind} event within {timeout:?}; saw {seen:?}"),
            }
        }
    }

    /// Every frame that arrives within `window`.
    pub fn drain(&mut self, window: Duration) -> Vec<Value> {
        let deadline = Instant::now() + window;
        let mut frames = Vec::new();
        while let Some(event) = self.try_next(deadline.saturating_duration_since(Instant::now())) {
            frames.push(event);
        }
        frames
    }
}

/// Binary response metadata as the client reads it off the headers.
pub struct Binary {
    pub shape: Vec<u64>,
    pub dtype: String,
    pub level: u32,
    pub session: String,
    pub bytes: Vec<u8>,
    pub extra: std::collections::HashMap<String, String>,
}

impl Binary {
    pub fn read(response: reqwest::blocking::Response) -> Self {
        let status = response.status();
        assert!(status.is_success(), "binary read -> {status}");
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_else(|| panic!("missing header {name}"))
                .to_owned()
        };
        let shape: Vec<u64> = header("x-cellstudio-shape")
            .split(',')
            .map(|part| part.trim().parse().expect("shape extent"))
            .collect();
        let dtype = header("x-cellstudio-dtype");
        let level = header("x-cellstudio-level").parse().expect("level");
        let session = header("x-cellstudio-session");
        let declared: usize = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .expect("content-length");
        let extra = ["x-cellstudio-volume-source"]
            .into_iter()
            .filter_map(|name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| (name.to_owned(), v.to_owned()))
            })
            .collect();
        let bytes = response.bytes().expect("body").to_vec();
        let item = item_size(&dtype);
        let expected = shape.iter().product::<u64>() as usize * item;
        assert_eq!(
            bytes.len(),
            expected,
            "body length must equal product(shape) * itemsize"
        );
        assert_eq!(
            declared, expected,
            "content-length must equal product(shape) * itemsize"
        );
        Self {
            shape,
            dtype,
            level,
            session,
            bytes,
            extra,
        }
    }

    pub fn u32_values(&self) -> Vec<u32> {
        assert_eq!(self.dtype, "u32");
        self.bytes
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    pub fn u16_values(&self) -> Vec<u16> {
        assert_eq!(self.dtype, "u16");
        self.bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect()
    }
}

pub fn item_size(dtype: &str) -> usize {
    match dtype {
        "u8" => 1,
        "u16" => 2,
        "u32" => 4,
        other => panic!("unknown dtype {other}"),
    }
}

/// `<repo>/.data`: the tiny correctness stores.
fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.data")
        .canonicalize()
        .expect("data directory (run `mise run data`)")
}

/// Path to one artifact inside a named data store.
pub fn data(name: &str, artifact: &str) -> PathBuf {
    let path = data_dir().join(name).join(artifact);
    assert!(path.exists(), "missing data {path:?} (run `mise run data`)");
    path
}

/// A private copy of a data store, so project directories and caches land in a tempdir
/// instead of next to the checked-in store.
pub fn data_copy(dir: &tempfile::TempDir, name: &str, artifact: &str) -> PathBuf {
    let source = data_dir().join(name).join(artifact);
    let target = dir.path().join(format!("{name}-{artifact}"));
    copy_tree(&source, &target);
    target
}

pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// The development dataset, symlinked into `dir` so its project container is created in
/// the tempdir and the read-only original is never written next to.
pub fn dev_dataset(dir: &tempfile::TempDir) -> Option<PathBuf> {
    let raw = std::env::var(DEV_DATASET_ENV).ok()?;
    let source = PathBuf::from(&raw);
    if !source.is_dir() {
        panic!("{DEV_DATASET_ENV}={raw} is not a directory");
    }
    let link = dir.path().join("dev.zarr");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &link).expect("symlink the development dataset");
    Some(link)
}

/// Every file under `root` keyed by its path relative to it. Data only; it holds the
/// whole tree in memory, which is how "byte-identical before and after" is checked.
pub fn store_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    snapshot_into(root, root, &mut files);
    files
}

fn snapshot_into(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if entry.file_type().expect("file type").is_dir() {
            snapshot_into(root, &path, out);
        } else {
            let relative = path.strip_prefix(root).expect("under root").to_path_buf();
            out.insert(relative, std::fs::read(&path).expect("read file"));
        }
    }
}

pub fn skip(reason: &str) {
    eprintln!("skipped: {reason}");
}
