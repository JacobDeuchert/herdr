//! fork: `[ui.main_agent]`, the pinned Main agent row (FORK.md 0004).
//!
//! The client hides the local workspace labelled `workspace` from every
//! workspace list and draws one pinned row for it at the top of the expanded
//! sidebar instead. `key` toggles between that workspace and the place the
//! client showed before. Creating and restarting the workspace is not herdr's
//! job; a plugin does that over the CLI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct MainAgentConfig {
    /// Label of the local workspace that holds the Main agent. Empty disables
    /// the pinned row, the hiding, and the key.
    pub workspace: String,
    /// Text shown in the pinned row. Default: the workspace label.
    pub title: String,
    /// Binding that toggles between Main and the previous location, e.g.
    /// "prefix+m". Empty leaves it unbound.
    pub key: String,
}

impl MainAgentConfig {
    pub fn enabled(&self) -> bool {
        !self.workspace.trim().is_empty()
    }

    pub fn display_title(&self) -> &str {
        let title = self.title.trim();
        if title.is_empty() {
            self.workspace.trim()
        } else {
            title
        }
    }

    /// Parsed `key`, or a diagnostic when it does not parse.
    pub fn keybinds(&self) -> Result<Option<super::ActionKeybinds>, String> {
        let key = self.key.trim();
        if key.is_empty() || !self.enabled() {
            return Ok(None);
        }
        super::ActionKeybinds::from_labels(&[key.to_string()])
            .map(Some)
            .map_err(|_| format!("invalid ui.main_agent.key: {key}; ignoring key"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_agent_config_defaults_to_disabled() {
        let config = MainAgentConfig::default();
        assert!(!config.enabled());
        assert!(config.keybinds().unwrap().is_none());
    }

    #[test]
    fn main_agent_config_parses_from_ui_section() {
        let config: crate::config::Config =
            toml::from_str("[ui.main_agent]\nworkspace = \"Main\"\nkey = \"prefix+m\"\n").unwrap();
        let main = &config.ui.main_agent;
        assert!(main.enabled());
        assert_eq!(main.display_title(), "Main");
        assert!(main.keybinds().unwrap().is_some());
    }

    #[test]
    fn main_agent_config_rejects_bad_key() {
        let config = MainAgentConfig {
            workspace: "Main".into(),
            title: String::new(),
            key: "prefix+".into(),
        };
        assert!(config.keybinds().is_err());
    }
}
