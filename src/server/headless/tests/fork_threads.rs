//! fork: FORK.md 0005, server half of the sidebar thread list.
use super::*;

fn report_resume(server: &mut HeadlessServer, pane_id: &str, seq: u64, thread: &str) {
    completion_guard_api_report(
        server,
        api::schema::Method::PaneReportAgent(api::schema::PaneReportAgentParams {
            pane_id: pane_id.into(),
            source: "t3code-tui".into(),
            agent: "t3code".into(),
            state: api::schema::PaneAgentState::Idle,
            message: None,
            seq: Some(seq),
            agent_session_id: None,
            agent_session_path: None,
            resume_argv: Some(vec!["t3t".into(), "-r".into(), thread.into()]),
        }),
    );
}

fn call(server: &mut HeadlessServer, method: api::schema::Method) -> serde_json::Value {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "fork-threads".into(),
            method,
        },
        respond_to,
        response_write_complete: None,
    });
    let response = response_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn token<'a>(tokens: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tokens
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

#[test]
fn threads_resume_reports_build_a_workspace_history_and_pane_token() {
    let (writer, _control_rx, _render_rx) = test_client_writer();
    let (mut server, pane_id) = completion_guard_server(writer);
    let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();

    report_resume(&mut server, &public_pane_id, 1, "first");
    report_resume(&mut server, &public_pane_id, 2, "second");
    // Repeating a thread moves it to the end instead of duplicating it.
    report_resume(&mut server, &public_pane_id, 3, "first");

    let snapshot = crate::server::client_shell::snapshot(&server.app, "boot", 1, None, None);
    let workspace = &snapshot.workspaces[0];
    assert_eq!(
        token(&workspace.tokens, crate::app::FORK_SESSIONS_TOKEN),
        Some("t3t -r second\nt3t -r first")
    );
    assert_eq!(
        token(
            &snapshot.workspaces[1].tokens,
            crate::app::FORK_SESSIONS_TOKEN
        ),
        None
    );
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == public_pane_id)
        .expect("reported pane is an agent");
    assert_eq!(
        token(&agent.tokens, crate::app::FORK_RESUME_TOKEN),
        Some("t3t -r first")
    );
}

#[test]
fn threads_history_keys_cannot_be_written_through_the_api() {
    let (writer, _control_rx, _render_rx) = test_client_writer();
    let (mut server, _) = completion_guard_server(writer);
    let workspace_id = server.app.public_workspace_id(0);
    let response = call(
        &mut server,
        api::schema::Method::WorkspaceReportMetadata(api::schema::WorkspaceReportMetadataParams {
            workspace_id,
            source: "spoof".into(),
            tokens: [(
                crate::app::FORK_SESSIONS_TOKEN.to_owned(),
                Some("t3t -r x".into()),
            )]
            .into(),
            seq: None,
            ttl_ms: None,
        }),
    );
    assert!(response.get("error").is_some(), "{response}");
}

#[tokio::test]
async fn threads_run_command_restarts_a_pane_in_place() {
    let (writer, _control_rx, _render_rx) = test_client_writer();
    let (mut server, pane_id) = completion_guard_server(writer);
    server.app.state.default_shell = "/bin/sh".into();
    let old_pane = server.app.public_pane_id(0, pane_id).unwrap();

    let response = call(
        &mut server,
        api::schema::Method::PaneRunCommand(api::schema::PaneRunCommandParams {
            command: "true".into(),
            replace_pane_id: Some(old_pane.clone()),
            workspace_id: None,
            cwd: None,
            label: None,
            focus: true,
        }),
    );
    let pane = &response["result"]["pane"];
    let new_pane = pane["pane_id"].as_str().expect("new pane").to_owned();
    assert_ne!(new_pane, old_pane);
    let workspace = &server.app.state.workspaces[0];
    assert_eq!(
        workspace.tabs[0].layout.pane_ids().len(),
        1,
        "old pane closed"
    );
    assert!(server.app.parse_pane_id(&old_pane).is_none());
    assert_eq!(
        server.app.state.active,
        Some(0),
        "focus moved to the thread"
    );
    assert_eq!(
        server
            .app
            .public_pane_id(0, workspace.focused_pane_id().unwrap()),
        Some(new_pane)
    );
}

#[tokio::test]
async fn threads_run_command_creates_a_labelled_workspace() {
    let (writer, _control_rx, _render_rx) = test_client_writer();
    let (mut server, _) = completion_guard_server(writer);
    server.app.state.default_shell = "/bin/sh".into();
    let cwd = std::env::temp_dir().display().to_string();

    let response = call(
        &mut server,
        api::schema::Method::PaneRunCommand(api::schema::PaneRunCommandParams {
            command: "true".into(),
            replace_pane_id: None,
            workspace_id: None,
            cwd: Some(cwd),
            label: Some("PROJ-7\u{1b}".into()),
            focus: true,
        }),
    );
    assert!(
        response["result"]["pane"]["pane_id"].is_string(),
        "{response}"
    );
    assert_eq!(server.app.state.workspaces.len(), 3);
    assert_eq!(
        server.app.state.workspaces[2].custom_name.as_deref(),
        Some("PROJ-7")
    );
    assert_eq!(server.app.state.active, Some(2));
    // The new workspace is a draft until a pane in it reports a resume.
    let draft = |server: &HeadlessServer| {
        let snapshot = crate::server::client_shell::snapshot(&server.app, "boot", 1, None, None);
        token(&snapshot.workspaces[2].tokens, crate::app::FORK_DRAFT_TOKEN).map(str::to_owned)
    };
    assert_eq!(draft(&server).as_deref(), Some("PROJ-7"));
    let pane_id = response["result"]["pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    report_resume(&mut server, &pane_id, 1, "fresh");
    assert_eq!(draft(&server), None);

    let rejected = call(
        &mut server,
        api::schema::Method::PaneRunCommand(api::schema::PaneRunCommandParams {
            command: "echo a\necho b".into(),
            replace_pane_id: None,
            workspace_id: None,
            cwd: None,
            label: None,
            focus: false,
        }),
    );
    assert_eq!(rejected["error"]["code"], "pane_run_failed");
    assert_eq!(server.app.state.workspaces.len(), 3);
}
