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
    pub(super) template: CommandTemplate,
    /// Ids in `rows`, for hiding workspaces that own a listed thread.
    pub(super) listed: std::collections::HashSet<String>,
}

static VIEW: LazyLock<RwLock<ThreadsView>> = LazyLock::new(Default::default);

pub(super) fn view() -> ThreadsView {
    VIEW.read().map(|view| view.clone()).unwrap_or_default()
}

/// Test seam: what the reader would have delivered.
#[cfg(test)]
pub(super) fn set_view_for_test(rows: Vec<ThreadRow>, stale: bool) {
    if let Ok(mut view) = VIEW.write() {
        view.listed = rows.iter().map(|row| row.id.clone()).collect();
        view.rows = Arc::new(rows);
        view.loaded = true;
        view.stale = stale;
        view.error = stale.then(|| "down".into());
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
    view.listed = feed.rows.iter().map(|row| row.id.clone()).collect();
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
    let shown = config
        .threads
        .enabled()
        .then(|| {
            let endpoint_id = machine_endpoint_id(&config.threads.machine, state.endpoints)?;
            if endpoint_id != *state.active_endpoint_id {
                return None;
            }
            shown_thread(active_snapshot?, &config.threads.template)
        })
        .flatten();
    render_thread_block(buffer, area, config, shown.as_deref(), hits)
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

    /// Client timer hook: picks up a new thread list and finishes an open
    /// that waited for its machine.
    pub(crate) fn fork_threads_tick(&mut self, outcome: &mut ClientShellInput) {
        if !self.config.threads.enabled() {
            return;
        }
        outcome.repaint |= refresh_view();
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

    /// Handles a press on a thread row. Returns true when it was one.
    pub(super) fn fork_threads_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(thread_id) = self
            .hits
            .fork_threads
            .iter()
            .find(|hit| super::contains(hit.rect, point))
            .map(|hit| hit.thread_id.clone())
        else {
            return false;
        };
        self.fork_threads_open(&thread_id, outcome);
        true
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

    /// Position in `display_order` of the thread on screen, if any.
    fn fork_threads_shown_position(&self, rows: &[ThreadRow]) -> Option<usize> {
        let threads = &self.config.threads;
        if machine_endpoint_id(&threads.machine, &self.endpoints)? != self.active_endpoint_id {
            return None;
        }
        let shown = shown_thread(self.snapshot.as_deref()?, &threads.template)?;
        display_order(rows)
            .iter()
            .position(|index| rows[*index].id == shown)
    }

    /// Navigate-mode hook. The thread rows join the workspace cycle above
    /// the first workspace: the workspace up/down keys move into and through
    /// them, Enter opens the selected thread. Returns true when it handled
    /// `key`.
    pub(super) fn fork_threads_navigate_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let view = view();
        let count = display_order(&view.rows).len();
        let clear = || update_view(|view| view.cursor = None);
        if !self.config.threads.enabled()
            || count == 0
            || self.sidebar_collapsed
            || self.mobile_layout_active()
        {
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
        let set = |cursor: Option<usize>| update_view(|view| view.cursor = cursor);
        match view.cursor {
            Some(cursor) if up && cursor > 0 => set(Some(cursor - 1)),
            Some(cursor) if down && cursor + 1 < count => set(Some(cursor + 1)),
            Some(_) if up || down => {
                set(None);
                self.navigate_workspace_id = None;
                self.move_navigate_workspace(if up { -1 } else { 1 });
            }
            Some(cursor) if enter => {
                set(None);
                self.mode = self.copy_or_terminal_mode();
                self.navigate_workspace_id = None;
                if let Some(index) = display_order(&view.rows).get(cursor) {
                    let thread_id = view.rows[*index].id.clone();
                    self.fork_threads_open(&thread_id, outcome);
                }
            }
            Some(_) => {
                set(None);
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
                    Some(current) if (up || down) && !targets.contains(current) => self
                        .fork_threads_shown_position(&view.rows)
                        .unwrap_or(if up { count - 1 } else { 0 }),
                    _ if up && (targets.is_empty() || current.as_ref() == targets.first()) => {
                        count - 1
                    }
                    _ if down && (targets.is_empty() || current.as_ref() == targets.last()) => 0,
                    _ => return false,
                };
                set(Some(cursor));
                self.navigate_workspace_id = None;
            }
        }
        outcome.repaint = true;
        true
    }
}
