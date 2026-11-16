//! fork: `pane.run_command` (FORK.md 0005).

use serde::{Deserialize, Serialize};

/// Runs a shell command line in a fresh pane, which is placed by the first
/// target that is set:
///
/// - `replace_pane_id`: takes that pane's place (split next to it, then close
///   it), so a long-running program restarts in place on new arguments;
/// - `workspace_id`: split off that workspace's focused pane;
/// - neither: a new workspace with `cwd` and `label`.
///
/// The command is typed into the new pane's shell, the same way a restored
/// agent session is resumed. Answers with the new pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneRunCommandParams {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub focus: bool,
}
