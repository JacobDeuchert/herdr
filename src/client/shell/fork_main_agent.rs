//! fork: pinned Main agent row (FORK.md 0004).
//!
//! `[ui.main_agent] workspace = "<label>"` names a workspace on the local
//! server. The client hides it from every workspace list, draws one pinned row
//! for it at the top of the expanded sidebar, and binds `key` to toggle
//! between it and the previously shown workspace, on whichever machine that
//! was. herdr does not start the workspace; a plugin does that over the CLI.
//!
//! The hidden label sits in a process-wide cell instead of being threaded
//! through `workspace_entries`, so the upstream call sites stay untouched.

use super::*;
use crate::input::{KeybindDispatch, TerminalKey};
use std::sync::RwLock;

static HIDDEN_WORKSPACE_LABEL: RwLock<String> = RwLock::new(String::new());

/// Rows the pinned block takes from the top of the expanded sidebar: the row
/// itself and a spacer.
pub(super) const PINNED_ROWS: u16 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClientMainAgent {
    pub(super) workspace: String,
    pub(super) title: String,
    pub(super) key: Option<crate::config::ActionKeybinds>,
}

impl ClientMainAgent {
    /// Builds the client view of `[ui.main_agent]` and publishes the hidden
    /// workspace label. Returns a diagnostic when the key does not parse.
    pub(super) fn from_config(config: &crate::config::MainAgentConfig) -> (Self, Option<String>) {
        let (key, diagnostic) = match config.keybinds() {
            Ok(key) => (key, None),
            Err(diagnostic) => (None, Some(diagnostic)),
        };
        let main = Self {
            workspace: if config.enabled() {
                config.workspace.trim().to_string()
            } else {
                String::new()
            },
            title: config.display_title().to_string(),
            key,
        };
        set_hidden_workspace_label(&main.workspace);
        (main, diagnostic)
    }

    pub(super) fn enabled(&self) -> bool {
        !self.workspace.is_empty()
    }
}

pub(super) fn set_hidden_workspace_label(label: &str) {
    if let Ok(mut hidden) = HIDDEN_WORKSPACE_LABEL.write() {
        label.clone_into(&mut hidden);
    }
}

/// True for the Main workspace. Matches by label on any endpoint; a remote
/// workspace with the same label is hidden too.
pub(super) fn is_hidden_workspace(workspace: &ClientShellWorkspace) -> bool {
    HIDDEN_WORKSPACE_LABEL
        .read()
        .is_ok_and(|hidden| !hidden.is_empty() && workspace.label == *hidden)
}

fn main_workspace<'a>(
    snapshot: &'a ClientShellSnapshot,
    label: &str,
) -> Option<&'a ClientShellWorkspace> {
    if label.is_empty() {
        return None;
    }
    snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.label == label)
}

/// The local server's snapshot: the active one while local is shown,
/// otherwise the copy the client keeps for the local endpoint.
pub(super) fn local_snapshot<'a>(
    active: Option<&'a ClientShellSnapshot>,
    active_endpoint_id: &ClientEndpointId,
    endpoints: &'a [ClientShellEndpoint],
) -> Option<&'a ClientShellSnapshot> {
    active
        .filter(|_| active_endpoint_id.is_local())
        .or_else(|| {
            endpoints
                .iter()
                .find(|endpoint| endpoint.endpoint_id.is_local())
                .and_then(|endpoint| endpoint.snapshot.as_deref())
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MainAgentHit {
    pub(super) rect: Rect,
}

/// Draws the pinned row at the top of `area` and returns the rest of the
/// sidebar. Returns `area` unchanged when the feature is off or the sidebar
/// is too short to spare the rows.
pub(super) fn render_pinned_row(
    buffer: &mut Buffer,
    area: Rect,
    local: Option<&ClientShellSnapshot>,
    showing_local: bool,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> Rect {
    hits.fork_main_agent = None;
    let main = &config.main_agent;
    if !main.enabled() || area.height < PINNED_ROWS + 4 || area.width < 6 {
        return area;
    }
    let palette = &config.palette;
    let workspace = local.and_then(|snapshot| main_workspace(snapshot, &main.workspace));
    let focused = showing_local && workspace.is_some_and(|workspace| workspace.focused);
    // The last column is the sidebar divider.
    let row = Rect::new(area.x, area.y, area.width.saturating_sub(1), 1);
    if focused {
        buffer.set_style(
            row,
            Style::default().bg(super::sidebar::workspace_active_background(palette, false)),
        );
    }
    let (marker, marker_color, state) = if workspace.is_some() {
        ("◆", palette.accent, "running")
    } else {
        ("◇", palette.overlay0, "stopped")
    };
    super::render::put_text(
        buffer,
        row.x.saturating_add(1),
        row.y,
        2.min(row.width.saturating_sub(1)),
        marker,
        Style::default().fg(marker_color),
    );
    let state_width = (UnicodeWidthStr::width(state) as u16).saturating_add(2);
    let title_x = row.x.saturating_add(3);
    super::render::put_text(
        buffer,
        title_x,
        row.y,
        row.right()
            .saturating_sub(title_x)
            .saturating_sub(state_width),
        &main.title,
        Style::default()
            .fg(if focused {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(Modifier::BOLD),
    );
    let state_area = Rect::new(row.x, row.y, row.width.saturating_sub(1), 1);
    super::render::put_right_text(
        buffer,
        state_area,
        row.y,
        state,
        Style::default().fg(palette.overlay0),
    );
    if config.mouse_capture {
        hits.fork_main_agent = Some(MainAgentHit { rect: row });
    }
    Rect::new(
        area.x,
        area.y.saturating_add(PINNED_ROWS),
        area.width,
        area.height.saturating_sub(PINNED_ROWS),
    )
}

impl ClientShellState {
    fn fork_main_agent_workspace_id(&self) -> Option<String> {
        let snapshot = local_snapshot(
            self.snapshot.as_deref(),
            &self.active_endpoint_id,
            &self.endpoints,
        )?;
        main_workspace(snapshot, &self.config.main_agent.workspace)
            .map(|workspace| workspace.workspace_id.clone())
    }

    fn fork_main_agent_is_shown(&self) -> bool {
        self.active_endpoint_id.is_local()
            && self.snapshot.as_deref().is_some_and(|snapshot| {
                main_workspace(snapshot, &self.config.main_agent.workspace)
                    .is_some_and(|workspace| workspace.focused)
            })
    }

    /// Shows Main, remembering where the client was so the key can return.
    pub(super) fn fork_main_agent_focus(&mut self, outcome: &mut ClientShellInput) {
        if self.fork_main_agent_is_shown() {
            return;
        }
        let Some(workspace_id) = self.fork_main_agent_workspace_id() else {
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unavailable,
                "main_agent_missing",
                "Main agent not running",
                format!(
                    "No local workspace labelled \"{}\"",
                    self.config.main_agent.workspace
                ),
            );
            outcome.repaint = true;
            return;
        };
        self.fork_main_agent_return = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_workspace_id.clone())
            .map(|workspace_id| (self.active_endpoint_id.clone(), workspace_id));
        self.focus_or_activate(
            ClientEndpointId::Local,
            ClientEndpointFocusTarget::Workspace(workspace_id),
            outcome,
        );
    }

    /// `herdr main-agent show` (server `client.main_agent.show`): show Main,
    /// never toggle away from it. Lets a window-manager key focus the herdr
    /// window and show Main without knowing what the client is showing.
    pub(crate) fn fork_main_agent_show_from_server(&mut self) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        self.fork_main_agent_focus(&mut outcome);
        outcome.repaint = true;
        outcome
    }

    /// Toggle: Main when elsewhere, back to the remembered place when on Main.
    pub(super) fn fork_main_agent_toggle(&mut self, outcome: &mut ClientShellInput) {
        if !self.fork_main_agent_is_shown() {
            self.fork_main_agent_focus(outcome);
            return;
        }
        if let Some((endpoint_id, workspace_id)) = self.fork_main_agent_return.take() {
            self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Workspace(workspace_id),
                outcome,
            );
        }
    }

    /// Handles a press on the pinned row. Returns true when it was one.
    pub(super) fn fork_main_agent_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let hit = self
            .hits
            .fork_main_agent
            .as_ref()
            .is_some_and(|hit| super::contains(hit.rect, point));
        if hit {
            self.fork_main_agent_focus(outcome);
        }
        hit
    }

    /// Handles the toggle key. Returns true when `key` was it.
    pub(super) fn fork_main_agent_key(
        &mut self,
        key: &TerminalKey,
        dispatch: KeybindDispatch,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(keys) = self.config.main_agent.key.as_ref() else {
            return false;
        };
        let matched = match dispatch {
            KeybindDispatch::Direct => keys.matches_direct_key(key),
            KeybindDispatch::Prefix => keys.matches_prefix_key(key),
        };
        if matched {
            self.fork_main_agent_toggle(outcome);
        }
        matched
    }
}
