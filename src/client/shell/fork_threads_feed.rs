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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadTicket {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) title: String,
}

/// Row status of the thread inbox (t3code `resolveSidebarThreadStatus`),
/// shipped precomputed as `inbox.status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RowStatus {
    Approval,
    Input,
    Request,
    Waiting,
    Preparing,
    Working,
    Limited,
    Failed,
    #[default]
    Ready,
}

impl RowStatus {
    fn parse(value: &str) -> Self {
        match value {
            "approval" => Self::Approval,
            "input" => Self::Input,
            "request" => Self::Request,
            "waiting" => Self::Waiting,
            "preparing" => Self::Preparing,
            "working" => Self::Working,
            "limited" => Self::Limited,
            "failed" => Self::Failed,
            _ => Self::Ready,
        }
    }

    /// Statuses that ask for Jacob: drawn in herdr's blocked style.
    pub(crate) fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::Approval | Self::Input | Self::Request | Self::Limited | Self::Failed
        )
    }
}

/// The `inbox` object of a session: the inputs of the thread inbox rules
/// (sections, order, row status). Times are Unix milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ThreadInbox {
    pub(crate) listed: bool,
    pub(crate) pinned: bool,
    pub(crate) settled: bool,
    pub(crate) raised_hand: bool,
    pub(crate) working: bool,
    pub(crate) snoozed_until: Option<i64>,
    pub(crate) raised_hand_woke_at: Option<i64>,
    pub(crate) pin_order_key: Option<String>,
    pub(crate) created_at: Option<i64>,
    pub(crate) inbox_sort_at: Option<i64>,
    pub(crate) working_sort_at: Option<i64>,
    pub(crate) settled_sort_at: Option<i64>,
    pub(crate) activity_at: Option<i64>,
    pub(crate) status: RowStatus,
    pub(crate) goal_active: bool,
    pub(crate) unread: bool,
    pub(crate) last_visited_at: Option<i64>,
    pub(crate) working_started_at: Option<i64>,
    pub(crate) waiting_since: Option<i64>,
    pub(crate) project_title: String,
    pub(crate) pr: Option<ThreadPr>,
    /// False when the service sent no `inbox` and these fields were derived
    /// from the top-level ones.
    pub(crate) from_service: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ThreadPr {
    pub(crate) number: Option<u64>,
    /// `open`, `closed`, `merged`, or empty when unknown.
    pub(crate) state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadRow {
    pub(crate) id: String,
    pub(crate) ticket: Option<ThreadTicket>,
    /// The coarse top-level status (idle|running|blocked|done|stopped|error).
    pub(crate) status: ThreadStatus,
    pub(crate) agent: String,
    pub(crate) model: String,
    pub(crate) title: String,
    pub(crate) cwd: String,
    pub(crate) branch: String,
    pub(crate) inbox: ThreadInbox,
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
    branch: Option<String>,
    pending_kind: Option<String>,
    created_at: Option<serde_json::Value>,
    updated_at: Option<serde_json::Value>,
    inbox: Option<serde_json::Value>,
}

fn ticket_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(id) => Some(sanitize_text(id)),
        serde_json::Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
    .filter(|id| !id.is_empty())
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// RFC 3339 / ISO 8601 date-time (`2026-11-16T10:00:00.123Z`, offsets,
/// date only) to Unix milliseconds.
pub(crate) fn parse_iso_millis(value: &str) -> Option<i64> {
    fn number(text: &str, range: std::ops::Range<usize>) -> Option<i64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())?
    }
    let text = value.trim();
    let year = number(text, 0..4)?;
    let month = number(text, 5..7)?;
    let day = number(text, 8..10)?;
    if text.get(4..5)? != "-" || text.get(7..8)? != "-" {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut rest = &text[10..];
    let (mut hour, mut minute, mut second, mut millis) = (0, 0, 0, 0);
    if let Some(time) = rest
        .strip_prefix('T')
        .or_else(|| rest.strip_prefix('t'))
        .or_else(|| rest.strip_prefix(' '))
    {
        hour = number(time, 0..2)?;
        minute = number(time, 3..5)?;
        if time.get(2..3)? != ":" {
            return None;
        }
        rest = &time[5..];
        if let Some(seconds) = rest.strip_prefix(':') {
            second = number(seconds, 0..2)?;
            rest = &seconds[2..];
            if let Some(fraction) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(',')) {
                let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
                if digits == 0 {
                    return None;
                }
                let scaled = format!("{:0<3}", &fraction[..digits.min(3)]);
                millis = scaled.parse().ok()?;
                rest = &fraction[digits..];
            }
        }
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
    }
    let offset_minutes = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = rest[1..].replace(':', "");
            let hours = number(&zone, 0..2)?;
            let minutes = if zone.len() > 2 {
                number(&zone, 2..4)?
            } else {
                0
            };
            if zone.len() != 2 && zone.len() != 4 {
                return None;
            }
            sign * (hours * 60 + minutes)
        }
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1_000 + millis)
}

/// A service timestamp: ISO string, or a number of Unix milliseconds.
fn timestamp(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::String(text) => parse_iso_millis(text),
        serde_json::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value as i64)),
        _ => None,
    }
}

fn flag(inbox: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<bool> {
    inbox.get(key).and_then(serde_json::Value::as_bool)
}

fn text(inbox: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    inbox
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(sanitize_text)
}

/// Reads the `inbox` object leniently: wrong types count as missing, and
/// missing fields take their defaults.
fn parse_inbox(inbox: &serde_json::Map<String, serde_json::Value>) -> ThreadInbox {
    let time = |key: &str| timestamp(inbox.get(key));
    let pr = inbox
        .get("pr")
        .and_then(serde_json::Value::as_object)
        .map(|pr| ThreadPr {
            number: pr.get("number").and_then(serde_json::Value::as_u64),
            state: text(pr, "state")
                .filter(|state| matches!(state.as_str(), "open" | "closed" | "merged"))
                .unwrap_or_default(),
        });
    ThreadInbox {
        listed: flag(inbox, "listed").unwrap_or(true),
        pinned: flag(inbox, "pinned").unwrap_or(false),
        settled: flag(inbox, "settled").unwrap_or(false),
        raised_hand: flag(inbox, "raised_hand").unwrap_or(false),
        working: flag(inbox, "working").unwrap_or(false),
        snoozed_until: time("snoozed_until"),
        raised_hand_woke_at: time("raised_hand_woke_at"),
        pin_order_key: inbox
            .get("pin_order_key")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        created_at: time("created_at"),
        inbox_sort_at: time("inbox_sort_at"),
        working_sort_at: time("working_sort_at"),
        settled_sort_at: time("settled_sort_at"),
        activity_at: time("activity_at"),
        status: RowStatus::parse(
            inbox
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        ),
        goal_active: flag(inbox, "goal_active").unwrap_or(false),
        unread: flag(inbox, "unread").unwrap_or(false),
        last_visited_at: time("last_visited_at"),
        working_started_at: time("working_started_at"),
        waiting_since: time("waiting_since"),
        project_title: text(inbox, "project_title").unwrap_or_default(),
        pr,
        from_service: true,
    }
}

/// The inbox of a service without the `inbox` object: listed, in no shelf
/// but Working (top-level `running`), ordered by `updated_at`.
fn fallback_inbox(
    status: ThreadStatus,
    pending_kind: Option<&str>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
) -> ThreadInbox {
    let row_status = match status {
        ThreadStatus::Running => RowStatus::Working,
        ThreadStatus::Blocked
            if pending_kind.is_some_and(|kind| kind.contains("input") || kind == "question") =>
        {
            RowStatus::Input
        }
        ThreadStatus::Blocked => RowStatus::Approval,
        ThreadStatus::Error => RowStatus::Failed,
        _ => RowStatus::Ready,
    };
    ThreadInbox {
        listed: true,
        working: status == ThreadStatus::Running,
        created_at,
        inbox_sort_at: updated_at,
        working_sort_at: updated_at,
        settled_sort_at: updated_at,
        activity_at: updated_at,
        status: row_status,
        unread: status == ThreadStatus::Done,
        ..ThreadInbox::default()
    }
}

/// Parses `GET /api/sessions`: a JSON array. Unknown fields are ignored;
/// malformed items and items with unusable ids are skipped.
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
        let status = ThreadStatus::parse(session.status.as_deref().unwrap_or_default());
        let created_at = timestamp(session.created_at.as_ref());
        let mut inbox = match session
            .inbox
            .as_ref()
            .and_then(serde_json::Value::as_object)
        {
            Some(inbox) => parse_inbox(inbox),
            None => fallback_inbox(
                status,
                session.pending_kind.as_deref(),
                created_at,
                timestamp(session.updated_at.as_ref()),
            ),
        };
        inbox.created_at = inbox.created_at.or(created_at);
        rows.push(ThreadRow {
            id,
            ticket,
            status,
            agent: sanitize_text(session.agent.as_deref().unwrap_or_default()),
            model: sanitize_text(session.model.as_deref().unwrap_or_default()),
            title: sanitize_text(session.title.as_deref().unwrap_or_default()),
            cwd: session
                .cwd
                .filter(|cwd| !cwd.chars().any(char::is_control) && cwd.starts_with('/'))
                .unwrap_or_default(),
            branch: sanitize_text(session.branch.as_deref().unwrap_or_default()),
            inbox,
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
        assert_eq!(rows[1].status, ThreadStatus::Running);
    }

    #[test]
    fn threads_iso_timestamps_parse_to_unix_millis() {
        assert_eq!(parse_iso_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_millis("2026-11-16T10:00:00Z"),
            Some(1_794_823_200_000)
        );
        assert_eq!(
            parse_iso_millis("2026-11-16T10:00:00.5Z"),
            Some(1_794_823_200_500)
        );
        assert_eq!(
            parse_iso_millis("2026-11-16T11:30:00.123456+01:30"),
            Some(1_794_823_200_123)
        );
        assert_eq!(
            parse_iso_millis("2026-11-16T05:00:00-0500"),
            Some(1_794_823_200_000)
        );
        assert_eq!(parse_iso_millis("2026-11-16"), Some(1_794_787_200_000));
        assert_eq!(
            parse_iso_millis("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        for bad in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-11-16T10:00Q",
            "2026-11-16T25:00:00Z",
        ] {
            assert_eq!(parse_iso_millis(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn threads_parse_sessions_reads_the_inbox_object() {
        let body = r#"[{
            "id":"t1","status":"blocked","title":"T","branch":"feat/x","created_at":"2026-11-16T09:00:00Z",
            "inbox":{
                "listed":true,"pinned":true,"settled":false,"raised_hand":true,"working":false,
                "snoozed_until":"2026-11-16T12:00:00Z","raised_hand_woke_at":"2026-11-16T10:00:00Z",
                "pin_order_key":"a0","inbox_sort_at":1794823200000,"working_sort_at":null,
                "settled_sort_at":"bogus","activity_at":"2026-11-16T10:00:00Z",
                "status":"approval","goal_active":"yes","unread":true,
                "last_visited_at":null,"working_started_at":null,"waiting_since":null,
                "project_id":"p1","project_title":"Proj \u001b[1mX","pr":{"number":12,"url":"u","state":"merged"},
                "future_field":{"x":1}
            }
        },{"id":"t2","status":"running","updated_at":"2026-11-16T10:00:00Z","inbox":{}}]"#;
        let rows = parse_sessions(body.as_bytes()).unwrap();
        let inbox = &rows[0].inbox;
        assert!(inbox.from_service && inbox.listed && inbox.pinned && inbox.raised_hand);
        assert_eq!(inbox.status, RowStatus::Approval);
        assert_eq!(inbox.snoozed_until, Some(1_794_823_200_000 + 2 * 3_600_000));
        assert_eq!(inbox.inbox_sort_at, Some(1_794_823_200_000));
        assert_eq!(
            inbox.settled_sort_at, None,
            "unparseable times count as missing"
        );
        assert!(!inbox.goal_active, "wrong types fall back to the default");
        assert!(inbox.unread);
        assert_eq!(inbox.pin_order_key.as_deref(), Some("a0"));
        assert_eq!(
            inbox.created_at,
            Some(1_794_819_600_000),
            "top-level created_at"
        );
        assert_eq!(inbox.project_title, "Proj [1mX");
        assert_eq!(
            inbox.pr,
            Some(ThreadPr {
                number: Some(12),
                state: "merged".into()
            })
        );
        assert_eq!(rows[0].branch, "feat/x");
        // An empty inbox: listed, everything else default.
        let empty = &rows[1].inbox;
        assert!(empty.from_service && empty.listed && !empty.working);
        assert_eq!(empty.status, RowStatus::Ready);
    }

    #[test]
    fn threads_parse_sessions_without_inbox_falls_back_to_top_level_fields() {
        let body = format!(
            "[{},{},{},{}]",
            session_json("t1", None, "running"),
            session_json("t2", None, "blocked")
                .replace(r#""pending_kind":null"#, r#""pending_kind":"user_input""#),
            session_json("t3", None, "error"),
            session_json("t4", None, "done")
        );
        let rows = parse_sessions(body.as_bytes()).unwrap();
        let updated = Some(1_794_823_200_000);
        for row in &rows {
            assert!(!row.inbox.from_service && row.inbox.listed);
            assert!(!row.inbox.pinned && !row.inbox.settled && row.inbox.snoozed_until.is_none());
            assert_eq!(row.inbox.inbox_sort_at, updated);
            assert_eq!(row.inbox.activity_at, updated);
        }
        assert!(rows[0].inbox.working);
        assert_eq!(rows[0].inbox.status, RowStatus::Working);
        assert_eq!(rows[1].inbox.status, RowStatus::Input);
        assert_eq!(rows[2].inbox.status, RowStatus::Failed);
        assert_eq!(rows[3].inbox.status, RowStatus::Ready);
        assert!(rows[3].inbox.unread);
        assert!(!rows[1].inbox.working);
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
            session_json("t2", None, "blocked").replace(
                r#""summary":null"#,
                r#""summary":null,"inbox":{"listed":true,"settled":true,"status":"input"}"#
            ),
            session_json("t1", None, "idle")
        );
        service.bump();
        let state = wait_for(|state| state.rows.len() == 2);
        assert_eq!(state.rows[0].id, "t2");
        assert!(state.rows[0].inbox.from_service && state.rows[0].inbox.settled);
        assert_eq!(state.rows[0].inbox.status, RowStatus::Input);
        assert!(!state.rows[1].inbox.from_service);

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
