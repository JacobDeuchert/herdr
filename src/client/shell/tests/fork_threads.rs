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
