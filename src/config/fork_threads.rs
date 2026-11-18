//! fork: `[ui.threads]`, the agent thread list in the sidebar (FORK.md 0005).
//!
//! The client reads a thread list from an external HTTP service (`url`) and
//! draws one sidebar row per thread. Each row opens the thread in a workspace
//! on the server named by `machine`, running `command` there. Empty `url`
//! (the default) disables everything.
//!
//! `new_key` opens a ticket picker; picking a ticket starts a new thread for
//! it with `new_command`. Empty `new_key` (the default) leaves it off.

use serde::{Deserialize, Serialize};

/// Placeholder in `command` that is replaced by the thread id.
pub const THREAD_PLACEHOLDER: &str = "{thread}";

/// Placeholder in `new_command` that is replaced by the ticket label
/// (`t<digits>`, validated before substitution).
pub const TICKET_PLACEHOLDER: &str = "{ticket}";

/// Placeholder in `project_command` that is replaced by the project root,
/// single-quoted (roots with `'` are refused before substitution).
pub const DIR_PLACEHOLDER: &str = "{dir}";

const DEFAULT_COMMAND: &str = "t3t -r {thread}";
/// Typed into the new pane's shell (`pane.run_command`), so shell syntax
/// works. `tk dir` prints the ticket dir and creates it when missing.
const DEFAULT_NEW_COMMAND: &str = r#"d="$(tk dir {ticket})" && cd "$d" && exec t3t"#;
/// Typed into the new pane's shell for a ticket-less thread in a project.
const DEFAULT_PROJECT_COMMAND: &str = "cd {dir} && exec t3t";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ThreadsConfig {
    /// Base URL of the thread service, plain `http://host:port`. The client
    /// reads `GET <url>/api/sessions` and follows `GET <url>/api/events`.
    /// Empty disables the thread list.
    pub url: String,
    /// Label of the saved machine whose server owns thread workspaces. Empty
    /// means the local server.
    pub machine: String,
    /// Command a thread pane runs; `{thread}` is replaced by the thread id.
    /// The same pattern recognises which thread an existing pane shows, from
    /// the resume command the pane reported.
    pub command: String,
    /// Binding that opens the ticket picker, e.g. "prefix+t". Empty (default)
    /// leaves the picker off.
    pub new_key: String,
    /// Command a new thread for a picked ticket runs; `{ticket}` is replaced
    /// by the ticket label. Typed into the pane's shell.
    pub new_command: String,
    /// Command a new thread in a project (the picker's `projects` group)
    /// runs; `{dir}` is replaced by the single-quoted project root. Typed
    /// into the pane's shell.
    pub project_command: String,
}

impl Default for ThreadsConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            machine: String::new(),
            command: DEFAULT_COMMAND.into(),
            new_key: String::new(),
            new_command: DEFAULT_NEW_COMMAND.into(),
            project_command: DEFAULT_PROJECT_COMMAND.into(),
        }
    }
}

impl ThreadsConfig {
    pub fn enabled(&self) -> bool {
        !self.url.trim().is_empty()
    }

    /// Diagnostic for a config that is on but cannot work.
    pub fn diagnostic(&self) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let url = self.url.trim();
        if !url.starts_with("http://") {
            return Some(format!(
                "ui.threads.url must be a plain http:// URL: {url}; thread list disabled"
            ));
        }
        let command = self.command.trim();
        if command.split_whitespace().count() < 2
            || !command
                .split_whitespace()
                .skip(1)
                .any(|word| word.contains(THREAD_PLACEHOLDER))
        {
            return Some(format!(
                "ui.threads.command needs a program and an argument containing {THREAD_PLACEHOLDER}: {command}; thread list disabled"
            ));
        }
        None
    }

    /// Parsed `new_key`, or a diagnostic when the picker cannot work. `None`
    /// when the key is unset or the thread list is off.
    pub fn new_keybinds(&self) -> Result<Option<super::ActionKeybinds>, String> {
        let key = self.new_key.trim();
        if key.is_empty() || !self.enabled() {
            return Ok(None);
        }
        let command = self.new_command.trim();
        if command.is_empty() || command.chars().any(char::is_control) {
            return Err(
                "ui.threads.new_command must be one non-empty line; ticket picker disabled".into(),
            );
        }
        let project = self.project_command.trim();
        if project.is_empty() || project.chars().any(char::is_control) {
            return Err(
                "ui.threads.project_command must be one non-empty line; ticket picker disabled"
                    .into(),
            );
        }
        super::ActionKeybinds::from_labels(&[key.to_string()])
            .map(Some)
            .map_err(|_| format!("invalid ui.threads.new_key: {key}; ticket picker disabled"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_config_defaults_to_disabled() {
        let config = ThreadsConfig::default();
        assert!(!config.enabled());
        assert!(config.diagnostic().is_none());
        assert_eq!(config.command, "t3t -r {thread}");
        assert!(config.new_key.is_empty());
        assert_eq!(
            config.new_command,
            r#"d="$(tk dir {ticket})" && cd "$d" && exec t3t"#
        );
        assert_eq!(config.project_command, "cd {dir} && exec t3t");
        assert!(config.new_keybinds().unwrap().is_none());
    }

    #[test]
    fn threads_config_parses_the_ticket_picker_key() {
        let config: crate::config::Config =
            toml::from_str("[ui.threads]\nurl = \"http://h:1\"\nnew_key = \"prefix+t\"\n").unwrap();
        assert!(config.ui.threads.new_keybinds().unwrap().is_some());
        let bad_key = ThreadsConfig {
            url: "http://h:1".into(),
            new_key: "prefix+".into(),
            ..ThreadsConfig::default()
        };
        assert!(bad_key.new_keybinds().is_err());
        let no_command = ThreadsConfig {
            url: "http://h:1".into(),
            new_key: "prefix+t".into(),
            new_command: " ".into(),
            ..ThreadsConfig::default()
        };
        assert!(no_command.new_keybinds().is_err());
        let threads_off = ThreadsConfig {
            new_key: "prefix+t".into(),
            ..ThreadsConfig::default()
        };
        assert!(threads_off.new_keybinds().unwrap().is_none());
    }

    #[test]
    fn threads_config_parses_from_ui_section() {
        let config: crate::config::Config = toml::from_str(
            "[ui.threads]\nurl = \"http://192.168.1.2:7420\"\nmachine = \"devvm\"\n",
        )
        .unwrap();
        let threads = &config.ui.threads;
        assert!(threads.enabled());
        assert_eq!(threads.machine, "devvm");
        assert_eq!(threads.command, "t3t -r {thread}");
        assert!(threads.diagnostic().is_none());
    }

    #[test]
    fn threads_config_rejects_https_and_commands_without_placeholder() {
        let https = ThreadsConfig {
            url: "https://example".into(),
            ..ThreadsConfig::default()
        };
        assert!(https.diagnostic().is_some());
        let no_placeholder = ThreadsConfig {
            url: "http://example".into(),
            command: "t3t".into(),
            ..ThreadsConfig::default()
        };
        assert!(no_placeholder.diagnostic().is_some());
        let placeholder_as_program = ThreadsConfig {
            url: "http://example".into(),
            command: "{thread}".into(),
            ..ThreadsConfig::default()
        };
        assert!(placeholder_as_program.diagnostic().is_some());
    }
}
