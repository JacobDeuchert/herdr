//! fork: FORK.md 0005, ticket picker (a new thread for a ticket).
use super::fork_threads::{sample_threads, thread, threads_config};
use super::*;
use crate::client::endpoint::{ClientEndpointStatus, ProfileId, SavedSshEndpoint};
use crate::client::shell::fork_threads::{self, OpenPlan, OpenTarget};
use crate::client::shell::fork_threads_feed::RowStatus;
use crate::client::shell::fork_tickets::{
    self, filter_tickets, parse_tickets, picker_lines, plan_ticket_open, ticket_command,
    valid_ticket_label, PickerLine, TicketList, TicketRow,
};
use std::sync::Arc;

const NEW_COMMAND: &str = r#"cd "$(tk dir {ticket})" && exec t3t"#;

/// The live shape of tkd's `/api/tickets` (trimmed), plus closed and
/// malformed items.
const TICKETS_JSON: &str = r#"[
  {"label":"t6","id":6,"title":"Accept IANA timezones","kind":"c","status":"in-progress",
   "brief":"…","branch":"jd/c-3745","dir":"/home/dev/work/t6-iana","extRef":"redmine:3745",
   "createdAt":"2026-11-09T11:28:04.115Z","updatedAt":"2026-11-13T11:58:28.924Z","activeSessions":10},
  {"label":"t11","id":11,"title":"Historian layering","kind":"c","status":"in-review",
   "dir":"/home/dev/work/t11","extRef":"redmine:3763"},
  {"label":"t13","id":13,"title":"Configurator \u001b[31mmigrate\u202e","kind":"c","status":"todo",
   "branch":null,"dir":null,"extRef":null},
  {"label":"t14","id":14,"title":"Draft idea","status":"draft","dir":"relative/dir"},
  {"label":"t9","id":9,"title":"Blocked thing","status":"blocked","dir":null},
  {"label":"t1","id":1,"title":"Old","status":"done","dir":null},
  {"label":"t2","id":2,"title":"Dropped","status":"dropped","dir":null},
  {"label":"t3; rm -rf ~","id":3,"title":"Evil","status":"todo"},
  {"label":"T4","id":4,"title":"Upper","status":"todo"},
  {"label":"t6","id":66,"title":"Duplicate","status":"todo"},
  42
]"#;

fn tickets() -> Vec<TicketRow> {
    parse_tickets(TICKETS_JSON.as_bytes()).unwrap()
}

fn labels(tickets: &[TicketRow], indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| tickets[*index].label.clone())
        .collect()
}

fn set_tickets() {
    fork_tickets::set_tickets_for_test(TicketList {
        tickets: Arc::new(tickets()),
        ..TicketList::default()
    });
}

fn picker_config(machine: &str) -> ClientShellConfig {
    let config: Config = toml::from_str(&format!(
        "[ui.threads]\nurl = \"http://127.0.0.1:9\"\nmachine = \"{machine}\"\nnew_key = \"prefix+t\"\n"
    ))
    .unwrap();
    ClientShellConfig::from_config(&config)
}

fn t3t_agent(pane: &str, workspace: &str, resume: &str, focused: bool) -> ClientShellAgent {
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
        tokens: vec![(crate::app::FORK_RESUME_TOKEN.to_owned(), resume.to_owned())],
        focused,
    }
}

/// ws_1 plain; ws_2 owns thread th6 of t6 and has two t3t panes (pane_b
/// focused); ws_3 owns thread th11 of t11 but has no t3t pane.
fn ticket_snapshot() -> ClientShellSnapshot {
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
                    "t3t -r th6".to_owned(),
                )],
                3 => vec![(
                    crate::app::FORK_SESSIONS_TOKEN.to_owned(),
                    "t3t -r th11".to_owned(),
                )],
                _ => Vec::new(),
            };
            workspace
        })
        .collect();
    projected.agents = vec![
        t3t_agent("pane_a", "ws_2", "t3t -r th6", false),
        t3t_agent("pane_b", "ws_2", "t3t -r other", true),
        t3t_agent("pane_c", "ws_3", "claude --resume x", false),
    ];
    projected
}

fn ticket_threads() -> Vec<crate::client::shell::fork_threads_feed::ThreadRow> {
    let mut rows = sample_threads();
    rows.push(thread("th6", Some("t6"), RowStatus::Ready));
    rows.push(thread("th11", Some("t11"), RowStatus::Ready));
    rows
}

fn template() -> fork_threads::CommandTemplate {
    fork_threads::CommandTemplate::parse("t3t -r {thread}").unwrap()
}

fn run_command(outcome: &ClientShellInput) -> crate::api::schema::PaneRunCommandParams {
    outcome
        .actions
        .iter()
        .find_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => match &request.method {
                crate::api::schema::Method::PaneRunCommand(params) => Some(params.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected pane.run_command, got {:?}", outcome.actions))
}

fn press(state: &mut ClientShellState, code: KeyCode, modifiers: KeyModifiers) -> ClientShellInput {
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Key(
        crate::input::TerminalKey::new(code, modifiers),
    )])
}

fn type_text(state: &mut ClientShellState, text: &str) {
    for character in text.chars() {
        press(state, KeyCode::Char(character), KeyModifiers::NONE);
    }
}

fn open_picker(state: &mut ClientShellState) {
    state.handle_input_bytes(&[0x02]);
    let outcome = state.handle_input_bytes(b"t");
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
    assert!(fork_tickets::picker_open());
}

fn local_state() -> ClientShellState {
    fork_threads::set_view_for_test(ticket_threads(), false);
    set_tickets();
    let mut state = ClientShellState::new(picker_config(""));
    state.config.palette = Palette::terminal();
    state.set_snapshot(Box::new(ticket_snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 30).unwrap();
    state
}

#[test]
fn tickets_decode_the_service_shape_and_drop_closed_and_unsafe_items() {
    let tickets = tickets();
    let all = (0..tickets.len()).collect::<Vec<_>>();
    // Started first (in progress/review by number desc, then other
    // statuses), then parked (todo, then draft).
    assert_eq!(labels(&tickets, &all), ["t11", "t6", "t9", "t13", "t14"]);
    let t6 = &tickets[1];
    assert_eq!(t6.title, "Accept IANA timezones");
    assert_eq!(t6.status, "in-progress");
    assert_eq!(t6.ext_ref.as_deref(), Some("redmine:3745"));
    assert_eq!(t6.dir.as_deref(), Some("/home/dev/work/t6-iana"));
    let t13 = &tickets[3];
    assert_eq!(
        t13.title, "Configurator [31mmigrate",
        "control/bidi stripped"
    );
    assert_eq!((t13.ext_ref.as_deref(), t13.dir.as_deref()), (None, None));
    assert_eq!(tickets[4].dir, None, "relative dirs are dropped");
    assert!(parse_tickets(b"{}").is_err());
}

#[test]
fn tickets_filter_on_label_title_and_ref() {
    let tickets = tickets();
    assert_eq!(labels(&tickets, &filter_tickets(&tickets, "")).len(), 5);
    assert_eq!(labels(&tickets, &filter_tickets(&tickets, "T13")), ["t13"]);
    assert_eq!(
        labels(&tickets, &filter_tickets(&tickets, "redmine")),
        ["t11", "t6"]
    );
    assert_eq!(labels(&tickets, &filter_tickets(&tickets, "3745")), ["t6"]);
    assert_eq!(
        labels(&tickets, &filter_tickets(&tickets, "iana time")),
        ["t6"]
    );
    assert!(filter_tickets(&tickets, "nothing").is_empty());
}

#[test]
fn tickets_group_parked_ones_below_a_header() {
    let tickets = tickets();
    let all = filter_tickets(&tickets, "");
    assert_eq!(
        picker_lines(&tickets, &all),
        [
            PickerLine::Ticket(0),
            PickerLine::Ticket(1),
            PickerLine::Ticket(2),
            PickerLine::ParkedHeader,
            PickerLine::Ticket(3),
            PickerLine::Ticket(4),
        ]
    );
    // No parked match: no header. Only parked matches: header first.
    let started = filter_tickets(&tickets, "redmine");
    assert!(!picker_lines(&tickets, &started).contains(&PickerLine::ParkedHeader));
    let parked = filter_tickets(&tickets, "draft");
    assert_eq!(
        picker_lines(&tickets, &parked),
        [PickerLine::ParkedHeader, PickerLine::Ticket(0)]
    );

    // In the overlay the cursor starts on the first started ticket and
    // steps over the header.
    let mut state = local_state();
    open_picker(&mut state);
    let rows = frame_rows(&state.compose(100, 30).unwrap());
    let line = |text: &str| {
        rows.iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("{text:?} not in {rows:#?}"))
    };
    assert!(line("t9 ") < line(" parked"));
    assert!(line(" parked") < line("t13"));
    assert!(line("t13") < line("t14"));
    for _ in 0..3 {
        press(&mut state, KeyCode::Down, KeyModifiers::NONE);
    }
    let view = state.fork_tickets_view().unwrap();
    assert_eq!(view.list.tickets[view.filtered[view.selected]].label, "t13");
}

#[test]
fn tickets_labels_are_validated_before_substitution() {
    assert!(valid_ticket_label("t13"));
    for bad in [
        "",
        "t",
        "T13",
        "t1a",
        "t-1",
        "t13 ",
        "13",
        "t1234567890",
        "t$(x)",
    ] {
        assert!(!valid_ticket_label(bad), "{bad:?}");
    }
    assert_eq!(
        ticket_command(NEW_COMMAND, "t13").as_deref(),
        Some(r#"cd "$(tk dir t13)" && exec t3t"#)
    );
    assert_eq!(ticket_command(NEW_COMMAND, "t13;reboot"), None);
    assert_eq!(ticket_command("", "t13"), None);
}

#[test]
fn tickets_plan_replaces_adds_or_creates() {
    let snapshot = ticket_snapshot();
    let rows = ticket_threads();
    let tickets = tickets();
    let ticket = |label: &str| tickets.iter().find(|t| t.label == label).unwrap();
    // t6's workspace has two t3t panes: the focused one is replaced.
    assert_eq!(
        plan_ticket_open(&snapshot, &template(), &rows, ticket("t6")),
        OpenPlan::Replace("pane_b".into())
    );
    // t11's workspace has no t3t pane: add one.
    assert_eq!(
        plan_ticket_open(&snapshot, &template(), &rows, ticket("t11")),
        OpenPlan::AddTo("ws_3".into())
    );
    // t13 has no workspace and no dir; t9 none either.
    assert_eq!(
        plan_ticket_open(&snapshot, &template(), &rows, ticket("t13")),
        OpenPlan::Create {
            cwd: None,
            label: "t13".into()
        }
    );
    // With a dir and no threads: the dir is the cwd.
    let mut parked = ticket("t6").clone();
    parked.label = "t77".into();
    assert_eq!(
        plan_ticket_open(&snapshot, &template(), &rows, &parked),
        OpenPlan::Create {
            cwd: Some("/home/dev/work/t6-iana".into()),
            label: "t77".into()
        }
    );
    // A workspace labelled with the ticket (opened before its t3t reported
    // a thread) is the ticket's workspace too.
    let mut labelled = snapshot.clone();
    labelled.workspaces[0].label = "t13".into();
    assert_eq!(
        plan_ticket_open(&labelled, &template(), &rows, ticket("t13")),
        OpenPlan::AddTo("ws_1".into())
    );
}

#[test]
fn tickets_picker_filters_and_enter_runs_the_new_command() {
    let mut state = local_state();
    open_picker(&mut state);
    let rows = frame_rows(&state.compose(100, 30).unwrap());
    assert!(rows.iter().any(|row| row.contains("new thread for ticket")));
    assert!(rows.iter().any(|row| row.contains("redmine:3745")));
    // The selected ticket (t11) has a workspace without a t3t pane.
    assert!(
        rows.iter()
            .any(|row| row.contains("new thread pane in space 3")),
        "{rows:#?}"
    );
    type_text(&mut state, "iana");
    let view = state.fork_tickets_view().unwrap();
    assert_eq!(view.filtered.len(), 1);
    let outcome = press(&mut state, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!fork_tickets::picker_open());
    let params = run_command(&outcome);
    assert_eq!(params.command, r#"cd "$(tk dir t6)" && exec t3t"#);
    assert_eq!(params.replace_pane_id.as_deref(), Some("pane_b"));

    // ctrl+n moves, Enter on a parked ticket creates its workspace.
    let mut state = local_state();
    open_picker(&mut state);
    for _ in 0..3 {
        press(&mut state, KeyCode::Char('n'), KeyModifiers::CONTROL);
    }
    press(&mut state, KeyCode::Char('p'), KeyModifiers::CONTROL);
    press(&mut state, KeyCode::Down, KeyModifiers::NONE);
    let params = run_command(&press(&mut state, KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(params.command, r#"cd "$(tk dir t13)" && exec t3t"#);
    assert_eq!(params.label.as_deref(), Some("t13"));
    assert_eq!(params.cwd, None);

    // Esc closes without doing anything; keys reach the pane again.
    let mut state = local_state();
    open_picker(&mut state);
    let outcome = press(&mut state, KeyCode::Esc, KeyModifiers::NONE);
    assert!(outcome.actions.is_empty());
    assert!(!fork_tickets::picker_open());
}

#[test]
fn tickets_picker_shows_loading_and_offline() {
    let mut state = local_state();
    fork_tickets::set_tickets_for_test(TicketList {
        loading: true,
        ..TicketList::default()
    });
    open_picker(&mut state);
    let rows = frame_rows(&state.compose(100, 30).unwrap());
    assert!(rows.iter().any(|row| row.contains("loading tickets")));
    fork_tickets::set_tickets_for_test(TicketList {
        offline: true,
        error: Some("connection refused".into()),
        ..TicketList::default()
    });
    let rows = frame_rows(&state.compose(100, 30).unwrap());
    assert!(rows
        .iter()
        .any(|row| row.contains("tickets unavailable: connection refused")));
    // Enter with nothing to pick keeps the picker open.
    let outcome = press(&mut state, KeyCode::Enter, KeyModifiers::NONE);
    assert!(outcome.actions.is_empty());
    assert!(fork_tickets::picker_open());
}

#[test]
fn tickets_key_is_off_unless_configured() {
    fork_threads::set_view_for_test(ticket_threads(), false);
    set_tickets();
    let mut state = ClientShellState::new(threads_config(""));
    state.set_snapshot(Box::new(ticket_snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 30).unwrap();
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"t");
    assert!(!fork_tickets::picker_open());
}

#[test]
fn tickets_open_on_another_machine_waits_for_it() {
    fork_threads::set_view_for_test(ticket_threads(), false);
    set_tickets();
    let mut state = ClientShellState::new(picker_config("devvm"));
    let profile = SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "devvm".into(),
        target: "dev@devvm".into(),
        session: "default".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut vm = ticket_snapshot();
    vm.boot_id = "vm-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(vm));
    state.compose(100, 30).unwrap();

    open_picker(&mut state);
    type_text(&mut state, "t6");
    let outcome = press(&mut state, KeyCode::Enter, KeyModifiers::NONE);
    // The local machine is shown: switch to the VM's t3t pane first.
    assert!(
        matches!(outcome.actions.as_slice(), [ClientShellAction::ActivateEndpoint {
            endpoint_id,
            target: Some(ClientEndpointFocusTarget::Pane(pane)),
        }] if *endpoint_id == remote && pane == "pane_b"),
        "{:?}",
        outcome.actions
    );
    assert_eq!(
        fork_threads::pending_open_for_test(),
        Some((remote.clone(), OpenTarget::NewForTicket("t6".into())))
    );

    // Not ready yet: the tick does nothing.
    let mut outcome = ClientShellInput::default();
    state.fork_threads_tick(&mut outcome);
    assert!(outcome.actions.is_empty());
    assert!(fork_threads::pending_open_for_test().is_some());

    // The VM is shown and its surface is up: the tick finishes the open.
    assert!(state.activate_endpoint_projection(&remote));
    let mut vm_surface = surface();
    vm_surface.boot_id = "vm-boot".into();
    state.set_pane_surface(vm_surface);
    set_tickets();
    let mut outcome = ClientShellInput::default();
    state.fork_threads_tick(&mut outcome);
    let params = run_command(&outcome);
    assert_eq!(params.command, r#"cd "$(tk dir t6)" && exec t3t"#);
    assert_eq!(params.replace_pane_id.as_deref(), Some("pane_b"));
    assert_eq!(fork_threads::pending_open_for_test(), None);
}
