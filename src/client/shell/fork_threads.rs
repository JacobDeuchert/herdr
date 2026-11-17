//! fork: agent thread list in the sidebar (FORK.md 0005).
//!
//! `[ui.threads] url = "http://…"` names a thread service. A background
//! reader (`fork_threads_feed`) keeps its thread list current; this module
//! draws it in place of herdr's agents section, below the machines/spaces
//! list, as t3code's thread inbox (`fork_threads_inbox`): pinned and active
//! rows, then the Working, Snoozed and Settled shelves. It also takes over
//! every input the agents section answers to: the `next_agent`,
//! `previous_agent` and `focus_agent` bindings, clicks, the wheel and the
//! scrollbar (through the agents section's own hit areas and scroll offset),
//! and a navigate-mode cursor.
//!
//! What the sidebar draws lives in a process-wide view, like the hidden label
//! of FORK.md 0004, so the upstream render call sites stay untouched. The
//! client has one shell per process; nextest runs each test in its own.

use super::fork_threads_feed::{RowStatus, ThreadRow};
use super::fork_threads_inbox::{self as inbox, InboxLine, RowLabel, Section, Shelves};
use super::*;
use std::sync::{Arc, LazyLock, RwLock};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClientThreads {
    pub(super) url: String,
    pub(super) machine: String,
    pub(super) template: CommandTemplate,
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
                template: CommandTemplate::parse(&config.command).unwrap_or_default(),
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
            view.template = threads.template.clone();
        }
        (threads, diagnostic)
    }

    pub(super) fn enabled(&self) -> bool {
        !self.url.is_empty()
    }
}

/// What the panel shows, refreshed from the reader on the client timer.
#[derive(Debug, Clone, Default)]
pub(super) struct ThreadsView {
    pub(super) enabled: bool,
    pub(super) generation: Option<u64>,
    pub(super) rows: Arc<Vec<ThreadRow>>,
    pub(super) loaded: bool,
    pub(super) stale: bool,
    pub(super) error: Option<String>,
    /// Navigate-mode cursor, an index into the panel's lines.
    pub(super) cursor: Option<usize>,
    pub(super) template: CommandTemplate,
    /// Ids of listed rows, for hiding workspaces that own a listed thread.
    pub(super) listed: std::collections::HashSet<String>,
    /// Shelf open/closed state and the Settled page size (client-local).
    pub(super) shelves: Shelves,
    /// Scroll the next render so this comes into view.
    pub(super) reveal: Option<Reveal>,
    /// When a clock-driven label or section changes next (Unix ms).
    pub(super) clock_due: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Reveal {
    Cursor,
    Thread(String),
}

static VIEW: LazyLock<RwLock<ThreadsView>> = LazyLock::new(Default::default);

pub(super) fn view() -> ThreadsView {
    VIEW.read().map(|view| view.clone()).unwrap_or_default()
}

fn listed_ids(rows: &[ThreadRow]) -> std::collections::HashSet<String> {
    rows.iter()
        .filter(|row| row.inbox.listed)
        .map(|row| row.id.clone())
        .collect()
}

/// Test seam: what the reader would have delivered.
#[cfg(test)]
pub(super) fn set_view_for_test(rows: Vec<ThreadRow>, stale: bool) {
    if let Ok(mut view) = VIEW.write() {
        view.listed = listed_ids(&rows);
        view.rows = Arc::new(rows);
        view.loaded = true;
        view.stale = stale;
        view.error = stale.then(|| "down".into());
        view.shelves = Default::default();
        view.cursor = None;
        view.reveal = None;
    }
}

pub(super) fn update_view(change: impl FnOnce(&mut ThreadsView)) {
    if let Ok(mut view) = VIEW.write() {
        change(&mut view);
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
    view.listed = listed_ids(&feed.rows);
    view.rows = feed.rows;
    view.loaded = feed.loaded;
    view.stale = feed.stale;
    view.error = feed.error;
    if view.listed.is_empty() {
        view.cursor = None;
    }
    true
}

/// Re-evaluates the clock: true when a clock-driven part of the panel (a
/// snooze ending, a relative time or duration) changed since the last
/// check. `changed` forces a new deadline after a new list.
fn clock_tick(now: i64, changed: bool) -> bool {
    let Ok(mut view) = VIEW.write() else {
        return false;
    };
    let due = view.clock_due.is_some_and(|due| due <= now);
    if changed || due {
        view.clock_due = inbox::next_clock_change(&view.rows, now);
    }
    due
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ThreadHitTarget {
    Thread(String),
    Shelf(Section),
    ShowMore,
    /// "Show N more" / "Show less" under the capped inbox.
    ExpandInbox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ThreadHit {
    pub(super) rect: Rect,
    pub(super) target: ThreadHitTarget,
}

/// The thread on screen, for highlighting and for keeping it visible in a
/// closed shelf: the thread of the focused pane, when the shown machine is
/// the thread machine.
fn shown_for_render(
    active_snapshot: Option<&ClientShellSnapshot>,
    config: &ClientShellConfig,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
) -> Option<String> {
    if !config.threads.enabled() {
        return None;
    }
    let endpoint_id = machine_endpoint_id(&config.threads.machine, endpoints)?;
    if endpoint_id != *active_endpoint_id {
        return None;
    }
    shown_thread(active_snapshot?, &config.threads.template)
}

/// Sidebar hook, expanded sidebars: draws the thread panel in the agents
/// section's area. Returns what is left for the agents section: nothing
/// when the panel was drawn, all of `area` when the feature is off.
pub(super) fn render_thread_panel(
    buffer: &mut Buffer,
    area: Rect,
    active_snapshot: Option<&ClientShellSnapshot>,
    config: &ClientShellConfig,
    state: &mut super::render::ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) -> Rect {
    if !config.threads.enabled() {
        return area;
    }
    let shown = shown_for_render(
        active_snapshot,
        config,
        state.endpoints,
        state.active_endpoint_id,
    );
    render_panel(
        buffer,
        area,
        config,
        shown.as_deref(),
        state.agent_scroll,
        hits,
    );
    Rect::new(area.x, area.y, area.width, 0)
}

/// Sidebar hook, collapsed single-machine sidebar: numbered status icons of
/// the displayed rows in place of the agents'. Returns the area left for
/// the agents (none when drawn).
pub(super) fn render_collapsed_threads(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> Rect {
    if !config.threads.enabled() {
        return area;
    }
    let shown = config
        .threads
        .machine
        .is_empty()
        .then(|| shown_thread(snapshot, &config.threads.template))
        .flatten();
    render_collapsed(buffer, area, config, shown.as_deref(), hits);
    Rect::new(area.x, area.y, area.width, 0)
}

/// Sidebar hook, collapsed multi-machine sidebar; see
/// `render_collapsed_threads`.
pub(super) fn render_collapsed_endpoint_threads(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    state: &super::render::ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) -> Rect {
    if !config.threads.enabled() {
        return area;
    }
    let active_snapshot = state
        .endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_id == *state.active_endpoint_id)
        .and_then(|endpoint| endpoint.snapshot.as_deref());
    let shown = shown_for_render(
        active_snapshot,
        config,
        state.endpoints,
        state.active_endpoint_id,
    );
    render_collapsed(buffer, area, config, shown.as_deref(), hits);
    Rect::new(area.x, area.y, area.width, 0)
}

fn render_collapsed(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    shown: Option<&str>,
    hits: &mut ShellHitMap,
) {
    let view = view();
    let now = inbox::now_millis();
    let lines = inbox::inbox_lines(&view.rows, now, &view.shelves, shown);
    let palette = &config.palette;
    for (number, index) in inbox::displayed_rows(&lines)
        .into_iter()
        .take(area.height as usize)
        .enumerate()
    {
        let row = &view.rows[index];
        let rect = Rect::new(area.x, area.y + number as u16, area.width, 1);
        let is_shown = shown == Some(row.id.as_str());
        if is_shown {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        super::render::put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &format!("{:<2}", number + 1),
            Style::default().fg(if is_shown {
                palette.text
            } else {
                palette.overlay0
            }),
        );
        let section = inbox::section(row, now);
        let (icon, color) = row_icon(row, section, now, config);
        super::render::put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            icon,
            Style::default().fg(if view.stale { palette.overlay0 } else { color }),
        );
        if config.mouse_capture {
            hits.fork_threads.push(ThreadHit {
                rect,
                target: ThreadHitTarget::Thread(row.id.clone()),
            });
        }
    }
}

/// Lines a row takes: cards have a meta line when there is something to put
/// on it; slim rows, shelves and "Show more" take one.
fn line_height(line: &InboxLine, rows: &[ThreadRow]) -> u16 {
    match line {
        InboxLine::Row { index, section } if !section.slim() => {
            1 + u16::from(!row_meta(&rows[*index]).is_empty())
        }
        _ => 1,
    }
}

/// The card's second line: ticket, project, branch, pull request.
fn row_meta(row: &ThreadRow) -> String {
    let mut parts = Vec::new();
    if let Some(ticket) = &row.ticket {
        parts.push(ticket.label.clone());
    }
    if !row.inbox.project_title.is_empty() {
        parts.push(row.inbox.project_title.clone());
    }
    if !row.branch.is_empty() {
        parts.push(row.branch.clone());
    }
    if let Some(pr) = &row.inbox.pr {
        let number = pr.number.map(|number| format!("#{number}"));
        let text = match (number, pr.state.is_empty()) {
            (Some(number), false) => format!("{number} {}", pr.state),
            (Some(number), true) => number,
            (None, false) => format!("PR {}", pr.state),
            (None, true) => String::new(),
        };
        if !text.is_empty() {
            parts.push(text);
        }
    }
    parts.join(" · ")
}

/// herdr's status icon and colour for a row: approval, input, request,
/// limited and failed borrow `blocked`; running states `working`; an unread
/// or woke row `done`; slim rows a neutral dot.
fn row_icon(
    row: &ThreadRow,
    section: Section,
    now: i64,
    config: &ClientShellConfig,
) -> (&'static str, ratatui::style::Color) {
    use crate::api::schema::AgentStatus;
    let status = if section.slim() {
        AgentStatus::Unknown
    } else {
        match row.inbox.status {
            status if status.needs_attention() => AgentStatus::Blocked,
            RowStatus::Waiting | RowStatus::Preparing | RowStatus::Working => AgentStatus::Working,
            _ if row.inbox.unread || inbox::is_woke(row, now) => AgentStatus::Done,
            _ => AgentStatus::Idle,
        }
    };
    (
        status_icon(status, config.status_indicators),
        status_color(status, &config.palette),
    )
}

fn label_style(label: &RowLabel, palette: &Palette) -> Style {
    let bold = |color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    match label {
        RowLabel::Approval | RowLabel::Limited => bold(palette.peach),
        RowLabel::Input => bold(palette.mauve),
        RowLabel::Request => bold(palette.teal),
        RowLabel::Failed => bold(palette.red),
        RowLabel::Waiting(_)
        | RowLabel::SettingUp(_)
        | RowLabel::Working(_)
        | RowLabel::Goal(_) => Style::default().fg(palette.yellow),
        RowLabel::Woke => bold(palette.peach),
        RowLabel::Done => bold(palette.green),
        RowLabel::Snoozed(_) | RowLabel::Time(_) | RowLabel::Nothing => {
            Style::default().fg(palette.overlay0)
        }
    }
}

fn render_panel(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    shown: Option<&str>,
    scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    if area.height == 0 {
        return;
    }
    // Same header as the agents section: a rule, the title, a blank line.
    super::render::put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(palette.surface_dim),
    );
    if area.height < 2 {
        return;
    }
    let mut view = view();
    super::render::put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " threads",
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let now = inbox::now_millis();
    // Right of the title: the load state, else the open-thread count.
    let state = if !view.loaded && view.error.is_none() {
        Some("loading".to_string())
    } else if view.stale {
        Some("offline".to_string())
    } else if view.loaded {
        Some(inbox::open_count(&view.rows, now).to_string())
    } else {
        None
    };
    if let Some(state) = state {
        super::render::put_right_text(
            buffer,
            Rect::new(area.x, area.y + 1, area.width.saturating_sub(1), 1),
            area.y + 1,
            &state,
            Style::default().fg(palette.overlay0),
        );
    }
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    let lines = inbox::inbox_lines(&view.rows, now, &view.shelves, shown);
    if body.is_empty() || lines.is_empty() {
        *scroll = 0;
        if view.loaded && !body.is_empty() {
            super::render::put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                " no threads",
                Style::default()
                    .fg(palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }
    let all_heights = lines
        .iter()
        .map(|line| line_height(line, &view.rows))
        .collect::<Vec<_>>();
    let cursor = view.cursor.filter(|cursor| *cursor < lines.len());
    // The Working section: its own block at the bottom of the panel, up to
    // half of it, not scrolled with the inbox above (D41 amendment
    // 2026-11-17).
    let split = inbox::working_start(&lines);
    let working_height = if split < lines.len() {
        let wanted = 2 + all_heights[split..].iter().sum::<u16>();
        wanted.min((body.height / 2).max(3)).min(body.height)
    } else {
        0
    };
    let working_area = Rect::new(
        body.x,
        body.bottom().saturating_sub(working_height),
        body.width,
        working_height,
    );
    let body = Rect::new(
        body.x,
        body.y,
        body.width,
        body.height.saturating_sub(working_height),
    );
    hits.agent_body = body;
    render_working(
        buffer,
        working_area,
        &lines,
        &all_heights,
        split,
        &view,
        shown,
        cursor,
        now,
        config,
        hits,
    );
    let heights = &all_heights[..split];
    let gaps = vec![0; split];
    let reveal_line = match view.reveal.take() {
        Some(Reveal::Cursor) => cursor,
        Some(Reveal::Thread(id)) => lines.iter().position(
            |line| matches!(line, InboxLine::Row { index, .. } if view.rows[*index].id == id),
        ),
        None => None,
    };
    if reveal_line.is_some() {
        update_view(|view| view.reveal = None);
    }
    if split == 0 || body.is_empty() {
        *scroll = 0;
        return;
    }
    if let Some(target) = reveal_line.filter(|line| *line < split) {
        *scroll = super::scroll::list_scroll_start_to_reveal(
            heights,
            &gaps,
            body.height,
            *scroll,
            target,
        );
    }
    let metrics = super::scroll::list_scroll_metrics(heights, &gaps, body.height, *scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    // Like t3code, the shelves sit at the bottom when everything fits.
    let total: u16 = heights.iter().sum();
    let first_shelf = lines[..split]
        .iter()
        .position(|line| matches!(line, InboxLine::Shelf { .. }));
    let shelf_offset = match first_shelf {
        Some(_) if metrics.max_offset_from_bottom == 0 && total < body.height => {
            body.height - total
        }
        _ => 0,
    };
    let mut y = body.y;
    for (position, line) in lines[..split].iter().enumerate().skip(*scroll) {
        if Some(position) == first_shelf {
            y = y.saturating_add(shelf_offset);
        }
        let height = heights[position].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        let selected = cursor == Some(position);
        let target = render_line(buffer, rect, line, &view, shown, selected, now, config);
        if config.mouse_capture {
            hits.fork_threads.push(ThreadHit { rect, target });
        }
        y = y.saturating_add(height);
    }
    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }
}

/// The Working section: a rule, " working N", then its rows (never
/// scrolled; rows that do not fit are cut off).
#[allow(clippy::too_many_arguments)] // one call site; the arguments are the render inputs
fn render_working(
    buffer: &mut Buffer,
    area: Rect,
    lines: &[InboxLine],
    heights: &[u16],
    split: usize,
    view: &ThreadsView,
    shown: Option<&str>,
    cursor: Option<usize>,
    now: i64,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    if area.height == 0 {
        return;
    }
    super::render::put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(palette.surface_dim),
    );
    if area.height < 2 {
        return;
    }
    super::render::put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        &format!(" working {}", lines.len() - split),
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let mut y = area.y + 2;
    for (position, line) in lines.iter().enumerate().skip(split) {
        let height = heights[position];
        if y.saturating_add(height) > area.bottom() {
            break;
        }
        let rect = Rect::new(area.x, y, area.width, height);
        let target = render_line(
            buffer,
            rect,
            line,
            view,
            shown,
            cursor == Some(position),
            now,
            config,
        );
        if config.mouse_capture {
            hits.fork_threads.push(ThreadHit { rect, target });
        }
        y = y.saturating_add(height);
    }
}

#[allow(clippy::too_many_arguments)] // the arguments are the render inputs
fn render_line(
    buffer: &mut Buffer,
    rect: Rect,
    line: &InboxLine,
    view: &ThreadsView,
    shown: Option<&str>,
    selected: bool,
    now: i64,
    config: &ClientShellConfig,
) -> ThreadHitTarget {
    let palette = &config.palette;
    if selected {
        buffer.set_style(rect, Style::default().bg(palette.selection_bg));
    }
    let header = Style::default()
        .fg(palette.overlay0)
        .add_modifier(Modifier::BOLD);
    match line {
        InboxLine::Shelf {
            section,
            count,
            open,
        } => {
            let marker = if *open { "▾" } else { "▸" };
            super::render::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                &format!(" {marker} {} {count}", section.shelf_label()),
                header,
            );
            ThreadHitTarget::Shelf(*section)
        }
        InboxLine::ShowMore(count) => {
            super::render::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                &format!("   Show {count} more"),
                Style::default().fg(palette.overlay1),
            );
            ThreadHitTarget::ShowMore
        }
        InboxLine::Expand { hidden, open } => {
            let text = if *open {
                "   Show less".to_owned()
            } else {
                format!("   Show {hidden} more")
            };
            super::render::put_text(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                &text,
                Style::default().fg(palette.overlay1),
            );
            ThreadHitTarget::ExpandInbox
        }
        InboxLine::Row { index, section } => {
            let row = &view.rows[*index];
            let is_shown = shown == Some(row.id.as_str());
            render_row(
                buffer, rect, row, *section, is_shown, selected, view.stale, now, config,
            );
            ThreadHitTarget::Thread(row.id.clone())
        }
    }
}

#[allow(clippy::too_many_arguments)] // one call site; the arguments are the render inputs
fn render_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &ThreadRow,
    section: Section,
    shown: bool,
    selected: bool,
    stale: bool,
    now: i64,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    if shown && !selected {
        buffer.set_style(
            rect,
            Style::default().bg(super::sidebar::workspace_active_background(palette, false)),
        );
    }
    let (icon, icon_color) = row_icon(row, section, now, config);
    super::render::put_text(
        buffer,
        rect.x + 1,
        rect.y,
        2,
        icon,
        Style::default().fg(icon_color),
    );
    let label = inbox::row_label(row, section, now);
    let label_text = label.text();
    let attention = !section.slim() && row.inbox.status.needs_attention();
    let title_style = if attention || shown || selected {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else if inbox::receded(row, now) {
        Style::default().fg(palette.overlay1)
    } else {
        Style::default().fg(palette.subtext0)
    };
    let title_x = rect.x + 3;
    let available = rect.right().saturating_sub(title_x);
    // The status wins over the title, but the title keeps a few cells.
    let label_width = super::render::display_width(&label_text)
        .min(available.saturating_sub(8).max(available / 2));
    let title_width = if label_width > 0 {
        available.saturating_sub(label_width + 2)
    } else {
        available.saturating_sub(1)
    };
    super::render::put_text(
        buffer,
        title_x,
        rect.y,
        title_width,
        row.display_title(),
        title_style,
    );
    if label_width > 0 {
        super::render::put_right_text(
            buffer,
            Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), 1),
            rect.y,
            &truncate_to_width(&label_text, label_width),
            label_style(&label, palette),
        );
    }
    if rect.height > 1 {
        super::render::put_text(
            buffer,
            rect.x + 3,
            rect.y + 1,
            rect.width.saturating_sub(4),
            &row_meta(row),
            Style::default().fg(palette.overlay0),
        );
    }
    if stale {
        buffer.set_style(
            rect,
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::DIM),
        );
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

/// `command` from `[ui.threads]`, split around its `{thread}` argument.
/// Builds the command for a thread and recognises a thread in a pane's
/// reported resume command (`t3t -r <id>`, also `t3t --server x -r <id>`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommandTemplate {
    text: String,
    program: String,
    /// The option word before `{thread}`, e.g. `-r`. Without one the thread
    /// is positional.
    flag: Option<String>,
    position: usize,
    prefix: String,
    suffix: String,
}

fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

impl CommandTemplate {
    pub(crate) fn parse(command: &str) -> Option<Self> {
        let words = command.split_whitespace().collect::<Vec<_>>();
        let position = words
            .iter()
            .skip(1)
            .position(|word| word.contains(crate::config::THREAD_PLACEHOLDER))?
            + 1;
        let (prefix, suffix) = words[position].split_once(crate::config::THREAD_PLACEHOLDER)?;
        let flag = (position >= 2 && words[position - 1].starts_with('-'))
            .then(|| words[position - 1].to_owned());
        Some(Self {
            text: words.join(" "),
            program: program_name(words[0]).to_owned(),
            flag,
            position,
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
        })
    }

    pub(crate) fn command_for(&self, thread_id: &str) -> Option<String> {
        (!self.text.is_empty() && super::fork_threads_feed::valid_thread_id(thread_id)).then(|| {
            self.text
                .replace(crate::config::THREAD_PLACEHOLDER, thread_id)
        })
    }

    /// The resume command runs this template's program.
    pub(crate) fn runs_program(&self, line: &str) -> bool {
        !self.program.is_empty()
            && line
                .split_whitespace()
                .next()
                .is_some_and(|word| program_name(word) == self.program)
    }

    pub(crate) fn thread_in(&self, line: &str) -> Option<String> {
        if !self.runs_program(line) {
            return None;
        }
        let words = line.split_whitespace().collect::<Vec<_>>();
        let word = match &self.flag {
            Some(flag) => {
                let index = words.iter().skip(1).position(|word| word == flag)? + 1;
                words.get(index + 1)?
            }
            None => words.get(self.position)?,
        };
        let id = word
            .strip_prefix(&self.prefix)?
            .strip_suffix(&self.suffix)?;
        super::fork_threads_feed::valid_thread_id(id).then(|| id.to_owned())
    }
}

fn token<'a>(tokens: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tokens
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Threads a workspace ever showed, from the server's resume history.
pub(super) fn workspace_threads(
    workspace: &ClientShellWorkspace,
    template: &CommandTemplate,
) -> Vec<String> {
    token(&workspace.tokens, crate::app::FORK_SESSIONS_TOKEN)
        .map(|history| {
            history
                .lines()
                .filter_map(|line| template.thread_in(line))
                .collect()
        })
        .unwrap_or_default()
}

fn agent_resume(agent: &crate::protocol::ClientShellAgent) -> Option<&str> {
    token(&agent.tokens, crate::app::FORK_RESUME_TOKEN)
}

/// The thread on the focused pane of `snapshot`, if it shows one.
fn shown_thread(snapshot: &ClientShellSnapshot, template: &CommandTemplate) -> Option<String> {
    let pane_id = snapshot.focused_pane_id.as_deref()?;
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    template.thread_in(agent_resume(agent)?)
}

/// Sidebar hook: workspaces that own a listed thread show as thread rows
/// instead. Matches on any machine, by content.
pub(super) fn is_hidden_workspace(workspace: &ClientShellWorkspace) -> bool {
    if workspace.tokens.is_empty() {
        return false;
    }
    VIEW.read().is_ok_and(|view| {
        view.enabled
            && !view.listed.is_empty()
            && workspace_threads(workspace, &view.template)
                .iter()
                .any(|thread| view.listed.contains(thread))
    })
}

/// The endpoint that owns thread workspaces: local for an empty label, else
/// the saved machine with that label.
pub(super) fn machine_endpoint_id(
    machine: &str,
    endpoints: &[ClientShellEndpoint],
) -> Option<ClientEndpointId> {
    if machine.is_empty() {
        return Some(ClientEndpointId::Local);
    }
    endpoints
        .iter()
        .find(|endpoint| !endpoint.endpoint_id.is_local() && endpoint.label == machine)
        .map(|endpoint| endpoint.endpoint_id.clone())
}

/// What opening a thread does, given the owning server's snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OpenPlan {
    /// A pane already shows the thread.
    FocusPane(String),
    /// Restart this thread pane in place on the thread.
    Replace(String),
    /// Add a thread pane to this workspace.
    AddTo(String),
    /// New workspace for the thread.
    Create { cwd: Option<String>, label: String },
}

pub(super) fn plan_open(
    snapshot: &ClientShellSnapshot,
    template: &CommandTemplate,
    rows: &[ThreadRow],
    thread_id: &str,
) -> Option<OpenPlan> {
    let row = rows.iter().find(|row| row.id == thread_id)?;
    let owner = |ids: &[&str]| {
        snapshot
            .workspaces
            .iter()
            .find(|workspace| {
                workspace_threads(workspace, template)
                    .iter()
                    .any(|thread| ids.contains(&thread.as_str()))
            })
            .map(|workspace| workspace.workspace_id.clone())
    };
    let workspace_id = owner(&[thread_id]).or_else(|| {
        let ticket = row.ticket.as_ref()?;
        let siblings = rows
            .iter()
            .filter(|other| {
                other.id != row.id
                    && other
                        .ticket
                        .as_ref()
                        .is_some_and(|other| other.id == ticket.id)
            })
            .map(|other| other.id.as_str())
            .collect::<Vec<_>>();
        owner(&siblings)
    });
    let Some(workspace_id) = workspace_id else {
        return Some(OpenPlan::Create {
            cwd: (!row.cwd.is_empty()).then(|| row.cwd.clone()),
            label: row
                .ticket
                .as_ref()
                .map(|ticket| ticket.label.clone())
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| row.display_title().to_owned()),
        });
    };
    let agents = snapshot
        .agents
        .iter()
        .filter(|agent| agent.workspace_id == workspace_id)
        .filter_map(|agent| Some((agent, agent_resume(agent)?)))
        .collect::<Vec<_>>();
    if let Some((agent, _)) = agents
        .iter()
        .find(|(_, line)| template.thread_in(line).as_deref() == Some(thread_id))
    {
        return Some(OpenPlan::FocusPane(agent.pane_id.clone()));
    }
    let mut thread_panes = agents
        .iter()
        .filter(|(_, line)| template.runs_program(line))
        .map(|(agent, _)| *agent)
        .collect::<Vec<_>>();
    thread_panes.sort_by_key(|agent| !agent.focused);
    Some(match thread_panes.first() {
        Some(agent) => OpenPlan::Replace(agent.pane_id.clone()),
        None => OpenPlan::AddTo(workspace_id),
    })
}

/// An open waiting for its machine to become the shown one.
struct PendingOpen {
    endpoint_id: ClientEndpointId,
    thread_id: String,
    deadline: std::time::Instant,
}

static PENDING: std::sync::Mutex<Option<PendingOpen>> = std::sync::Mutex::new(None);
const PENDING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

impl ClientShellState {
    fn fork_threads_snapshot(
        &self,
        endpoint_id: &ClientEndpointId,
    ) -> Option<&ClientShellSnapshot> {
        if *endpoint_id == self.active_endpoint_id {
            return self.snapshot.as_deref();
        }
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == *endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
    }

    fn fork_threads_ready(&self, endpoint_id: &ClientEndpointId) -> bool {
        *endpoint_id == self.active_endpoint_id
            && self.endpoint_is_online(endpoint_id)
            && self.snapshot.is_some()
            && self.pane_surface.is_some()
    }

    /// Opens a thread: shows its workspace on its machine, then focuses the
    /// pane that shows it, restarts the workspace's thread pane on it, or adds
    /// one; a thread without a workspace gets one first.
    pub(super) fn fork_threads_open(&mut self, thread_id: &str, outcome: &mut ClientShellInput) {
        let threads = &self.config.threads;
        let Some(endpoint_id) = machine_endpoint_id(&threads.machine, &self.endpoints) else {
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unavailable,
                "threads_machine_missing",
                "Thread machine unknown",
                format!("No saved machine labelled \"{}\"", threads.machine),
            );
            outcome.repaint = true;
            return;
        };
        let view = view();
        let template = threads.template.clone();
        let plan = self
            .fork_threads_snapshot(&endpoint_id)
            .and_then(|snapshot| plan_open(snapshot, &template, &view.rows, thread_id));
        outcome.repaint = true;
        if let Some(OpenPlan::FocusPane(pane_id)) = &plan {
            self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Pane(pane_id.clone()),
                outcome,
            );
            return;
        }
        if !self.fork_threads_ready(&endpoint_id) {
            // pane.run_command goes to the shown machine; show it first and
            // finish on the client timer once its surface is up.
            let target = match &plan {
                Some(OpenPlan::Replace(pane_id)) => {
                    Some(ClientEndpointFocusTarget::Pane(pane_id.clone()))
                }
                Some(OpenPlan::AddTo(workspace_id)) => {
                    Some(ClientEndpointFocusTarget::Workspace(workspace_id.clone()))
                }
                _ => None,
            };
            let shown = match target {
                Some(target) => self.focus_or_activate(endpoint_id.clone(), target, outcome),
                None => self.activate_endpoint(endpoint_id.clone(), outcome),
            };
            if shown {
                if let Ok(mut pending) = PENDING.lock() {
                    *pending = Some(PendingOpen {
                        endpoint_id,
                        thread_id: thread_id.to_owned(),
                        deadline: std::time::Instant::now() + PENDING_TIMEOUT,
                    });
                }
            }
            return;
        }
        let Some(plan) = plan else {
            return;
        };
        let Some(command) = template.command_for(thread_id) else {
            return;
        };
        let mut params = crate::api::schema::PaneRunCommandParams {
            command,
            replace_pane_id: None,
            workspace_id: None,
            cwd: None,
            label: None,
            focus: true,
        };
        match plan {
            OpenPlan::FocusPane(_) => return,
            OpenPlan::Replace(pane_id) => params.replace_pane_id = Some(pane_id),
            OpenPlan::AddTo(workspace_id) => params.workspace_id = Some(workspace_id),
            OpenPlan::Create { cwd, label } => {
                params.cwd = cwd;
                params.label = Some(label);
            }
        }
        self.push_endpoint_method(crate::api::schema::Method::PaneRunCommand(params), outcome);
    }

    /// Client timer hook: picks up a new thread list, re-evaluates the clock
    /// (snooze ends, relative times) only when its next change is due, and
    /// finishes an open that waited for its machine.
    pub(crate) fn fork_threads_tick(&mut self, outcome: &mut ClientShellInput) {
        if !self.config.threads.enabled() {
            return;
        }
        let changed = refresh_view();
        let due = clock_tick(inbox::now_millis(), changed);
        outcome.repaint |= changed || due;
        let ready = {
            let Ok(mut pending) = PENDING.lock() else {
                return;
            };
            match pending.as_ref() {
                Some(open) if open.deadline <= std::time::Instant::now() => {
                    *pending = None;
                    None
                }
                Some(open) if self.fork_threads_ready(&open.endpoint_id) => pending.take(),
                _ => None,
            }
        };
        if let Some(open) = ready {
            self.fork_threads_open(&open.thread_id, outcome);
        }
    }

    /// The thread on screen (focused pane of the thread machine), if any.
    fn fork_threads_shown(&self) -> Option<String> {
        let threads = &self.config.threads;
        if machine_endpoint_id(&threads.machine, &self.endpoints)? != self.active_endpoint_id {
            return None;
        }
        shown_thread(self.snapshot.as_deref()?, &threads.template)
    }

    /// The panel's lines as the sidebar draws them now.
    fn fork_threads_lines(&self, view: &ThreadsView) -> Vec<InboxLine> {
        let shown = self.fork_threads_shown();
        inbox::inbox_lines(
            &view.rows,
            inbox::now_millis(),
            &view.shelves,
            shown.as_deref(),
        )
    }

    /// Runs what a thread panel line does: open a thread, toggle a shelf,
    /// or show more settled rows.
    fn fork_threads_activate(&mut self, target: ThreadHitTarget, outcome: &mut ClientShellInput) {
        match target {
            ThreadHitTarget::Thread(thread_id) => self.fork_threads_open(&thread_id, outcome),
            ThreadHitTarget::Shelf(section) => {
                update_view(|view| view.shelves.toggle(section));
                outcome.repaint = true;
            }
            ThreadHitTarget::ShowMore => {
                update_view(|view| view.shelves.show_more());
                outcome.repaint = true;
            }
            ThreadHitTarget::ExpandInbox => {
                update_view(|view| view.shelves.toggle_inbox());
                outcome.repaint = true;
            }
        }
    }

    /// Handles a press on a thread panel line. Returns true when it was one.
    pub(super) fn fork_threads_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(target) = self
            .hits
            .fork_threads
            .iter()
            .find(|hit| super::contains(hit.rect, point))
            .map(|hit| hit.target.clone())
        else {
            return false;
        };
        self.fork_threads_activate(target, outcome);
        true
    }

    /// Key binding hook: with the thread panel in place of the agents
    /// section, `next_agent` / `previous_agent` step through the displayed
    /// thread rows (wrapping, from the thread on screen) and `focus_agent N`
    /// takes the Nth; each opens its thread like a click. Returns true when
    /// it handled `action`.
    pub(super) fn fork_threads_agent_action(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction;
        if !self.config.threads.enabled()
            || !matches!(
                action,
                KeybindAction::PreviousAgent
                    | KeybindAction::NextAgent
                    | KeybindAction::FocusAgent(_)
            )
        {
            return false;
        }
        let view = view();
        let rows = inbox::displayed_rows(&self.fork_threads_lines(&view));
        if rows.is_empty() {
            return true;
        }
        let next = match action {
            KeybindAction::FocusAgent(index) if index < rows.len() => index,
            KeybindAction::FocusAgent(_) => return true,
            _ => {
                let shown = self.fork_threads_shown();
                let current = rows
                    .iter()
                    .position(|index| Some(view.rows[*index].id.as_str()) == shown.as_deref());
                match (current, action) {
                    (Some(index), KeybindAction::PreviousAgent) => {
                        (index + rows.len() - 1) % rows.len()
                    }
                    (Some(index), _) => (index + 1) % rows.len(),
                    (None, KeybindAction::PreviousAgent) => rows.len() - 1,
                    (None, _) => 0,
                }
            }
        };
        let thread_id = view.rows[rows[next]].id.clone();
        update_view(|view| view.reveal = Some(Reveal::Thread(thread_id.clone())));
        self.fork_threads_open(&thread_id, outcome);
        outcome.repaint = true;
        true
    }

    /// `focus_agent N` exists while the panel shows at least N thread rows.
    /// `None` when the agents section is not replaced.
    pub(super) fn fork_threads_target_exists(
        &self,
        binding: &crate::input::KeybindMatch,
    ) -> Option<bool> {
        use crate::input::{KeybindAction, KeybindMatch};
        let KeybindMatch::Action(KeybindAction::FocusAgent(index)) = binding else {
            return None;
        };
        if !self.config.threads.enabled() {
            return None;
        }
        let view = view();
        Some(*index < inbox::displayed_rows(&self.fork_threads_lines(&view)).len())
    }

    /// Workspaces the navigate cursor can reach, as `(endpoint, workspace)`,
    /// in the order `move_navigate_workspace` walks them.
    fn fork_threads_navigation_targets(&self) -> Vec<(ClientEndpointId, String)> {
        let empty = HashSet::new();
        self.endpoints
            .iter()
            .filter(|endpoint| endpoint.status == ClientEndpointStatus::Online)
            .filter_map(|endpoint| {
                let snapshot = endpoint.snapshot.as_deref()?;
                let groups = self
                    .collapsed_groups_for_endpoint(&endpoint.endpoint_id)
                    .unwrap_or(&empty);
                Some(
                    render::workspace_entries(snapshot, groups)
                        .into_iter()
                        .map(|entry| {
                            (
                                endpoint.endpoint_id.clone(),
                                snapshot.workspaces[entry.index].workspace_id.clone(),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    }

    /// Navigate-mode hook. The panel's lines join the workspace cycle after
    /// the last workspace, as the panel sits below the workspace list: the
    /// workspace up/down keys move into and through them; Enter opens a
    /// thread, opens or closes a shelf, or shows more settled rows. Returns
    /// true when it handled `key`.
    pub(super) fn fork_threads_navigate_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let view = view();
        let lines = if self.config.threads.enabled() {
            self.fork_threads_lines(&view)
        } else {
            Vec::new()
        };
        let count = lines.len();
        let clear = || update_view(|view| view.cursor = None);
        if count == 0 || self.sidebar_collapsed || self.mobile_layout_active() {
            if view.cursor.is_some() {
                clear();
            }
            return false;
        }
        let navigate = &self.config.keybinds.keybinds.navigate;
        let up = navigate.workspace_up.matches_direct_key(key);
        let down = navigate.workspace_down.matches_direct_key(key);
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        let enter = code == KeyCode::Enter && modifiers.is_empty();
        let set = |cursor: usize| {
            update_view(|view| {
                view.cursor = Some(cursor);
                view.reveal = Some(Reveal::Cursor);
            })
        };
        match view.cursor.map(|cursor| cursor.min(count - 1)) {
            Some(cursor) if up && cursor > 0 => set(cursor - 1),
            Some(cursor) if down && cursor + 1 < count => set(cursor + 1),
            Some(_) if up || down => {
                clear();
                self.navigate_workspace_id = None;
                self.move_navigate_workspace(if up { -1 } else { 1 });
            }
            Some(cursor) if enter => {
                let target = match lines[cursor] {
                    InboxLine::Row { index, .. } => {
                        ThreadHitTarget::Thread(view.rows[index].id.clone())
                    }
                    InboxLine::Shelf { section, .. } => ThreadHitTarget::Shelf(section),
                    InboxLine::ShowMore(_) => ThreadHitTarget::ShowMore,
                    InboxLine::Expand { .. } => ThreadHitTarget::ExpandInbox,
                };
                if matches!(target, ThreadHitTarget::Thread(_)) {
                    clear();
                    self.mode = self.copy_or_terminal_mode();
                    self.navigate_workspace_id = None;
                }
                self.fork_threads_activate(target, outcome);
            }
            Some(_) => {
                clear();
                return false;
            }
            None => {
                let current = self
                    .navigate_workspace_id
                    .as_ref()
                    .map(|target| (target.endpoint_id.clone(), target.workspace_id.clone()));
                let targets = self.fork_threads_navigation_targets();
                let cursor = match &current {
                    // Navigation started on a thread workspace, which the
                    // list hides: start on that thread's row.
                    Some(current) if (up || down) && !targets.contains(current) => {
                        let shown = self.fork_threads_shown();
                        lines
                            .iter()
                            .position(|line| {
                                matches!(line, InboxLine::Row { index, .. }
                                    if Some(view.rows[*index].id.as_str()) == shown.as_deref())
                            })
                            .unwrap_or(if up { count - 1 } else { 0 })
                    }
                    _ if down && (targets.is_empty() || current.as_ref() == targets.last()) => 0,
                    _ if up && (targets.is_empty() || current.as_ref() == targets.first()) => {
                        count - 1
                    }
                    _ => return false,
                };
                set(cursor);
                self.navigate_workspace_id = None;
            }
        }
        outcome.repaint = true;
        true
    }
}
