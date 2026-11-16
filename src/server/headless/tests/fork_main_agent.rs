//! fork: FORK.md 0004, server half of `herdr main-agent`.
use super::*;

fn shell_client_server() -> (HeadlessServer, std::sync::mpsc::Receiver<Vec<u8>>) {
    let mut server = test_headless_server();
    let mut main = crate::workspace::Workspace::test_new("main");
    main.custom_name = Some("Main".into());
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("herd"), main];
    server.app.state.active = Some(1);
    server.app.state.selected = 1;

    let (client_tx, control_rx, _render_rx) = test_client_writer();
    let mut client = ClientConnection::new(
        (80, 24),
        crate::kitty_graphics::HostCellSize::default(),
        1,
        RenderEncoding::SemanticFrame,
        Some(client_tx),
    );
    client.mode = ClientConnectionMode::ClientShell;
    client.shell_surface_active = true;
    server.clients.insert(1, client);
    server.promote_client_to_foreground(1);
    drain_window_titles(&control_rx);
    (server, control_rx)
}

fn next_control(control_rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let bytes = control_rx.recv_timeout(remaining).ok()?;
        if let ServerMessage::EndpointControl { kind, .. } = read_server_message(bytes) {
            return Some(kind);
        }
    }
    None
}

fn result(response: String) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(&response).unwrap()["result"].clone()
}

#[test]
fn main_agent_show_forwards_a_control_to_the_client_shell() {
    let (mut server, control_rx) = shell_client_server();

    let response = result(server.handle_client_main_agent_show_api("show".into()));

    assert_eq!(response["sent"], true);
    assert_eq!(
        next_control(&control_rx).as_deref(),
        Some(crate::protocol::endpoint::FORK_MAIN_AGENT_SHOW_KIND)
    );
    shutdown_test_runtimes(&mut server);
}

#[test]
fn main_agent_show_reaches_a_client_showing_another_machine() {
    let (mut server, control_rx) = shell_client_server();
    server.clients.get_mut(&1).unwrap().shell_surface_active = false;
    server.foreground_client_id = None;

    let response = result(server.handle_client_main_agent_show_api("show".into()));

    assert_eq!(response["sent"], true);
    assert_eq!(
        next_control(&control_rx).as_deref(),
        Some(crate::protocol::endpoint::FORK_MAIN_AGENT_SHOW_KIND)
    );
    shutdown_test_runtimes(&mut server);
}

#[test]
fn main_agent_show_without_a_client_reports_not_sent() {
    let mut server = test_headless_server();

    let response = result(server.handle_client_main_agent_show_api("show".into()));

    assert_eq!(response["sent"], false);
    shutdown_test_runtimes(&mut server);
}

#[test]
fn main_agent_client_view_reports_label_and_whether_this_server_is_shown() {
    let (mut server, _control_rx) = shell_client_server();

    let view = result(server.handle_client_view_get_api("view".into()));
    assert_eq!(view["attached"], true);
    assert_eq!(view["active"], true);
    assert_eq!(view["workspace_label"], "Main");

    server.clients.get_mut(&1).unwrap().shell_surface_active = false;
    let view = result(server.handle_client_view_get_api("view".into()));
    assert_eq!(view["attached"], true);
    assert_eq!(view["active"], false);

    server.clients.clear();
    server.foreground_client_id = None;
    let view = result(server.handle_client_view_get_api("view".into()));
    assert_eq!(view["attached"], false);
    assert_eq!(view["active"], false);
    shutdown_test_runtimes(&mut server);
}
