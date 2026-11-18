//! fork: ticket picker, a new thread for a ticket (FORK.md 0005).
//!
//! `[ui.threads] new_key` opens a modal picker over the current view that
//! lists the open tickets of the thread service (`GET <url>/api/tickets`,
//! read by `fork_threads_feed` on the same stamp-driven refetch as the
//! threads). Typing filters, up/down (also ctrl+n/ctrl+p) move, Enter starts
//! a new thread for the ticket, Esc closes.
//!
//! Below the tickets, a `projects` group lists the t3 projects that are not
//! ticket dirs (`GET <url>/api/projects`); Enter on one starts a ticket-less
//! thread in a new workspace in the project root, running `project_command`
//! with `{dir}` replaced by the single-quoted root.
//!
//! A new thread opens like a thread row does (`fork_threads_run`): in the
//! ticket's workspace on the thread machine, replacing its thread pane in
//! place or adding one, else in a new workspace labelled with the ticket.
//! The pane runs `new_command` with `{ticket}` replaced by the label, which
//! is checked against `t<digits>` first; nothing else from the service goes
//! into the command line.
//!
//! Like the thread view, the picker lives in a process-wide cell so upstream
//! state and overlay types stay untouched; it is not a `ClientShellOverlay`.

use super::fork_threads::{self, CommandTemplate, OpenPlan, OpenTarget};
use super::fork_threads_feed::{sanitize_text, ThreadRow};
use super::text_editor::TextEditor;
use super::*;
use std::sync::{Arc, Mutex};

const MAX_TICKETS: usize = 500;
const MAX_PROJECTS: usize = 200;
const MAX_LABEL_DIGITS: usize = 9;

/// One open ticket of the thread service, sanitized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketRow {
    /// `t<digits>`, validated.
    pub(crate) label: String,
    pub(crate) title: String,
    pub(crate) status: String,
    /// External reference, e.g. `redmine:3745`.
    pub(crate) ext_ref: Option<String>,
    /// Ticket dir (absolute), `None` for parked tickets.
    pub(crate) dir: Option<String>,
}

impl TicketRow {
    fn number(&self) -> u64 {
        self.label[1..].parse().unwrap_or(0)
    }

    /// Parked: not started yet (draft or todo, no worktree). The picker
    /// lists parked tickets in their own group below the started ones.
    pub(crate) fn parked(&self) -> bool {
        matches!(self.status.as_str(), "todo" | "draft")
    }

    /// Picker order: started (in progress / in review, then any other
    /// status), then parked (todo, then draft).
    fn rank(&self) -> u8 {
        match self.status.as_str() {
            "in-progress" | "in-review" => 0,
            "todo" => 2,
            "draft" => 3,
            _ => 1,
        }
    }

    fn matches(&self, terms: &[String]) -> bool {
        let fields = [
            self.label.to_lowercase(),
            self.title.to_lowercase(),
            self.ext_ref.as_deref().unwrap_or_default().to_lowercase(),
        ];
        terms
            .iter()
            .all(|term| fields.iter().any(|field| field.contains(term.as_str())))
    }
}

/// Ticket labels go into a shell command line: only `t` and digits.
pub(crate) fn valid_ticket_label(label: &str) -> bool {
    label.strip_prefix('t').is_some_and(|digits| {
        !digits.is_empty()
            && digits.len() <= MAX_LABEL_DIGITS
            && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// `new_command` for a ticket, or `None` when the label is not `t<digits>`.
pub(crate) fn ticket_command(template: &str, label: &str) -> Option<String> {
    (!template.is_empty() && valid_ticket_label(label))
        .then(|| template.replace(crate::config::TICKET_PLACEHOLDER, label))
}

/// One t3 project of the thread service (not a ticket dir), sanitized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectRow {
    pub(crate) id: String,
    pub(crate) title: String,
    /// Workspace root: absolute, no control characters, no `'`.
    pub(crate) root: String,
}

impl ProjectRow {
    fn matches(&self, terms: &[String]) -> bool {
        let fields = [self.title.to_lowercase(), self.root.to_lowercase()];
        terms
            .iter()
            .all(|term| fields.iter().any(|field| field.contains(term.as_str())))
    }
}

/// Project roots go into a shell command line, single-quoted.
pub(crate) fn valid_project_root(root: &str) -> bool {
    root.starts_with('/')
        && root.len() <= 4096
        && !root.contains('\'')
        && !root.chars().any(char::is_control)
}

/// `project_command` for a project root, or `None` when the root is unsafe.
pub(crate) fn project_command(template: &str, root: &str) -> Option<String> {
    (!template.is_empty() && valid_project_root(root))
        .then(|| template.replace(crate::config::DIR_PLACEHOLDER, &format!("'{root}'")))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct WireProject {
    id: Option<String>,
    title: Option<String>,
    root: Option<String>,
}

/// Parses `GET /api/projects`: a JSON array, kept in the service's order
/// (most recently active first). Items without an id, with an unsafe root
/// or a repeated id are skipped; an empty title becomes the root's last
/// component.
pub(crate) fn parse_projects(body: &[u8]) -> Result<Vec<ProjectRow>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(body)
        .map_err(|error| format!("project list is not a JSON array: {error}"))?;
    let mut projects = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if projects.len() >= MAX_PROJECTS {
            break;
        }
        let Ok(project) = serde_json::from_value::<WireProject>(item) else {
            continue;
        };
        let (Some(id), Some(root)) = (project.id, project.root) else {
            continue;
        };
        let id = sanitize_text(&id);
        if id.is_empty() || !valid_project_root(&root) || !seen.insert(id.clone()) {
            continue;
        }
        let title = sanitize_text(project.title.as_deref().unwrap_or_default());
        let title = if title.trim().is_empty() {
            sanitize_text(
                root.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&root),
            )
        } else {
            title
        };
        projects.push(ProjectRow { id, title, root });
    }
    Ok(projects)
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct WireTicket {
    label: Option<String>,
    title: Option<String>,
    status: Option<String>,
    #[serde(rename = "extRef")]
    ext_ref: Option<String>,
    dir: Option<String>,
}

/// Parses `GET /api/tickets`: a JSON array. Done and dropped tickets, items
/// without a `t<digits>` label and repeated labels are skipped; the rest is
/// in picker order.
pub(crate) fn parse_tickets(body: &[u8]) -> Result<Vec<TicketRow>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(body)
        .map_err(|error| format!("ticket list is not a JSON array: {error}"))?;
    let mut tickets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if tickets.len() >= MAX_TICKETS {
            break;
        }
        let Ok(ticket) = serde_json::from_value::<WireTicket>(item) else {
            continue;
        };
        let Some(label) = ticket.label.filter(|label| valid_ticket_label(label)) else {
            continue;
        };
        let status = sanitize_text(ticket.status.as_deref().unwrap_or_default());
        if matches!(status.as_str(), "done" | "dropped") || !seen.insert(label.clone()) {
            continue;
        }
        tickets.push(TicketRow {
            label,
            title: sanitize_text(ticket.title.as_deref().unwrap_or_default()),
            status,
            ext_ref: ticket
                .ext_ref
                .map(|value| sanitize_text(&value))
                .filter(|value| !value.is_empty()),
            dir: ticket
                .dir
                .filter(|dir| dir.starts_with('/') && !dir.chars().any(char::is_control)),
        });
    }
    tickets.sort_by(|left, right| {
        left.rank()
            .cmp(&right.rank())
            .then(right.number().cmp(&left.number()))
    });
    Ok(tickets)
}

/// Indices of the tickets matching `query`: every whitespace-separated term
/// is a case-insensitive substring of the label, title or external ref.
/// Keeps the list order, so started tickets come before parked ones.
pub(crate) fn filter_tickets(tickets: &[TicketRow], query: &str) -> Vec<usize> {
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    tickets
        .iter()
        .enumerate()
        .filter(|(_, ticket)| ticket.matches(&terms))
        .map(|(index, _)| index)
        .collect()
}

/// Indices of the projects matching `query` (title or root), in list order.
pub(crate) fn filter_projects(projects: &[ProjectRow], query: &str) -> Vec<usize> {
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    projects
        .iter()
        .enumerate()
        .filter(|(_, project)| project.matches(&terms))
        .map(|(index, _)| index)
        .collect()
}

/// A selectable picker entry, by index into the list's tickets or projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerItem {
    Ticket(usize),
    Project(usize),
}

/// The entries matching `query`: the tickets, then the projects.
pub(crate) fn picker_items(list: &TicketList, query: &str) -> Vec<PickerItem> {
    filter_tickets(&list.tickets, query)
        .into_iter()
        .map(PickerItem::Ticket)
        .chain(
            filter_projects(&list.projects, query)
                .into_iter()
                .map(PickerItem::Project),
        )
        .collect()
}

/// A new thread in a project always gets its own workspace in the project
/// root, labelled with the project.
pub(super) fn plan_project_open(project: &ProjectRow) -> OpenPlan {
    OpenPlan::Create {
        cwd: Some(project.root.clone()),
        label: project.title.clone(),
    }
}

/// What starting a new thread for `ticket` does on the thread machine. The
/// ticket's workspace is the one whose history names a thread of the ticket
/// (as in `plan_open`), else one labelled with the ticket.
pub(super) fn plan_ticket_open(
    snapshot: &ClientShellSnapshot,
    template: &CommandTemplate,
    rows: &[ThreadRow],
    ticket: &TicketRow,
) -> OpenPlan {
    let threads = rows
        .iter()
        .filter(|row| {
            row.ticket
                .as_ref()
                .is_some_and(|other| other.label == ticket.label)
        })
        .map(|row| row.id.as_str())
        .collect::<Vec<_>>();
    let owner = snapshot
        .workspaces
        .iter()
        .find(|workspace| {
            !threads.is_empty()
                && fork_threads::workspace_threads(workspace, template)
                    .iter()
                    .any(|thread| threads.contains(&thread.as_str()))
        })
        .or_else(|| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.label == ticket.label)
        });
    match owner {
        Some(workspace) => {
            fork_threads::replace_or_add(snapshot, template, workspace.workspace_id.clone())
        }
        None => OpenPlan::Create {
            cwd: ticket.dir.clone(),
            label: ticket.label.clone(),
        },
    }
}

/// The open picker: the filter text and the selected line.
#[derive(Debug, Default)]
pub(super) struct TicketPicker {
    pub(super) query: TextEditor,
    /// Position in the filtered list.
    pub(super) selected: usize,
}

static PICKER: Mutex<Option<TicketPicker>> = Mutex::new(None);

pub(super) fn close_picker() {
    if let Ok(mut picker) = PICKER.lock() {
        *picker = None;
    }
}

pub(super) fn picker_open() -> bool {
    PICKER.lock().is_ok_and(|picker| picker.is_some())
}

/// Text-commit and paste hook: committed text goes into the open picker's
/// filter. Returns false when the picker is closed.
pub(super) fn insert_picker_text(text: &str) -> bool {
    let Ok(mut guard) = PICKER.lock() else {
        return false;
    };
    let Some(picker) = guard.as_mut() else {
        return false;
    };
    if picker.query.insert(text) {
        picker.selected = 0;
    }
    true
}

/// The tickets and their state, as the reader has them.
#[derive(Debug, Clone, Default)]
pub(super) struct TicketList {
    pub(super) tickets: Arc<Vec<TicketRow>>,
    /// t3 projects for ticket-less threads; empty when the service has no
    /// project list.
    pub(super) projects: Arc<Vec<ProjectRow>>,
    pub(super) loading: bool,
    pub(super) offline: bool,
    pub(super) error: Option<String>,
}

fn ticket_list() -> TicketList {
    let Some(feed) = super::fork_threads_feed::feed_state() else {
        return TicketList {
            loading: true,
            ..TicketList::default()
        };
    };
    let error = feed.tickets_error.or(feed.error);
    TicketList {
        loading: !feed.tickets_loaded && error.is_none(),
        offline: error.is_some(),
        error,
        tickets: feed.tickets,
        projects: feed.projects,
    }
}

/// Test seam: what the reader would have delivered.
#[cfg(test)]
static TEST_LIST: Mutex<Option<TicketList>> = Mutex::new(None);

#[cfg(test)]
pub(super) fn set_tickets_for_test(list: TicketList) {
    if let Ok(mut test) = TEST_LIST.lock() {
        *test = Some(list);
    }
}

fn current_list() -> TicketList {
    #[cfg(test)]
    if let Some(list) = TEST_LIST.lock().ok().and_then(|list| list.clone()) {
        return list;
    }
    ticket_list()
}

/// A line of the picker list: a group header (`parked`, `projects`; not
/// selectable) or an entry, as its position in the item list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerLine {
    ParkedHeader,
    ProjectsHeader,
    Item(usize),
}

/// The list lines for `items` (in list order): started tickets, then a
/// `parked` header and the parked ones, then a `projects` header and the
/// projects. A header is left out when its group is empty.
pub(crate) fn picker_lines(list: &TicketList, items: &[PickerItem]) -> Vec<PickerLine> {
    let mut lines = Vec::with_capacity(items.len() + 2);
    let (mut parked, mut projects) = (false, false);
    for (position, item) in items.iter().enumerate() {
        match item {
            PickerItem::Ticket(index) if !parked && list.tickets[*index].parked() => {
                lines.push(PickerLine::ParkedHeader);
                parked = true;
            }
            PickerItem::Project(_) if !projects => {
                lines.push(PickerLine::ProjectsHeader);
                projects = true;
            }
            _ => {}
        }
        lines.push(PickerLine::Item(position));
    }
    lines
}

/// Everything the overlay draws.
pub(super) struct PickerView {
    pub(super) query: TextEditor,
    pub(super) list: TicketList,
    /// The matching entries, in display order.
    pub(super) items: Vec<PickerItem>,
    pub(super) selected: usize,
    /// What Enter does for the selected entry, in words.
    pub(super) action: Option<String>,
}

impl ClientShellState {
    /// Opens the picker when `key` is `new_key`. Returns true when it was.
    pub(super) fn fork_tickets_key(
        &mut self,
        key: &crate::input::TerminalKey,
        dispatch: crate::input::KeybindDispatch,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(keys) = self.config.threads.new_key.as_ref() else {
            return false;
        };
        let matched = match dispatch {
            crate::input::KeybindDispatch::Direct => keys.matches_direct_key(key),
            crate::input::KeybindDispatch::Prefix => keys.matches_prefix_key(key),
        };
        if matched && self.overlay.is_none() {
            if let Ok(mut picker) = PICKER.lock() {
                *picker = Some(TicketPicker::default());
            }
            outcome.repaint = true;
        }
        matched
    }

    /// Input hook while the picker is open: takes every key. Returns false
    /// when the picker is closed.
    pub(super) fn fork_tickets_picker_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crossterm::event::{KeyEventKind, KeyModifiers};
        if !picker_open() {
            return false;
        }
        if self.config.threads.new_key.is_none() || self.overlay.is_some() {
            close_picker();
            outcome.repaint = true;
            return false;
        }
        if key.kind == KeyEventKind::Release || matches!(key.code, KeyCode::Modifier(_)) {
            return true;
        }
        let list = current_list();
        let Ok(mut guard) = PICKER.lock() else {
            return true;
        };
        let Some(picker) = guard.as_mut() else {
            return true;
        };
        outcome.repaint = true;
        let filtered = picker_items(&list, &picker.query);
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        let ctrl = modifiers == KeyModifiers::CONTROL;
        let plain = modifiers.is_empty();
        match code {
            KeyCode::Esc => *guard = None,
            KeyCode::Char('c') if ctrl => *guard = None,
            KeyCode::Up if plain => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Char('p') if ctrl => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down if plain => {
                picker.selected = (picker.selected + 1).min(filtered.len().saturating_sub(1))
            }
            KeyCode::Char('n') if ctrl => {
                picker.selected = (picker.selected + 1).min(filtered.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                let item = filtered
                    .get(picker.selected.min(filtered.len().saturating_sub(1)))
                    .copied();
                if let Some(item) = item {
                    *guard = None;
                    drop(guard);
                    match item {
                        PickerItem::Ticket(index) => {
                            self.fork_tickets_open(&list.tickets[index].label, outcome)
                        }
                        PickerItem::Project(index) => {
                            self.fork_projects_open(&list.projects[index].id, outcome)
                        }
                    }
                }
            }
            _ => {
                if picker.query.handle_key(key) == Some(true) {
                    picker.selected = 0;
                }
            }
        }
        true
    }

    /// Mouse hook: the picker is modal; a click closes it, everything else
    /// is swallowed. Returns false when the picker is closed.
    pub(super) fn fork_tickets_mouse(
        &mut self,
        mouse: &crossterm::event::MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !picker_open() {
            return false;
        }
        if matches!(mouse.kind, crossterm::event::MouseEventKind::Down(_)) {
            close_picker();
            outcome.repaint = true;
        }
        true
    }

    /// Starts a new thread for the ticket labelled `label` on the thread
    /// machine.
    pub(super) fn fork_tickets_open(&mut self, label: &str, outcome: &mut ClientShellInput) {
        let list = current_list();
        let Some(ticket) = list.tickets.iter().find(|ticket| ticket.label == label) else {
            return;
        };
        let Some(endpoint_id) = self.fork_threads_machine(outcome) else {
            return;
        };
        let rows = fork_threads::view().rows;
        let template = self.config.threads.template.clone();
        let plan = self
            .fork_threads_snapshot(&endpoint_id)
            .map(|snapshot| plan_ticket_open(snapshot, &template, &rows, ticket));
        let command = ticket_command(&self.config.threads.new_command, &ticket.label);
        self.fork_threads_run(
            endpoint_id,
            plan,
            command,
            OpenTarget::NewForTicket(ticket.label.clone()),
            outcome,
        );
    }

    /// Starts a ticket-less thread in the project with id `project_id`, in a
    /// new workspace on the thread machine.
    pub(super) fn fork_projects_open(&mut self, project_id: &str, outcome: &mut ClientShellInput) {
        let list = current_list();
        let Some(project) = list
            .projects
            .iter()
            .find(|project| project.id == project_id)
        else {
            return;
        };
        let Some(endpoint_id) = self.fork_threads_machine(outcome) else {
            return;
        };
        let command = project_command(&self.config.threads.project_command, &project.root);
        self.fork_threads_run(
            endpoint_id,
            Some(plan_project_open(project)),
            command,
            OpenTarget::NewForProject(project.id.clone()),
            outcome,
        );
    }

    /// The picker as the overlay draws it, `None` when closed.
    pub(super) fn fork_tickets_view(&self) -> Option<PickerView> {
        let guard = PICKER.lock().ok()?;
        let picker = guard.as_ref()?;
        let list = current_list();
        let items = picker_items(&list, &picker.query);
        let selected = picker.selected.min(items.len().saturating_sub(1));
        let action = items.get(selected).map(|item| match item {
            PickerItem::Ticket(index) => self.fork_tickets_action(&list.tickets[*index]),
            PickerItem::Project(index) => {
                let project = &list.projects[*index];
                format!("new workspace {} in {}", project.title, project.root)
            }
        });
        Some(PickerView {
            query: picker.query.clone(),
            items,
            selected,
            action,
            list,
        })
    }

    fn fork_tickets_action(&self, ticket: &TicketRow) -> String {
        let threads = &self.config.threads;
        let Some(endpoint_id) =
            fork_threads::machine_endpoint_id(&threads.machine, &self.endpoints)
        else {
            return format!("no saved machine labelled \"{}\"", threads.machine);
        };
        let Some(snapshot) = self.fork_threads_snapshot(&endpoint_id) else {
            return "thread machine not connected".into();
        };
        let label_of = |workspace_id: &str| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .map(|workspace| workspace.label.clone())
                .unwrap_or_default()
        };
        let pane_workspace = |pane_id: &str| {
            snapshot
                .agents
                .iter()
                .find(|agent| agent.pane_id == pane_id)
                .map(|agent| label_of(&agent.workspace_id))
                .unwrap_or_default()
        };
        match plan_ticket_open(
            snapshot,
            &threads.template,
            &fork_threads::view().rows,
            ticket,
        ) {
            OpenPlan::Replace(pane_id) => {
                format!(
                    "new thread in place of the thread pane in {}",
                    pane_workspace(&pane_id)
                )
            }
            OpenPlan::AddTo(workspace_id) => {
                format!("new thread pane in {}", label_of(&workspace_id))
            }
            OpenPlan::Create { cwd, label } => match cwd {
                Some(cwd) => format!("new workspace {label} in {cwd}"),
                None => format!("new workspace {label}"),
            },
            OpenPlan::FocusPane(_) => String::new(),
        }
    }

    /// Composition hook: draws the picker over everything else.
    pub(super) fn fork_tickets_compose(
        &self,
        frame: &mut FrameData,
        occlusion: &mut crate::kitty_graphics::surface::Occlusion,
    ) {
        if self.overlay.is_some() {
            return;
        }
        let Some(view) = self.fork_tickets_view() else {
            return;
        };
        let Some(mut buffer) = frame.to_ratatui_buffer() else {
            return;
        };
        if let Some((area, cursor)) =
            render::fork_render_ticket_picker(&mut buffer, &view, &self.config.palette)
        {
            occlusion.cover(area);
            frame.replace_from_ratatui_buffer_preserving_effects(&buffer, cursor);
        }
    }
}
