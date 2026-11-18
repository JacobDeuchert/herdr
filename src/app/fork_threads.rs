//! fork: server half of the sidebar thread list (FORK.md 0005).
//!
//! A thread belongs to the workspace it was started in. The client learns
//! which thread a pane shows from the resume command the pane reported
//! (`pane.report_agent` with `resume_argv`), and which threads a workspace
//! ever showed from a history the server keeps here:
//!
//! - every accepted resume command is appended to the workspace's metadata
//!   token `fork.sessions` (one command per line, newest last, capped), so the
//!   mapping outlives the pane and dies with the workspace;
//! - the client-shell projection adds the pane's current resume command as
//!   the agent token `fork.resume`.
//!
//! - a workspace `pane.run_command` creates carries the token `fork.draft`
//!   (its label) until a pane in it reports a resume command: a fresh t3t is
//!   a draft that reports its thread only after the first prompt, and the
//!   client hides the workspace from the start and shows a draft row for it.
//!
//! All three keys contain a dot, which `*.report_metadata` rejects, so API callers
//! can neither forge nor clear them. The history is in-memory only, like all
//! metadata tokens: after a server restart it is rebuilt as restored agents
//! report again.
//!
//! `pane.run_command` opens a thread: it composes `pane.split` /
//! `workspace.create`, `pane.close`, `pane.send_text` and `pane.focus` through
//! the regular API dispatcher instead of touching layout internals.

use crate::api::schema::{
    Method, PaneInfo, PaneRunCommandParams, PaneSendTextParams, PaneSplitParams, PaneTarget,
    Request, ResponseResult, SplitDirection, SuccessResponse, WorkspaceCreateParams,
};

/// Workspace token: resume commands reported in the workspace, one per line.
pub(crate) const SESSIONS_TOKEN: &str = "fork.sessions";
/// Agent token in the client-shell projection: the pane's resume command.
pub(crate) const RESUME_TOKEN: &str = "fork.resume";
/// Workspace token: set (to the workspace label) on a workspace created by
/// `pane.run_command`, removed once a pane in it reports a resume command.
pub(crate) const DRAFT_TOKEN: &str = "fork.draft";
const MAX_SESSIONS: usize = 64;
const MAX_COMMAND_BYTES: usize = 4096;

fn resume_line(argv: &[String]) -> Option<String> {
    let line = argv.join(" ");
    (!line.trim().is_empty() && !line.chars().any(char::is_control)).then_some(line)
}

/// `history` with `line` moved or appended to the end, oldest dropped.
pub(crate) fn push_session_line(history: Option<&str>, line: &str) -> String {
    let mut lines = history
        .unwrap_or_default()
        .lines()
        .filter(|existing| *existing != line && !existing.is_empty())
        .collect::<Vec<_>>();
    lines.push(line);
    let skip = lines.len().saturating_sub(MAX_SESSIONS);
    lines[skip..].join("\n")
}

impl crate::app::AppState {
    /// Hook after a resume report: records the pane's current resume command
    /// in its workspace's history.
    pub(crate) fn fork_record_session_command(&mut self, pane_id: crate::layout::PaneId) {
        let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.pane_state(pane_id).is_some())
        else {
            return;
        };
        let Some(line) = workspace
            .pane_state(pane_id)
            .and_then(|pane| self.terminals.get(&pane.attached_terminal_id))
            .and_then(|terminal| terminal.reported_resume())
            .and_then(|resume| resume_line(&resume.argv))
        else {
            return;
        };
        let values = workspace.metadata_tokens.values();
        let history = push_session_line(values.get(SESSIONS_TOKEN).map(String::as_str), &line);
        let mut patch = std::collections::HashMap::new();
        if values.get(SESSIONS_TOKEN) != Some(&history) {
            patch.insert(SESSIONS_TOKEN.to_owned(), Some(history));
        }
        if values.contains_key(DRAFT_TOKEN) {
            patch.insert(DRAFT_TOKEN.to_owned(), None);
        }
        if !patch.is_empty() {
            workspace
                .metadata_tokens
                .patch(patch, None, std::time::Instant::now());
        }
    }
}

impl crate::app::App {
    /// The pane's current resume command, for the client-shell projection.
    pub(crate) fn fork_pane_resume_line(&self, public_pane_id: &str) -> Option<String> {
        let (ws_idx, pane_id) = self.parse_pane_id(public_pane_id)?;
        let pane = self.state.workspaces.get(ws_idx)?.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
        resume_line(&terminal.reported_resume()?.argv)
    }

    /// Tags a workspace `pane.run_command` created as a draft, with the
    /// requested label (empty: the client falls back to the workspace's).
    fn fork_mark_draft(&mut self, public_workspace_id: &str, label: Option<&str>) {
        let Some(workspace) = self
            .parse_workspace_id(public_workspace_id)
            .and_then(|index| self.state.workspaces.get_mut(index))
        else {
            return;
        };
        let label = label
            .unwrap_or_default()
            .chars()
            .filter(|ch| !ch.is_control())
            .collect::<String>();
        workspace.metadata_tokens.patch(
            [(DRAFT_TOKEN.to_owned(), Some(label))].into(),
            None,
            std::time::Instant::now(),
        );
    }

    fn fork_call(&mut self, method: Method) -> Result<ResponseResult, String> {
        let response = self.handle_api_request_after_internal_events_drained(Request {
            id: "fork:pane.run_command".into(),
            method,
        });
        match serde_json::from_str::<SuccessResponse>(&response) {
            Ok(success) => Ok(success.result),
            Err(_) => Err(serde_json::from_str::<serde_json::Value>(&response)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/error/message")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(response)),
        }
    }

    pub(crate) fn fork_handle_pane_run_command(
        &mut self,
        id: String,
        params: PaneRunCommandParams,
    ) -> String {
        match self.fork_run_command(params) {
            Ok(pane) => serde_json::to_string(&SuccessResponse {
                id,
                result: ResponseResult::PaneInfo { pane },
            }),
            Err(message) => serde_json::to_string(&crate::api::schema::ErrorResponse {
                id,
                error: crate::api::schema::ErrorBody {
                    code: "pane_run_failed".into(),
                    message,
                },
            }),
        }
        .unwrap_or_else(|_| "{}".into())
    }

    fn fork_run_command(&mut self, params: PaneRunCommandParams) -> Result<PaneInfo, String> {
        let command = params.command.trim();
        if command.is_empty()
            || command.len() > MAX_COMMAND_BYTES
            || command.chars().any(char::is_control)
        {
            return Err("command must be one non-empty line".into());
        }
        let pane = if let Some(replace) = params.replace_pane_id.clone() {
            let ResponseResult::PaneInfo { pane } =
                self.fork_call(Method::PaneSplit(PaneSplitParams {
                    workspace_id: None,
                    target_pane_id: Some(replace.clone()),
                    direction: SplitDirection::Right,
                    ratio: None,
                    cwd: None,
                    focus: params.focus,
                    right_click: Default::default(),
                    env: Default::default(),
                }))?
            else {
                return Err("pane.split answered unexpectedly".into());
            };
            self.fork_call(Method::PaneClose(PaneTarget { pane_id: replace }))?;
            pane
        } else if let Some(workspace_id) = params.workspace_id.clone() {
            let ResponseResult::PaneInfo { pane } =
                self.fork_call(Method::PaneSplit(PaneSplitParams {
                    workspace_id: Some(workspace_id),
                    target_pane_id: None,
                    direction: SplitDirection::Right,
                    ratio: None,
                    cwd: None,
                    focus: params.focus,
                    right_click: Default::default(),
                    env: Default::default(),
                }))?
            else {
                return Err("pane.split answered unexpectedly".into());
            };
            pane
        } else {
            let ResponseResult::WorkspaceCreated { root_pane, .. } =
                self.fork_call(Method::WorkspaceCreate(WorkspaceCreateParams {
                    source_workspace_id: None,
                    cwd: params
                        .cwd
                        .clone()
                        .filter(|cwd| std::path::Path::new(cwd).is_dir()),
                    focus: params.focus,
                    label: params
                        .label
                        .clone()
                        .map(|label| label.chars().filter(|ch| !ch.is_control()).collect())
                        .filter(|label: &String| !label.trim().is_empty()),
                    env: Default::default(),
                }))?
            else {
                return Err("workspace.create answered unexpectedly".into());
            };
            self.fork_mark_draft(&root_pane.workspace_id, params.label.as_deref());
            root_pane
        };
        self.fork_call(Method::PaneSendText(PaneSendTextParams {
            pane_id: pane.pane_id.clone(),
            text: format!("{command}\r"),
        }))?;
        if params.focus {
            self.fork_call(Method::PaneFocus(PaneTarget {
                pane_id: pane.pane_id.clone(),
            }))?;
        }
        Ok(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_session_history_moves_repeats_to_the_end_and_caps() {
        assert_eq!(push_session_line(None, "t3t -r a"), "t3t -r a");
        let history = push_session_line(Some("t3t -r a\nt3t -r b"), "t3t -r a");
        assert_eq!(history, "t3t -r b\nt3t -r a");
        let mut history = String::new();
        for index in 0..100 {
            history = push_session_line(Some(&history), &format!("t3t -r {index}"));
        }
        let lines = history.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), MAX_SESSIONS);
        assert_eq!(lines.last(), Some(&"t3t -r 99"));
    }
}
