//! fork: `[ui.threads]`, the agent thread list in the sidebar (FORK.md 0005).
//!
//! The client reads a thread list from an external HTTP service (`url`) and
//! draws one sidebar row per thread. Each row opens the thread in a workspace
//! on the server named by `machine`, running `command` there. Empty `url`
//! (the default) disables everything.

use serde::{Deserialize, Serialize};

/// Placeholder in `command` that is replaced by the thread id.
pub const THREAD_PLACEHOLDER: &str = "{thread}";

const DEFAULT_COMMAND: &str = "t3t -r {thread}";

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
}

impl Default for ThreadsConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            machine: String::new(),
            command: DEFAULT_COMMAND.into(),
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
