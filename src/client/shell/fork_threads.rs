//! fork: agent thread list in the sidebar (FORK.md 0005).
//!
//! `[ui.threads] url = "http://…"` names a thread service. A background
//! reader (`fork_threads_feed`) keeps its thread list current; this module
//! draws it as a block of rows at the top of the expanded sidebar, below the
//! pinned Main row, grouped by ticket with a "no ticket" group last.
//!
//! What the sidebar draws lives in a process-wide view, like the hidden label
//! of FORK.md 0004, so the upstream render call sites stay untouched. The
//! client has one shell per process; nextest runs each test in its own.

use super::fork_threads_feed::{ThreadRow, ThreadStatus};
use super::*;
use std::sync::{Arc, LazyLock, RwLock};

/// Smallest sidebar that still gets a thread block.
const MIN_SIDEBAR_ROWS: u16 = 12;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClientThreads {
    pub(super) url: String,
    pub(super) machine: String,
    pub(super) command: String,
}

impl ClientThreads {
    /// Builds the client view of `[ui.threads]` and (re)starts or stops the
    /// reader to match. Returns a diagnostic when the config cannot work.
    pub(super) fn from_config(config: &crate::config::ThreadsConfig) -> (Self, Option<String>) {
        let diagnostic = config.diagnostic();
        let threads = if config.enabled() && diagnostic.is_none() {
            Self {
                url: config.url.trim().trim_end_matches('/').to_owned(),
                machine: config.machine.trim().to_owned(),
                command: config.command.trim().to_owned(),
            }
        } else {
            Self::default()
        };
        super::fork_threads_feed::ensure_reader(&threads.url);
        if let Ok(mut view) = VIEW.write() {
            view.enabled = threads.enabled();
            if !view.enabled {
                *view = ThreadsView::default();
            }
        }
        (threads, diagnostic)
    }

    pub(super) fn enabled(&self) -> bool {
        !self.url.is_empty()
    }
}

/// What the sidebar shows, refreshed from the reader on the client timer.
#[derive(Debug, Clone, Default)]
pub(super) struct ThreadsView {
    pub(super) enabled: bool,
    pub(super) generation: Option<u64>,
    pub(super) rows: Arc<Vec<ThreadRow>>,
    pub(super) loaded: bool,
    pub(super) stale: bool,
    pub(super) error: Option<String>,
    /// Keyboard cursor, an index into `display_order`, while navigating.
    pub(super) cursor: Option<usize>,
}

static VIEW: LazyLock<RwLock<ThreadsView>> = LazyLock::new(Default::default);

pub(super) fn view() -> ThreadsView {
    VIEW.read().map(|view| view.clone()).unwrap_or_default()
}

/// Test seam: what the reader would have delivered.
#[cfg(test)]
pub(super) fn set_view_for_test(rows: Vec<ThreadRow>, stale: bool) {
    if let Ok(mut view) = VIEW.write() {
        view.rows = Arc::new(rows);
        view.loaded = true;
        view.stale = stale;
        view.error = stale.then(|| "down".into());
    }
}

/// Pulls the reader's latest state into the view. True when it changed.
pub(super) fn refresh_view() -> bool {
    let Some(feed) = super::fork_threads_feed::feed_state() else {
        return false;
    };
    let Ok(mut view) = VIEW.write() else {
        return false;
    };
    if !view.enabled || view.generation == Some(feed.generation) {
        return false;
    }
    view.generation = Some(feed.generation);
    view.rows = feed.rows;
    view.loaded = feed.loaded;
    view.stale = feed.stale;
    view.error = feed.error;
    let count = view.rows.len();
    if let Some(cursor) = view.cursor.as_mut() {
        if count == 0 {
            view.cursor = None;
        } else {
            *cursor = (*cursor).min(count - 1);
        }
    }
    true
}

/// One line of the thread block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ThreadLine {
    /// Ticket group header; `None` is the "no ticket" group.
    Group(Option<usize>),
    /// A thread, by index into the row list.
    Thread(usize),
}

/// Lines in display order: ticket groups in order of their newest thread,
/// threads newest first within a group, the "no ticket" group last.
pub(super) fn thread_lines(rows: &[ThreadRow]) -> Vec<ThreadLine> {
    let mut groups: Vec<(Option<&str>, usize, Vec<usize>)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let key = row.ticket.as_ref().map(|ticket| ticket.id.as_str());
        match groups.iter_mut().find(|(group, _, _)| *group == key) {
            Some((_, _, members)) => members.push(index),
            None => groups.push((key, index, vec![index])),
        }
    }
    groups.sort_by_key(|(key, _, _)| key.is_none());
    let mut lines = Vec::new();
    for (key, first, members) in groups {
        lines.push(ThreadLine::Group(key.map(|_| first)));
        lines.extend(members.into_iter().map(ThreadLine::Thread));
    }
    lines
}

/// Row indices in the order the block shows them; the keyboard cursor walks
/// this list.
pub(super) fn display_order(rows: &[ThreadRow]) -> Vec<usize> {
    thread_lines(rows)
        .into_iter()
        .filter_map(|line| match line {
            ThreadLine::Thread(index) => Some(index),
            ThreadLine::Group(_) => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ThreadHit {
    pub(super) rect: Rect,
    pub(super) thread_id: String,
}

/// Rows the block may use: about two fifths of the sidebar, so spaces and
/// agents keep room.
fn block_limit(area: Rect) -> u16 {
    (area.height.saturating_mul(2) / 5).max(4)
}

/// Draws the thread block at the top of `area` and returns the rest of the
/// sidebar. Returns `area` unchanged when the feature is off or the sidebar
/// is too small.
pub(super) fn render_thread_block(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    shown_thread: Option<&str>,
    hits: &mut ShellHitMap,
) -> Rect {
    hits.fork_threads.clear();
    if !config.threads.enabled() || area.height < MIN_SIDEBAR_ROWS || area.width < 8 {
        return area;
    }
    let view = view();
    let palette = &config.palette;
    let lines = thread_lines(&view.rows);
    let order = display_order(&view.rows);
    let cursor_row = view.cursor.and_then(|cursor| order.get(cursor).copied());
    // Header, lines, and a spacer row.
    let limit = block_limit(area);
    let body_rows = (lines.len() as u16).max(1).min(limit.saturating_sub(2));
    let height = body_rows + 2;
    // The last column is the sidebar divider.
    let width = area.width.saturating_sub(1);

    super::render::put_text(
        buffer,
        area.x,
        area.y,
        width,
        " threads",
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let state = if !view.loaded && view.error.is_none() {
        Some("loading")
    } else if view.stale {
        Some("offline")
    } else {
        None
    };
    if let Some(state) = state {
        super::render::put_right_text(
            buffer,
            Rect::new(area.x, area.y, width.saturating_sub(1), 1),
            area.y,
            state,
            Style::default().fg(palette.overlay0),
        );
    }

    let body = Rect::new(area.x, area.y + 1, width, body_rows);
    if lines.is_empty() {
        let text = if view.loaded { " no threads" } else { "" };
        super::render::put_text(
            buffer,
            body.x,
            body.y,
            body.width,
            text,
            Style::default().fg(palette.overlay0),
        );
    } else {
        let cursor_line = cursor_row.and_then(|row| {
            lines
                .iter()
                .position(|line| *line == ThreadLine::Thread(row))
        });
        let visible = body_rows as usize;
        let offset = cursor_line.map_or(0, |line| (line + 1).saturating_sub(visible));
        let hidden_below = lines.len().saturating_sub(offset + visible);
        for (slot, line) in lines.iter().skip(offset).take(visible).enumerate() {
            let rect = Rect::new(body.x, body.y + slot as u16, body.width, 1);
            if hidden_below > 0 && slot + 1 == visible && cursor_line != Some(offset + slot) {
                super::render::put_text(
                    buffer,
                    rect.x + 1,
                    rect.y,
                    rect.width.saturating_sub(1),
                    &format!("… {hidden_below} more"),
                    Style::default().fg(palette.overlay0),
                );
                break;
            }
            match line {
                ThreadLine::Group(first) => {
                    render_group_line(buffer, rect, first.map(|index| &view.rows[index]), palette)
                }
                ThreadLine::Thread(index) => {
                    let row = &view.rows[*index];
                    let shown = shown_thread == Some(row.id.as_str());
                    let selected = cursor_row == Some(*index);
                    render_thread_line(buffer, rect, row, shown, selected, view.stale, config);
                    if config.mouse_capture {
                        hits.fork_threads.push(ThreadHit {
                            rect,
                            thread_id: row.id.clone(),
                        });
                    }
                }
            }
        }
    }
    Rect::new(
        area.x,
        area.y.saturating_add(height),
        area.width,
        area.height.saturating_sub(height),
    )
}

/// Sidebar hook: draws the thread block for whichever sidebar is expanded.
pub(super) fn render_sidebar_threads(
    buffer: &mut Buffer,
    area: Rect,
    active_snapshot: Option<&ClientShellSnapshot>,
    config: &ClientShellConfig,
    state: &super::render::ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) -> Rect {
    let _ = (active_snapshot, state);
    render_thread_block(buffer, area, config, None, hits)
}

fn render_group_line(
    buffer: &mut Buffer,
    rect: Rect,
    first: Option<&ThreadRow>,
    palette: &Palette,
) {
    let ticket = first.and_then(|row| row.ticket.as_ref());
    let label = ticket.map_or("no ticket", |ticket| ticket.label.as_str());
    let x = rect.x + 1;
    super::render::put_text(
        buffer,
        x,
        rect.y,
        rect.width.saturating_sub(1),
        label,
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let used = super::render::display_width(label).saturating_add(2);
    if let Some(title) = ticket
        .map(|ticket| ticket.title.as_str())
        .filter(|title| !title.is_empty())
    {
        super::render::put_text(
            buffer,
            x.saturating_add(used),
            rect.y,
            rect.width.saturating_sub(1).saturating_sub(used),
            title,
            Style::default().fg(palette.overlay0),
        );
    }
}

fn render_thread_line(
    buffer: &mut Buffer,
    rect: Rect,
    row: &ThreadRow,
    shown: bool,
    selected: bool,
    stale: bool,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    if selected {
        buffer.set_style(rect, Style::default().bg(palette.selection_bg));
    } else if shown {
        buffer.set_style(
            rect,
            Style::default().bg(super::sidebar::workspace_active_background(palette, false)),
        );
    }
    let status = row.status.agent_status();
    let dim = palette.overlay0;
    let icon_color = if stale {
        dim
    } else {
        status_color(status, palette)
    };
    let icon = if row.status == ThreadStatus::Stopped {
        "·"
    } else {
        status_icon(status, config.status_indicators)
    };
    super::render::put_text(
        buffer,
        rect.x + 2,
        rect.y,
        2,
        icon,
        Style::default().fg(icon_color),
    );
    let attention = row.status.needs_attention();
    let title_style = if stale {
        Style::default().fg(dim)
    } else if attention {
        Style::default()
            .fg(palette.red)
            .add_modifier(Modifier::BOLD)
    } else if shown || selected {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.subtext0)
    };
    let title_x = rect.x + 4;
    let available = rect.right().saturating_sub(title_x).saturating_sub(1);
    // Agent and model on the right when there is room for both columns.
    let detail = thread_detail(row);
    let detail_width = if available >= 28 && !detail.is_empty() {
        super::render::display_width(&detail).min(available / 3)
    } else {
        0
    };
    let title_width = if detail_width > 0 {
        available.saturating_sub(detail_width + 1)
    } else {
        available
    };
    let title = if attention {
        format!("{} [{}]", row.display_title(), row.status.label())
    } else {
        row.display_title().to_owned()
    };
    super::render::put_text(buffer, title_x, rect.y, title_width, &title, title_style);
    if detail_width > 0 {
        let detail = truncate_to_width(&detail, detail_width);
        super::render::put_right_text(
            buffer,
            Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), 1),
            rect.y,
            &detail,
            Style::default().fg(dim),
        );
    }
}

fn thread_detail(row: &ThreadRow) -> String {
    match (row.agent.is_empty(), row.model.is_empty()) {
        (false, false) => format!("{}·{}", row.agent, row.model),
        (false, true) => row.agent.clone(),
        (true, false) => row.model.clone(),
        (true, true) => String::new(),
    }
}

fn truncate_to_width(text: &str, width: u16) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + char_width > width as usize {
            break;
        }
        used += char_width;
        out.push(character);
    }
    out
}

impl ClientShellState {
    /// Client timer hook: picks up a new thread list. True when the sidebar
    /// needs a repaint.
    pub(crate) fn fork_threads_tick(&mut self) -> bool {
        self.config.threads.enabled() && refresh_view()
    }
}
