use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

/// Annotated cell state. Independent of graph structure: a `Dividing` annotation is
/// stored as given regardless of child count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellState {
    Normal,
    Dividing,
    Death,
}

impl CellState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CellState::Normal => "normal",
            CellState::Dividing => "dividing",
            CellState::Death => "death",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "normal" => Some(CellState::Normal),
            "dividing" => Some(CellState::Dividing),
            "death" => Some(CellState::Death),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkRef {
    pub id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// One detection in a tracking file. `seg_id` is the label value in the mask at frame
/// `t`; `track_id` is a pre-existing identity from an upstream tracker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellRecord {
    pub id: u32,
    pub t: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seg_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
    /// `[z, y, x]` in pixel units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centroid: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<LinkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<LinkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<CellState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub features: serde_json::Map<String, serde_json::Value>,
}

pub const TRACKING_FORMAT: &str = "cellstudio-tracking";
pub const TRACKING_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct TrackingHeader {
    pub format: String,
    pub version: u32,
    pub metadata: TrackingMetadata,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrackingMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape_tczyx: Option<[u64; 5]>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum TrackingOpenError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("not a readable {TRACKING_FORMAT} file: {0}")]
    Header(String),
}

/// A record the stream could not produce; everything staged before it must be discarded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cells[{index}]: {message}")]
pub struct ParseError {
    /// 0-based position in the `cells` array.
    pub index: u64,
    pub message: String,
}

/// A tracking file with its header validated and its `cells` array still unread: records
/// stream one at a time, so the whole file is never in memory.
pub struct TrackingStream {
    pub header: TrackingHeader,
    pub records: TrackingRecords,
    consumed: Arc<AtomicU64>,
    total_bytes: u64,
}

impl TrackingStream {
    /// Read-position probe for job progress; counts compressed bytes, so it is monotone
    /// for both plain and gzipped files.
    pub fn progress_probe(&self) -> ProgressProbe {
        ProgressProbe {
            consumed: self.consumed.clone(),
            total: self.total_bytes.max(1),
        }
    }
}

pub struct ProgressProbe {
    consumed: Arc<AtomicU64>,
    total: u64,
}

impl ProgressProbe {
    pub fn fraction(&self) -> f32 {
        (self.consumed.load(Ordering::Relaxed) as f32 / self.total as f32).min(1.0)
    }
}

/// Opens a `cellstudio-tracking` file, plain or gzipped — chosen by the gzip magic bytes,
/// not the extension — and validates the header (format, version, metadata, all of which
/// must precede `cells`) before any record is yielded.
pub fn open_tracking(path: &Path) -> Result<TrackingStream, TrackingOpenError> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 2];
    let seen = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    let total_bytes = file.metadata()?.len();

    let consumed = Arc::new(AtomicU64::new(0));
    let counted = CountingReader {
        inner: file,
        count: consumed.clone(),
    };
    let raw: Box<dyn Read + Send> = if seen == 2 && magic == [0x1f, 0x8b] {
        Box::new(GzDecoder::new(counted))
    } else {
        Box::new(counted)
    };

    let mut scanner = Scanner::new(raw);
    let header = read_header(&mut scanner)?;
    Ok(TrackingStream {
        header,
        records: TrackingRecords {
            scanner,
            buf: Vec::new(),
            index: 0,
            started: false,
            done: false,
        },
        consumed,
        total_bytes,
    })
}

fn read_header(scanner: &mut Scanner) -> Result<TrackingHeader, TrackingOpenError> {
    let bad = |message: &str| TrackingOpenError::Header(message.to_owned());
    if scanner.next_non_ws()? != Some(b'{') {
        return Err(bad("the file is not a JSON object"));
    }

    let mut format: Option<String> = None;
    let mut version: Option<u32> = None;
    let mut metadata: Option<TrackingMetadata> = None;
    let mut buf = Vec::new();
    loop {
        match scanner.next_non_ws()? {
            Some(b'"') => {}
            Some(b'}') | None => return Err(bad("the file has no `cells` array")),
            Some(_) => return Err(bad("expected an object key")),
        }
        buf.clear();
        scanner.read_string(&mut buf)?;
        let key: String = serde_json::from_slice(&buf)
            .map_err(|e| TrackingOpenError::Header(format!("unreadable object key: {e}")))?;
        if scanner.next_non_ws()? != Some(b':') {
            return Err(bad("expected `:` after an object key"));
        }
        if key == "cells" {
            if scanner.next_non_ws()? != Some(b'[') {
                return Err(bad("`cells` must be an array"));
            }
            break;
        }
        buf.clear();
        scanner.read_value(&mut buf)?;
        let parse_err = |e: serde_json::Error| TrackingOpenError::Header(format!("`{key}`: {e}"));
        match key.as_str() {
            "format" => format = Some(serde_json::from_slice(&buf).map_err(parse_err)?),
            "version" => version = Some(serde_json::from_slice(&buf).map_err(parse_err)?),
            "metadata" => metadata = Some(serde_json::from_slice(&buf).map_err(parse_err)?),
            _ => {}
        }
        match scanner.next_non_ws()? {
            Some(b',') => {}
            Some(b'}') | None => return Err(bad("the file has no `cells` array")),
            Some(_) => return Err(bad("expected `,` or `}` between header fields")),
        }
    }

    // all header fields must precede `cells`: nothing after the array can be read without
    // buffering every record first
    let format = format.ok_or_else(|| bad("`format` must precede `cells`"))?;
    if format != TRACKING_FORMAT {
        return Err(TrackingOpenError::Header(format!(
            "format {format:?} is not {TRACKING_FORMAT:?}"
        )));
    }
    let version = version.ok_or_else(|| bad("`version` must precede `cells`"))?;
    if version != TRACKING_VERSION {
        return Err(TrackingOpenError::Header(format!(
            "version {version} is not supported (this build reads version {TRACKING_VERSION})"
        )));
    }
    let metadata = metadata.ok_or_else(|| bad("`metadata` must precede `cells`"))?;
    Ok(TrackingHeader {
        format,
        version,
        metadata,
    })
}

/// The `cells` array, one record per `next()`. The first error fuses the iterator.
pub struct TrackingRecords {
    scanner: Scanner,
    buf: Vec<u8>,
    index: u64,
    started: bool,
    done: bool,
}

impl Iterator for TrackingRecords {
    type Item = Result<CellRecord, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.advance() {
            Ok(Some(record)) => {
                self.index += 1;
                Some(Ok(record))
            }
            Ok(None) => {
                self.done = true;
                None
            }
            Err(message) => {
                self.done = true;
                Some(Err(ParseError {
                    index: self.index,
                    message,
                }))
            }
        }
    }
}

impl TrackingRecords {
    fn advance(&mut self) -> Result<Option<CellRecord>, String> {
        let io_err = |e: io::Error| e.to_string();
        if self.started {
            match self.scanner.next_non_ws().map_err(io_err)? {
                Some(b',') => {}
                Some(b']') => return Ok(None),
                Some(other) => {
                    return Err(format!(
                        "expected `,` or `]` after a record, found {:?}",
                        char::from(other)
                    ));
                }
                None => return Err("the `cells` array is unterminated".to_owned()),
            }
        } else {
            self.started = true;
            if self.scanner.peek_non_ws().map_err(io_err)? == Some(b']') {
                self.scanner.discard_peeked();
                return Ok(None);
            }
        }
        self.buf.clear();
        self.scanner.read_value(&mut self.buf).map_err(io_err)?;
        serde_json::from_slice(&self.buf)
            .map(Some)
            .map_err(|e| e.to_string())
    }
}

struct CountingReader<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Byte-level JSON scanner: just enough structure to find the header fields and hand each
/// `cells` element to serde whole.
struct Scanner {
    reader: BufReader<Box<dyn Read + Send>>,
    peeked: Option<u8>,
}

fn eof() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected end of JSON")
}

impl Scanner {
    fn new(raw: Box<dyn Read + Send>) -> Self {
        Self {
            reader: BufReader::new(raw),
            peeked: None,
        }
    }

    fn next_byte(&mut self) -> io::Result<Option<u8>> {
        if let Some(byte) = self.peeked.take() {
            return Ok(Some(byte));
        }
        let mut buf = [0u8; 1];
        loop {
            match self.reader.read(&mut buf) {
                Ok(0) => return Ok(None),
                Ok(_) => return Ok(Some(buf[0])),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    fn peek_byte(&mut self) -> io::Result<Option<u8>> {
        if self.peeked.is_none() {
            self.peeked = self.next_byte()?;
        }
        Ok(self.peeked)
    }

    fn discard_peeked(&mut self) {
        self.peeked = None;
    }

    fn next_non_ws(&mut self) -> io::Result<Option<u8>> {
        loop {
            match self.next_byte()? {
                Some(byte) if byte.is_ascii_whitespace() => {}
                other => return Ok(other),
            }
        }
    }

    fn peek_non_ws(&mut self) -> io::Result<Option<u8>> {
        let byte = self.next_non_ws()?;
        self.peeked = byte;
        Ok(byte)
    }

    /// One complete JSON value appended to `out`: containers by bracket depth, strings
    /// escape-aware, scalars until a delimiter (which stays unconsumed).
    fn read_value(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        let first = self.next_non_ws()?.ok_or_else(eof)?;
        match first {
            b'"' => self.read_string(out),
            b'{' | b'[' => {
                out.push(first);
                let mut depth = 1usize;
                while depth > 0 {
                    let byte = self.next_byte()?.ok_or_else(eof)?;
                    if byte == b'"' {
                        self.read_string(out)?;
                        continue;
                    }
                    match byte {
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => depth -= 1,
                        _ => {}
                    }
                    out.push(byte);
                }
                Ok(())
            }
            _ => {
                out.push(first);
                loop {
                    match self.peek_byte()? {
                        None => return Ok(()),
                        Some(byte)
                            if byte.is_ascii_whitespace() || matches!(byte, b',' | b'}' | b']') =>
                        {
                            return Ok(());
                        }
                        Some(byte) => {
                            self.discard_peeked();
                            out.push(byte);
                        }
                    }
                }
            }
        }
    }

    /// The rest of a string whose opening quote was already consumed; `out` receives the
    /// whole quoted form.
    fn read_string(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        out.push(b'"');
        loop {
            let byte = self.next_byte()?.ok_or_else(eof)?;
            out.push(byte);
            match byte {
                b'\\' => out.push(self.next_byte()?.ok_or_else(eof)?),
                b'"' => return Ok(()),
                _ => {}
            }
        }
    }
}
