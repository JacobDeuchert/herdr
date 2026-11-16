//! fork: thread service reader for the sidebar thread list (FORK.md 0005).
//!
//! One background thread per client process follows `GET <url>/api/events`
//! (server-sent events) and refetches `GET <url>/api/sessions` whenever the
//! `stamp` event's data changes. Nothing polls: a refetch happens on connect
//! and on each new stamp. When the service drops or fails, the last rows stay
//! and are marked stale until a reconnect (with backoff) succeeds.
//!
//! Plain `http://` over `std::net::TcpStream`; requests are HTTP/1.0 so the
//! server answers with an unframed body that ends at close. Chunked bodies are
//! decoded anyway, in case a server ignores that.
//!
//! Every string from the service is agent-written and untrusted: it is
//! stripped of control and bidi formatting characters and length-capped
//! before it reaches the UI. Thread ids end up in a shell command, so rows
//! whose id is not `[A-Za-z0-9_.:-]{1,128}` are dropped.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TICK: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// An event stream with no bytes for this long is treated as dead.
const STREAM_IDLE_LIMIT: Duration = Duration::from_secs(600);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_ROWS: usize = 500;
const MAX_TEXT_CHARS: usize = 200;
const MAX_THREAD_ID_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadStatus {
    Idle,
    Running,
    Blocked,
    Done,
    Stopped,
    Error,
    Unknown,
}

impl ThreadStatus {
    fn parse(value: &str) -> Self {
        match value {
            "idle" => Self::Idle,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "stopped" => Self::Stopped,
            "error" => Self::Error,
            _ => Self::Unknown,
        }
    }

    /// herdr's own status, for the shared icons and colours. `error` borrows
    /// `blocked` so it stands out the same way.
    pub(crate) fn agent_status(self) -> crate::api::schema::AgentStatus {
        use crate::api::schema::AgentStatus;
        match self {
            Self::Idle => AgentStatus::Idle,
            Self::Running => AgentStatus::Working,
            Self::Blocked | Self::Error => AgentStatus::Blocked,
            Self::Done => AgentStatus::Done,
            Self::Stopped | Self::Unknown => AgentStatus::Unknown,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Stopped => "stopped",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn needs_attention(self) -> bool {
        matches!(self, Self::Blocked | Self::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadTicket {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadRow {
    pub(crate) id: String,
    pub(crate) ticket: Option<ThreadTicket>,
    pub(crate) status: ThreadStatus,
    pub(crate) agent: String,
    pub(crate) model: String,
    pub(crate) title: String,
    pub(crate) cwd: String,
}

impl ThreadRow {
    pub(crate) fn display_title(&self) -> &str {
        if self.title.is_empty() {
            &self.id
        } else {
            &self.title
        }
    }
}

/// Strips control and bidi/format characters, collapses whitespace runs, and
/// caps the length. Agent-written text must not be able to move the cursor,
/// recolour the sidebar, or reorder what is drawn.
pub(crate) fn sanitize_text(value: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if character.is_control() || is_format_character(character) {
            continue;
        }
        if out.chars().count() >= MAX_TEXT_CHARS {
            break;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(character);
    }
    out
}

fn is_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}

/// Thread ids go into a shell command line, so only a conservative alphabet
/// is accepted.
pub(crate) fn valid_thread_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_THREAD_ID_CHARS
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct WireTicket {
    id: Option<serde_json::Value>,
    label: Option<String>,
    title: Option<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct WireSession {
    id: Option<String>,
    ticket: Option<WireTicket>,
    status: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    title: Option<String>,
    cwd: Option<String>,
}

fn ticket_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(id) => Some(sanitize_text(id)),
        serde_json::Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
    .filter(|id| !id.is_empty())
}

/// Parses `GET /api/sessions`: a JSON array, newest first. Unknown fields are
/// ignored; malformed items and items with unusable ids are skipped.
pub(crate) fn parse_sessions(body: &[u8]) -> Result<Vec<ThreadRow>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(body)
        .map_err(|error| format!("thread list is not a JSON array: {error}"))?;
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if rows.len() >= MAX_ROWS {
            break;
        }
        let Ok(session) = serde_json::from_value::<WireSession>(item) else {
            continue;
        };
        let Some(id) = session.id.filter(|id| valid_thread_id(id)) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let ticket = session.ticket.and_then(|ticket| {
            let id = ticket.id.as_ref().and_then(ticket_id)?;
            let label = sanitize_text(ticket.label.as_deref().unwrap_or_default());
            Some(ThreadTicket {
                label: if label.is_empty() { id.clone() } else { label },
                title: sanitize_text(ticket.title.as_deref().unwrap_or_default()),
                id,
            })
        });
        rows.push(ThreadRow {
            id,
            ticket,
            status: ThreadStatus::parse(session.status.as_deref().unwrap_or_default()),
            agent: sanitize_text(session.agent.as_deref().unwrap_or_default()),
            model: sanitize_text(session.model.as_deref().unwrap_or_default()),
            title: sanitize_text(session.title.as_deref().unwrap_or_default()),
            cwd: session
                .cwd
                .filter(|cwd| !cwd.chars().any(char::is_control) && cwd.starts_with('/'))
                .unwrap_or_default(),
        });
    }
    Ok(rows)
}

/// `http://host[:port][/base]`, the only URL shape the reader supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpUrl {
    host: String,
    port: u16,
    base_path: String,
}

impl HttpUrl {
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let rest = url
            .trim()
            .strip_prefix("http://")
            .ok_or_else(|| format!("not a plain http:// URL: {url}"))?;
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(format!("unsupported URL authority: {url}"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => {
                let port = port
                    .parse::<u16>()
                    .map_err(|_| format!("invalid port in {url}"))?;
                (host, port)
            }
            _ => (authority, 80),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() {
            return Err(format!("missing host in {url}"));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            base_path: path.trim_end_matches('/').to_owned(),
        })
    }

    fn connect(&self) -> io::Result<TcpStream> {
        let mut last_error = io::Error::new(io::ErrorKind::NotFound, "host did not resolve");
        for address in (self.host.as_str(), self.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    fn request(&self, path: &str, accept: &str) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!(
            "GET {}{} HTTP/1.0\r\nHost: {}:{}\r\nAccept: {}\r\nCache-Control: no-cache\r\nUser-Agent: herdr-threads\r\n\r\n",
            self.base_path, path, host, self.port, accept
        )
    }
}

/// Response head: status code and whether the body is chunked.
fn read_response_head(reader: &mut impl BufRead) -> io::Result<bool> {
    let status_line = read_line(reader)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed status line"))?;
    let mut chunked = false;
    loop {
        let line = read_line(reader)?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    if status != 200 {
        return Err(io::Error::other(format!("HTTP {status}")));
    }
    Ok(chunked)
}

/// One CRLF/LF-terminated line without its terminator. EOF before any byte is
/// an error so callers do not spin on a closed stream.
fn read_line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(MAX_LINE_BYTES as u64)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "connection closed",
        ));
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Decodes a chunked body as a plain byte stream.
struct ChunkedReader<R> {
    inner: R,
    remaining: usize,
    done: bool,
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let line = read_line(&mut self.inner)?;
            let size = line.split(';').next().unwrap_or_default().trim();
            let size = usize::from_str_radix(size, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
            if size == 0 {
                self.done = true;
                return Ok(0);
            }
            self.remaining = size;
        }
        let limit = buf.len().min(self.remaining);
        let read = self.inner.read(&mut buf[..limit])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "chunk truncated",
            ));
        }
        self.remaining -= read;
        if self.remaining == 0 {
            // The CRLF that ends the chunk.
            read_line(&mut self.inner)?;
        }
        Ok(read)
    }
}

fn body_reader(stream: TcpStream) -> io::Result<Box<dyn BufRead + Send>> {
    let mut reader = BufReader::new(stream);
    let chunked = read_response_head(&mut reader)?;
    Ok(if chunked {
        Box::new(BufReader::new(ChunkedReader {
            inner: reader,
            remaining: 0,
            done: false,
        }))
    } else {
        Box::new(reader)
    })
}

pub(crate) fn fetch_sessions(url: &HttpUrl) -> Result<Vec<ThreadRow>, String> {
    let mut stream = url.connect().map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(url.request("/api/sessions", "application/json").as_bytes())
        .map_err(|error| error.to_string())?;
    let mut body = Vec::new();
    body_reader(stream)
        .map_err(|error| format!("/api/sessions: {error}"))?
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    if body.len() > MAX_BODY_BYTES {
        return Err("thread list is too large".into());
    }
    parse_sessions(&body)
}

/// Incremental server-sent-events parser. Yields `(event, data)` per
/// dispatched event; comments and unknown fields are ignored.
#[derive(Default)]
pub(crate) struct SseParser {
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub(crate) fn line(&mut self, line: &str) -> Option<(String, String)> {
        if line.is_empty() {
            if self.data.is_empty() && self.event.is_empty() {
                return None;
            }
            let event = std::mem::take(&mut self.event);
            let data = std::mem::take(&mut self.data).join("\n");
            return Some((
                if event.is_empty() {
                    "message".into()
                } else {
                    event
                },
                data,
            ));
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
        None
    }
}

/// What the client renders: the last rows, and whether they are current.
#[derive(Debug, Clone, Default)]
pub(crate) struct FeedState {
    /// Bumped on every change; the client repaints when it moves.
    pub(crate) generation: u64,
    pub(crate) rows: Arc<Vec<ThreadRow>>,
    /// At least one fetch succeeded since the reader started.
    pub(crate) loaded: bool,
    /// The service is unreachable or failing; `rows` are the last known ones.
    pub(crate) stale: bool,
    pub(crate) error: Option<String>,
}

struct Reader {
    url: String,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<FeedState>>,
}

static READER: Mutex<Option<Reader>> = Mutex::new(None);

/// Starts, restarts, or stops the process-wide reader so it follows `url`.
/// Empty `url` stops it.
pub(crate) fn ensure_reader(url: &str) {
    let Ok(mut reader) = READER.lock() else {
        return;
    };
    if reader.as_ref().is_some_and(|reader| reader.url == url) {
        return;
    }
    if let Some(previous) = reader.take() {
        previous.stop.store(true, Ordering::Release);
    }
    if url.is_empty() {
        return;
    }
    let state = Arc::new(Mutex::new(FeedState {
        stale: true,
        ..FeedState::default()
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let parsed = HttpUrl::parse(url);
    let thread_state = Arc::clone(&state);
    let thread_stop = Arc::clone(&stop);
    let spawned = crate::thread_spawn::spawn_named("herdr-threads", move || match parsed {
        Ok(url) => run_reader(&url, &thread_stop, &thread_state),
        Err(error) => update(&thread_state, |state| state.error = Some(error)),
    });
    if let Err(error) = spawned {
        update(&state, |state| {
            state.error = Some(format!("could not start thread reader: {error}"))
        });
    }
    *reader = Some(Reader {
        url: url.to_owned(),
        stop,
        state,
    });
}

/// The current feed, or `None` while no reader runs.
pub(crate) fn feed_state() -> Option<FeedState> {
    let reader = READER.lock().ok()?;
    let state = reader.as_ref()?.state.lock().ok()?;
    Some(state.clone())
}

fn update(state: &Mutex<FeedState>, change: impl FnOnce(&mut FeedState)) {
    if let Ok(mut state) = state.lock() {
        change(&mut state);
        state.generation = state.generation.wrapping_add(1);
    }
}

fn mark_stale(state: &Mutex<FeedState>, error: String) {
    let unchanged = state
        .lock()
        .is_ok_and(|state| state.stale && state.error.as_deref() == Some(error.as_str()));
    if !unchanged {
        update(state, |state| {
            state.stale = true;
            state.error = Some(error);
        });
    }
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(200)));
    }
}

fn run_reader(url: &HttpUrl, stop: &AtomicBool, state: &Mutex<FeedState>) {
    let mut backoff = BACKOFF_START;
    while !stop.load(Ordering::Acquire) {
        match follow_events(url, stop, state, &mut backoff) {
            Ok(()) => return,
            Err(error) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                tracing::debug!(%error, "thread service unavailable");
                mark_stale(state, error);
            }
        }
        sleep_unless_stopped(backoff, stop);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Holds one event stream until it fails. `Ok` only when stopped.
fn follow_events(
    url: &HttpUrl,
    stop: &AtomicBool,
    state: &Mutex<FeedState>,
    backoff: &mut Duration,
) -> Result<(), String> {
    let mut stream = url.connect().map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(url.request("/api/events", "text/event-stream").as_bytes())
        .map_err(|error| error.to_string())?;
    let socket = stream.try_clone().map_err(|error| error.to_string())?;
    let mut reader = body_reader(stream).map_err(|error| format!("/api/events: {error}"))?;
    socket
        .set_read_timeout(Some(READ_TICK))
        .map_err(|error| error.to_string())?;
    *backoff = BACKOFF_START;
    let mut parser = SseParser::default();
    let mut last_stamp: Option<String> = None;
    let mut line = Vec::new();
    let mut last_byte_at = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return Err("event stream closed".into()),
            Ok(_) if !line.ends_with(b"\n") && line.len() < MAX_LINE_BYTES => {
                last_byte_at = Instant::now();
                continue;
            }
            Ok(_) => last_byte_at = Instant::now(),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if last_byte_at.elapsed() > STREAM_IDLE_LIMIT {
                    return Err("event stream idle".into());
                }
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        let text = String::from_utf8_lossy(&line).into_owned();
        line.clear();
        let Some((event, data)) = parser.line(&text) else {
            continue;
        };
        if event != "stamp" || last_stamp.as_deref() == Some(data.as_str()) {
            continue;
        }
        match fetch_sessions(url) {
            Ok(rows) => {
                last_stamp = Some(data);
                update(state, |state| {
                    state.rows = Arc::new(rows);
                    state.loaded = true;
                    state.stale = false;
                    state.error = None;
                });
            }
            // Keep the stream: the next stamp retries the fetch.
            Err(error) => mark_stale(state, error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    pub(crate) fn session_json(id: &str, ticket: Option<(&str, &str)>, status: &str) -> String {
        let ticket = ticket.map_or("null".to_owned(), |(id, label)| {
            format!(r#"{{"id":"{id}","label":"{label}","title":"Ticket {label}"}}"#)
        });
        format!(
            r#"{{"id":"{id}","ticket":{ticket},"status":"{status}","agent":"codex","model":"gpt-5","title":"Thread {id}","branch":null,"pr_url":null,"last_event":null,"last_event_at":null,"pending_kind":null,"summary":null,"cwd":"/home/dev/{id}","created_at":"2026-11-16T10:00:00Z","updated_at":"2026-11-16T10:00:00Z"}}"#
        )
    }

    #[test]
    fn threads_parse_sessions_reads_the_service_shape() {
        let body = format!(
            "[{},{}]",
            session_json("t1", Some(("7", "PROJ-7")), "blocked"),
            session_json("t2", None, "running")
        );
        let rows = parse_sessions(body.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "t1");
        assert_eq!(rows[0].status, ThreadStatus::Blocked);
        assert_eq!(rows[0].ticket.as_ref().unwrap().label, "PROJ-7");
        assert_eq!(rows[0].cwd, "/home/dev/t1");
        assert_eq!(rows[1].ticket, None);
        assert_eq!(
            rows[1].status.agent_status(),
            crate::api::schema::AgentStatus::Working
        );
    }

    #[test]
    fn threads_parse_sessions_skips_unusable_items_and_sanitizes_text() {
        let body = r#"[
            {"id":"bad id; rm -rf ~","status":"idle"},
            {"status":"idle"},
            42,
            {"id":"ok-1","status":"weird","title":"\u001b[31mred\u0007 \u202eevil\n\nnext","ticket":{"id":12,"label":"","title":"x"}},
            {"id":"ok-1","status":"idle"}
        ]"#;
        let rows = parse_sessions(body.as_bytes()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "[31mred evil next");
        assert_eq!(rows[0].status, ThreadStatus::Unknown);
        let ticket = rows[0].ticket.as_ref().unwrap();
        assert_eq!(ticket.id, "12");
        assert_eq!(ticket.label, "12");
        assert!(parse_sessions(b"{}").is_err());
    }

    #[test]
    fn threads_thread_ids_are_shell_safe() {
        assert!(valid_thread_id("0a1b-2c3d_e.f:g"));
        for bad in ["", "a b", "a;b", "$(x)", "a'b", "a\nb", &"x".repeat(129)] {
            assert!(!valid_thread_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn threads_url_parsing_accepts_plain_http_only() {
        let url = HttpUrl::parse("http://192.168.1.2:7420").unwrap();
        assert_eq!((url.host.as_str(), url.port), ("192.168.1.2", 7420));
        assert_eq!(url.base_path, "");
        let url = HttpUrl::parse("http://example/tkd/").unwrap();
        assert_eq!((url.host.as_str(), url.port), ("example", 80));
        assert_eq!(url.base_path, "/tkd");
        assert!(url
            .request("/api/sessions", "x")
            .starts_with("GET /tkd/api/sessions HTTP/1.0\r\n"));
        assert!(HttpUrl::parse("https://example").is_err());
        assert!(HttpUrl::parse("http://user@example").is_err());
        assert!(HttpUrl::parse("http://example:port").is_err());
    }

    #[test]
    fn threads_sse_parser_dispatches_on_blank_lines() {
        let mut parser = SseParser::default();
        assert_eq!(parser.line(": keepalive"), None);
        assert_eq!(parser.line("event: stamp"), None);
        assert_eq!(parser.line("data: 41"), None);
        assert_eq!(parser.line(""), Some(("stamp".into(), "41".into())));
        assert_eq!(parser.line(""), None);
        assert_eq!(parser.line("data:x"), None);
        assert_eq!(parser.line(""), Some(("message".into(), "x".into())));
    }

    #[test]
    fn threads_chunked_bodies_decode() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n[{\"i\r\n3\r\nd\":\r\n6\r\n\"a\"}]\n\r\n0\r\n\r\n";
        let mut reader = BufReader::new(&raw[..]);
        assert!(read_response_head(&mut reader).unwrap());
        let mut decoded = String::new();
        ChunkedReader {
            inner: reader,
            remaining: 0,
            done: false,
        }
        .read_to_string(&mut decoded)
        .unwrap();
        assert_eq!(decoded, "[{\"id\":\"a\"}]\n");
        let mut not_found = BufReader::new(&b"HTTP/1.0 404 Not Found\r\n\r\n"[..]);
        assert!(read_response_head(&mut not_found).is_err());
    }

    /// Minimal stand-in for the thread service: `/api/sessions` answers with
    /// the current body, `/api/events` sends one stamp on connect and one per
    /// `bump`. Dropping `streams` closes every open event stream.
    pub(crate) struct StubService {
        pub(crate) url: String,
        pub(crate) body: Arc<Mutex<String>>,
        stamp: Arc<Mutex<u64>>,
        streams: Arc<Mutex<Vec<TcpStream>>>,
        pub(crate) listener_alive: Arc<AtomicBool>,
    }

    impl StubService {
        pub(crate) fn start(body: String) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let body = Arc::new(Mutex::new(body));
            let stamp = Arc::new(Mutex::new(1));
            let streams = Arc::new(Mutex::new(Vec::new()));
            let listener_alive = Arc::new(AtomicBool::new(true));
            let (thread_body, thread_stamp, thread_streams, alive) = (
                Arc::clone(&body),
                Arc::clone(&stamp),
                Arc::clone(&streams),
                Arc::clone(&listener_alive),
            );
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    if !alive.load(Ordering::Acquire) {
                        continue;
                    }
                    let mut request = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    let _ = request.read_line(&mut first);
                    loop {
                        let mut header = String::new();
                        if request.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                            break;
                        }
                    }
                    if first.starts_with("GET /api/sessions") {
                        let body = thread_body.lock().unwrap().clone();
                        let _ = write!(
                            stream,
                            "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{body}"
                        );
                    } else if first.starts_with("GET /api/events") {
                        let stamp = *thread_stamp.lock().unwrap();
                        let _ = write!(
                            stream,
                            "HTTP/1.0 200 OK\r\nContent-Type: text/event-stream\r\n\r\nevent: stamp\ndata: {stamp}\n\n"
                        );
                        thread_streams.lock().unwrap().push(stream);
                    } else {
                        let _ = write!(stream, "HTTP/1.0 404 Not Found\r\n\r\n");
                    }
                }
            });
            Self {
                url,
                body,
                stamp,
                streams,
                listener_alive,
            }
        }

        pub(crate) fn bump(&self) {
            let stamp = {
                let mut stamp = self.stamp.lock().unwrap();
                *stamp += 1;
                *stamp
            };
            for stream in self.streams.lock().unwrap().iter_mut() {
                let _ = write!(stream, "event: stamp\ndata: {stamp}\n\n");
            }
        }

        /// Closes open streams and refuses new requests.
        pub(crate) fn go_down(&self) {
            self.listener_alive.store(false, Ordering::Release);
            for stream in self.streams.lock().unwrap().drain(..) {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }

        pub(crate) fn come_up(&self) {
            self.listener_alive.store(true, Ordering::Release);
        }
    }

    fn wait_for(mut condition: impl FnMut(&FeedState) -> bool) -> FeedState {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(state) = feed_state() {
                if condition(&state) {
                    return state;
                }
            }
            assert!(
                Instant::now() < deadline,
                "feed did not reach the state in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn threads_reader_follows_stamps_and_keeps_rows_while_down() {
        let service = StubService::start(format!("[{}]", session_json("t1", None, "idle")));
        ensure_reader(&service.url);
        let state = wait_for(|state| state.loaded);
        assert!(!state.stale);
        assert_eq!(state.rows.len(), 1);

        *service.body.lock().unwrap() = format!(
            "[{},{}]",
            session_json("t2", None, "blocked"),
            session_json("t1", None, "idle")
        );
        service.bump();
        let state = wait_for(|state| state.rows.len() == 2);
        assert_eq!(state.rows[0].id, "t2");

        service.go_down();
        let state = wait_for(|state| state.stale);
        assert_eq!(state.rows.len(), 2, "rows survive an outage");

        service.come_up();
        let state = wait_for(|state| !state.stale);
        assert_eq!(state.rows.len(), 2);

        ensure_reader("");
        assert!(feed_state().is_none());
    }

    #[test]
    fn threads_reader_reports_an_unreachable_service_as_stale() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        ensure_reader(&url);
        let state = wait_for(|state| state.error.is_some());
        assert!(state.stale);
        assert!(!state.loaded);
        assert!(state.rows.is_empty());
    }
}
