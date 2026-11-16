//! fork: FORK.md 0004, pinned Main agent row.
use super::*;

fn main_config() -> ClientShellConfig {
    let config: Config =
        toml::from_str("[ui.main_agent]\nworkspace = \"Main\"\nkey = \"prefix+m\"\n").unwrap();
    ClientShellConfig::from_config(&config)
}

/// ws_1 focused, ws_2 is Main, ws_3 an ordinary workspace.
fn main_snapshot(focused: &str, with_main: bool) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.focused_workspace_id = Some(focused.into());
    projected.workspaces = (1..=3)
        .map(|number| {
            let mut workspace = projected.workspaces[0].clone();
            workspace.workspace_id = format!("ws_{number}");
            workspace.number = number;
            workspace.focused = workspace.workspace_id == focused;
            if number == 2 && with_main {
                workspace.label = "Main".into();
                workspace.custom_label = true;
            }
            workspace
        })
        .collect();
    projected
}

fn main_state(focused: &str, with_main: bool) -> ClientShellState {
    let mut state = ClientShellState::new(main_config());
    state.config.palette = Palette::terminal();
    state.set_snapshot(Box::new(main_snapshot(focused, with_main)));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    state
}

fn focused_workspace_request(outcome: &ClientShellInput) -> String {
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!(
            "expected one workspace focus request, got {:?}",
            outcome.actions
        );
    };
    let crate::api::schema::Method::WorkspaceFocus(target) = &request.method else {
        panic!("expected workspace focus, got {:?}", request.method);
    };
    target.workspace_id.clone()
}

#[test]
fn main_agent_workspace_is_hidden_and_pinned() {
    let mut state = main_state("ws_1", true);
    let frame = state.compose(100, 28).unwrap();
    let rows = frame_rows(&frame);
    assert!(rows[0].contains("Main"), "{:?}", rows[0]);
    assert!(rows[0].contains("running"), "{:?}", rows[0]);
    let listed = state
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.workspace_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(listed, ["ws_1", "ws_3"]);
    assert!(state.hits.fork_main_agent.is_some());
}

#[test]
fn main_agent_row_shows_stopped_without_workspace() {
    let mut state = main_state("ws_1", false);
    let frame = state.compose(100, 28).unwrap();
    assert!(frame_rows(&frame)[0].contains("stopped"));
    assert_eq!(state.hits.workspaces.len(), 3);

    let outcome = state.handle_input_bytes(&[0x02]);
    assert!(outcome.actions.is_empty());
    let outcome = state.handle_input_bytes(b"m");
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
}

#[test]
fn main_agent_key_toggles_between_main_and_previous_workspace() {
    let mut state = main_state("ws_1", true);
    state.handle_input_bytes(&[0x02]);
    let outcome = state.handle_input_bytes(b"m");
    assert_eq!(focused_workspace_request(&outcome), "ws_2");
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert_eq!(
        state.fork_main_agent_return,
        Some((ClientEndpointId::Local, "ws_1".to_string()))
    );

    let mut on_main = main_snapshot("ws_2", true);
    on_main.revision = 2;
    state.set_snapshot(Box::new(on_main));
    state.handle_input_bytes(&[0x02]);
    let outcome = state.handle_input_bytes(b"m");
    assert_eq!(focused_workspace_request(&outcome), "ws_1");
    assert!(state.fork_main_agent_return.is_none());
}

#[test]
fn main_agent_row_click_focuses_main() {
    let mut state = main_state("ws_1", true);
    let rect = state.hits.fork_main_agent.as_ref().unwrap().rect;
    let press = format!("\x1b[<0;{};{}M", rect.x + 2, rect.y + 1);
    let outcome = state.handle_input_bytes(press.as_bytes());
    assert_eq!(focused_workspace_request(&outcome), "ws_2");
}

#[test]
fn main_agent_disabled_by_default_changes_nothing() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.palette = Palette::terminal();
    state.set_snapshot(Box::new(main_snapshot("ws_1", true)));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    assert!(state.hits.fork_main_agent.is_none());
    assert_eq!(state.hits.workspaces.len(), 3);
}

#[test]
fn main_agent_show_from_server_focuses_main_and_never_toggles_back() {
    let mut state = main_state("ws_1", true);
    let outcome = state.fork_main_agent_show_from_server();
    assert_eq!(focused_workspace_request(&outcome), "ws_2");

    // Already on Main: `herdr main-agent show` must leave it there.
    state.set_snapshot(Box::new(main_snapshot("ws_2", true)));
    let outcome = state.fork_main_agent_show_from_server();
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
}
