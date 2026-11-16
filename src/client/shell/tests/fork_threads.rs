//! fork: FORK.md 0005, sidebar thread list.
use super::*;
use crate::client::shell::fork_threads;
use crate::client::shell::fork_threads_feed::{ThreadRow, ThreadStatus, ThreadTicket};

pub(super) fn threads_config(machine: &str) -> ClientShellConfig {
    // Port 9 (discard) on loopback: the reader fails fast and stays out of the way.
    let config: Config = toml::from_str(&format!(
        "[ui.threads]\nurl = \"http://127.0.0.1:9\"\nmachine = \"{machine}\"\ncommand = \"t3t -r {{thread}}\"\n"
    ))
    .unwrap();
    ClientShellConfig::from_config(&config)
}

pub(super) fn thread(id: &str, ticket: Option<&str>, status: ThreadStatus) -> ThreadRow {
    ThreadRow {
        id: id.into(),
        ticket: ticket.map(|label| ThreadTicket {
            id: format!("id-{label}"),
            label: label.into(),
            title: format!("{label} title"),
        }),
        status,
        agent: "codex".into(),
        model: "gpt-5".into(),
        title: format!("thread {id}"),
        cwd: format!("/work/{id}"),
    }
}

pub(super) fn sample_threads() -> Vec<ThreadRow> {
    vec![
        thread("t1", None, ThreadStatus::Running),
        thread("t2", Some("PROJ-1"), ThreadStatus::Blocked),
        thread("t3", Some("PROJ-2"), ThreadStatus::Idle),
        thread("t4", Some("PROJ-1"), ThreadStatus::Done),
    ]
}

fn render(config: ClientShellConfig) -> (ClientShellState, Vec<String>) {
    let mut state = ClientShellState::new(config);
    state.config.palette = Palette::terminal();
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let frame = state.compose(100, 30).unwrap();
    let rows = frame_rows(&frame);
    (state, rows)
}

#[test]
fn threads_lines_group_by_ticket_with_no_ticket_last() {
    let rows = sample_threads();
    use fork_threads::ThreadLine::{Group, Thread};
    assert_eq!(
        fork_threads::thread_lines(&rows),
        [
            Group(Some(1)),
            Thread(1),
            Thread(3),
            Group(Some(2)),
            Thread(2),
            Group(None),
            Thread(0)
        ]
    );
    assert_eq!(fork_threads::display_order(&rows), [1, 3, 2, 0]);
}

#[test]
fn threads_block_draws_groups_status_and_details() {
    fork_threads::set_view_for_test(sample_threads(), false);
    let (state, rows) = render(threads_config(""));
    assert!(rows[0].contains("threads"), "{rows:#?}");
    assert!(rows[1].contains("PROJ-1") && rows[1].contains("PROJ-1 title"));
    assert!(rows[2].contains("thread t2 [blocked]"), "{:?}", rows[2]);
    assert!(!rows[2].contains("codex"), "no detail column when narrow");
    assert!(rows[3].contains("thread t4"));
    assert!(rows[4].contains("PROJ-2"));
    assert!(rows[6].contains("no ticket"));
    assert!(rows[7].contains("thread t1"));
    let hit_ids = state
        .hits
        .fork_threads
        .iter()
        .map(|hit| hit.thread_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(hit_ids, ["t2", "t4", "t3", "t1"]);
    // The spaces section still follows.
    assert!(rows.iter().any(|row| row.contains("spaces")));
}

#[test]
fn threads_block_shows_agent_and_model_when_wide() {
    fork_threads::set_view_for_test(sample_threads(), false);
    let mut config = threads_config("");
    config.sidebar_width = 44;
    let (_, rows) = render(config);
    assert!(
        rows[2].contains("thread t2 [blocked] codex·gpt"),
        "{:?}",
        rows[2]
    );
}

#[test]
fn threads_block_marks_stale_rows_offline() {
    fork_threads::set_view_for_test(sample_threads(), true);
    let (_, rows) = render(threads_config(""));
    assert!(rows[0].contains("offline"), "{:?}", rows[0]);
    assert!(rows[2].contains("thread t2"), "stale rows stay visible");
}

#[test]
fn threads_block_truncates_long_lists() {
    let rows = (0..40)
        .map(|index| thread(&format!("t{index}"), None, ThreadStatus::Idle))
        .collect();
    fork_threads::set_view_for_test(rows, false);
    let (_, rows) = render(threads_config(""));
    assert!(rows.iter().any(|row| row.contains("more")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("spaces")));
}

#[test]
fn threads_disabled_leaves_the_sidebar_alone() {
    let (state, rows) = render(ClientShellConfig::from_config(&Config::default()));
    assert!(!rows[0].contains("threads"));
    assert!(state.hits.fork_threads.is_empty());
}

fn template() -> fork_threads::CommandTemplate {
    fork_threads::CommandTemplate::parse("t3t -r {thread}").unwrap()
}

fn agent(pane: &str, workspace: &str, resume: Option<&str>, focused: bool) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane.into(),
        workspace_id: workspace.into(),
        tab_id: format!("tab_{workspace}"),
        name: None,
        display_agent: None,
        agent: Some("t3code".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: resume
            .map(|line| vec![(crate::app::FORK_RESUME_TOKEN.to_owned(), line.to_owned())])
            .unwrap_or_default(),
        focused,
    }
}

/// ws_1 is a plain workspace; ws_2 owns t2 and t4 (pane_2 shows t4); ws_3
/// owns only threads no longer listed.
fn mapped_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let base = projected.workspaces[0].clone();
    projected.workspaces = (1..=3)
        .map(|number| {
            let mut workspace = base.clone();
            workspace.workspace_id = format!("ws_{number}");
            workspace.number = number;
            workspace.label = format!("space {number}");
            workspace.focused = number == 1;
            workspace.tokens = match number {
                2 => vec![(
                    crate::app::FORK_SESSIONS_TOKEN.to_owned(),
                    "t3t -r t2\nclaude --resume x\nt3t -r t4".to_owned(),
                )],
                3 => vec![(
                    crate::app::FORK_SESSIONS_TOKEN.to_owned(),
                    "t3t -r gone".to_owned(),
                )],
                _ => Vec::new(),
            };
            workspace
        })
        .collect();
    projected.agents = vec![agent("pane_2", "ws_2", Some("t3t -r t4"), false)];
    projected
}

fn mapped_state() -> ClientShellState {
    fork_threads::set_view_for_test(sample_threads(), false);
    let mut state = ClientShellState::new(threads_config(""));
    state.config.palette = Palette::terminal();
    state.set_snapshot(Box::new(mapped_snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 30).unwrap();
    state
}

fn run_command(outcome: &ClientShellInput) -> crate::api::schema::PaneRunCommandParams {
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!("expected one endpoint request, got {:?}", outcome.actions);
    };
    let crate::api::schema::Method::PaneRunCommand(params) = &request.method else {
        panic!("expected pane.run_command, got {:?}", request.method);
    };
    params.clone()
}

fn click_thread(state: &mut ClientShellState, thread_id: &str) -> ClientShellInput {
    let rect = state
        .hits
        .fork_threads
        .iter()
        .find(|hit| hit.thread_id == thread_id)
        .expect("thread row hit")
        .rect;
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 3,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn threads_template_matches_resume_commands() {
    let template = template();
    assert_eq!(
        template.command_for("abc-1").as_deref(),
        Some("t3t -r abc-1")
    );
    assert_eq!(template.command_for("a;b"), None);
    assert_eq!(template.thread_in("t3t -r abc").as_deref(), Some("abc"));
    assert_eq!(
        template
            .thread_in("/nix/store/x/bin/t3t --server ws://h -r abc")
            .as_deref(),
        Some("abc")
    );
    assert_eq!(template.thread_in("t3t"), None);
    assert_eq!(template.thread_in("claude -r abc"), None);
    assert!(template.runs_program("t3t -r other"));
    let positional = fork_threads::CommandTemplate::parse("tool open thread:{thread}").unwrap();
    assert_eq!(
        positional.thread_in("tool open thread:x1").as_deref(),
        Some("x1")
    );
    assert_eq!(positional.thread_in("tool open x1"), None);
}

#[test]
fn threads_mapped_workspaces_hide_until_their_threads_leave_the_list() {
    let state = mapped_state();
    let listed = state
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.workspace_id.as_str())
        .collect::<Vec<_>>();
    // ws_3's only thread is no longer listed, so it is a normal workspace again.
    assert_eq!(listed, ["ws_1", "ws_3"]);

    fork_threads::set_view_for_test(vec![thread("t1", None, ThreadStatus::Idle)], false);
    let mut state = state;
    state.compose(100, 30).unwrap();
    assert_eq!(state.hits.workspaces.len(), 3);
}

#[test]
fn threads_plan_covers_focus_restart_add_and_create() {
    let snapshot = mapped_snapshot();
    let rows = sample_threads();
    let template = template();
    use fork_threads::{plan_open, OpenPlan};
    // t4 is on screen in pane_2.
    assert_eq!(
        plan_open(&snapshot, &template, &rows, "t4"),
        Some(OpenPlan::FocusPane("pane_2".into()))
    );
    // t2 belongs to ws_2, whose thread pane shows t4: restart it in place.
    assert_eq!(
        plan_open(&snapshot, &template, &rows, "t2"),
        Some(OpenPlan::Replace("pane_2".into()))
    );
    // t1 has no ticket and no workspace.
    assert_eq!(
        plan_open(&snapshot, &template, &rows, "t1"),
        Some(OpenPlan::Create {
            cwd: Some("/work/t1".into()),
            label: "thread t1".into()
        })
    );
    // t3 (PROJ-2) has no workspace and no ticket sibling.
    assert!(matches!(
        plan_open(&snapshot, &template, &rows, "t3"),
        Some(OpenPlan::Create { label, .. }) if label == "PROJ-2"
    ));
    // A new PROJ-1 thread joins its ticket's workspace.
    let mut rows = rows;
    rows.push(thread("t5", Some("PROJ-1"), ThreadStatus::Idle));
    assert_eq!(
        plan_open(&snapshot, &template, &rows, "t5"),
        Some(OpenPlan::Replace("pane_2".into()))
    );
    // Without a thread pane, a pane is added.
    let mut no_pane = snapshot.clone();
    no_pane.agents.clear();
    assert_eq!(
        plan_open(&no_pane, &template, &rows, "t2"),
        Some(OpenPlan::AddTo("ws_2".into()))
    );
    assert_eq!(plan_open(&snapshot, &template, &rows, "missing"), None);
}

#[test]
fn threads_click_runs_the_planned_action() {
    let mut state = mapped_state();
    let outcome = click_thread(&mut state, "t2");
    let params = run_command(&outcome);
    assert_eq!(params.command, "t3t -r t2");
    assert_eq!(params.replace_pane_id.as_deref(), Some("pane_2"));
    assert!(params.focus);

    let mut state = mapped_state();
    let outcome = click_thread(&mut state, "t1");
    let params = run_command(&outcome);
    assert_eq!(params.command, "t3t -r t1");
    assert_eq!(params.cwd.as_deref(), Some("/work/t1"));
    assert_eq!(params.label.as_deref(), Some("thread t1"));

    let mut state = mapped_state();
    let outcome = click_thread(&mut state, "t4");
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!("expected a focus request, got {:?}", outcome.actions);
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
    ));
}

#[test]
fn threads_highlight_the_thread_on_screen() {
    let mut projected = mapped_snapshot();
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_pane_id = Some("pane_2".into());
    fork_threads::set_view_for_test(sample_threads(), false);
    let mut state = ClientShellState::new(threads_config(""));
    state.config.palette = Palette::terminal();
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(100, 30).unwrap();
    let rows = frame_rows(&frame);
    let line = rows
        .iter()
        .position(|row| row.contains("thread t4"))
        .unwrap();
    let row_bg = frame.cells[line * frame.width as usize + 5].bg;
    let other = rows
        .iter()
        .position(|row| row.contains("thread t3"))
        .unwrap();
    assert_ne!(row_bg, frame.cells[other * frame.width as usize + 5].bg);
}

#[test]
fn threads_navigate_cycles_through_rows_and_enter_opens() {
    let mut state = mapped_state();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_1");
    // Up from the first workspace enters the thread rows at the bottom.
    let outcome = state.handle_input_bytes(b"\x1b[A");
    assert!(outcome.actions.is_empty());
    assert_eq!(fork_threads::view().cursor, Some(3));
    assert!(state.navigate_workspace_id.is_none());
    // Display order is t2, t4, t3, t1; move up to t3.
    state.handle_input_bytes(b"\x1b[A");
    assert_eq!(fork_threads::view().cursor, Some(2));
    let outcome = state.handle_input_bytes(b"\r");
    let params = run_command(&outcome);
    assert_eq!(params.command, "t3t -r t3");
    assert_eq!(params.label.as_deref(), Some("PROJ-2"));
    assert_eq!(fork_threads::view().cursor, None);
    assert_ne!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn threads_navigate_leaves_rows_back_into_workspaces() {
    let mut state = mapped_state();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_3");
    // Down from the last workspace wraps into the first thread row.
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(fork_threads::view().cursor, Some(0));
    // Up from the first row goes back to the last workspace.
    state.handle_input_bytes(b"\x1b[A");
    assert_eq!(fork_threads::view().cursor, None);
    assert_eq!(
        state
            .navigate_workspace_id
            .as_ref()
            .map(|target| target.workspace_id.as_str()),
        Some("ws_3")
    );
}

#[test]
fn threads_navigate_from_a_thread_workspace_starts_on_its_row() {
    let mut projected = mapped_snapshot();
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_pane_id = Some("pane_2".into());
    fork_threads::set_view_for_test(sample_threads(), false);
    let mut state = ClientShellState::new(threads_config(""));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(100, 30).unwrap();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_2");
    state.handle_input_bytes(b"\x1b[A");
    // t4 is on screen; display order is t2, t4, t3, t1.
    assert_eq!(fork_threads::view().cursor, Some(1));
}
