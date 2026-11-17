//! fork: FORK.md 0005, sidebar thread list.
use super::*;
use crate::client::shell::fork_threads;
use crate::client::shell::fork_threads_feed::{
    RowStatus, ThreadInbox, ThreadRow, ThreadStatus, ThreadTicket,
};
use crate::client::shell::fork_threads_inbox::{
    self as inbox, InboxLine, RowLabel, Section, Shelves,
};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

pub(super) fn threads_config(machine: &str) -> ClientShellConfig {
    // Port 9 (discard) on loopback: the reader fails fast and stays out of the way.
    let config: Config = toml::from_str(&format!(
        "[ui.threads]\nurl = \"http://127.0.0.1:9\"\nmachine = \"{machine}\"\ncommand = \"t3t -r {{thread}}\"\n\n[keys]\nnext_agent = \"ctrl+alt+j\"\nprevious_agent = \"ctrl+alt+k\"\nfocus_agent = \"ctrl+alt+1..9\"\n"
    ))
    .unwrap();
    ClientShellConfig::from_config(&config)
}

pub(super) fn thread(id: &str, ticket: Option<&str>, status: RowStatus) -> ThreadRow {
    ThreadRow {
        id: id.into(),
        ticket: ticket.map(|label| ThreadTicket {
            id: format!("id-{label}"),
            label: label.into(),
            title: format!("{label} title"),
        }),
        status: ThreadStatus::Idle,
        agent: "codex".into(),
        model: "gpt-5".into(),
        title: format!("thread {id}"),
        cwd: format!("/work/{id}"),
        branch: String::new(),
        inbox: ThreadInbox {
            listed: true,
            status,
            from_service: true,
            ..ThreadInbox::default()
        },
    }
}

fn with(mut row: ThreadRow, change: impl FnOnce(&mut ThreadInbox)) -> ThreadRow {
    change(&mut row.inbox);
    row
}

/// Four active threads, shown t2, t4, t3, t1.
pub(super) fn sample_threads() -> Vec<ThreadRow> {
    [
        ("t1", None, RowStatus::Ready, 1),
        ("t2", Some("PROJ-1"), RowStatus::Approval, 4),
        ("t3", Some("PROJ-2"), RowStatus::Ready, 2),
        ("t4", Some("PROJ-1"), RowStatus::Ready, 3),
    ]
    .into_iter()
    .map(|(id, ticket, status, at)| {
        with(thread(id, ticket, status), |inbox| {
            inbox.inbox_sort_at = Some(at)
        })
    })
    .collect()
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

fn line_of(rows: &[String], text: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("{text:?} not drawn: {rows:#?}"))
}

fn thread_hits(state: &ClientShellState) -> Vec<String> {
    state
        .hits
        .fork_threads
        .iter()
        .filter_map(|hit| match &hit.target {
            fork_threads::ThreadHitTarget::Thread(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

// --- the inbox rules -------------------------------------------------------

#[test]
fn threads_inbox_section_precedence() {
    let now = 1_000 * DAY;
    let row = |change: fn(&mut ThreadInbox)| with(thread("t", None, RowStatus::Ready), change);
    let snoozed_settled_pinned = row(|inbox| {
        inbox.snoozed_until = Some(1_000 * DAY + HOUR);
        inbox.settled = true;
        inbox.pinned = true;
        inbox.working = true;
    });
    assert_eq!(
        inbox::section(&snoozed_settled_pinned, now),
        Section::Snoozed
    );
    // The snooze ends with the clock: no new fetch needed.
    assert_eq!(
        inbox::section(&snoozed_settled_pinned, now + HOUR),
        Section::Settled
    );
    let raised = with(snoozed_settled_pinned.clone(), |inbox| {
        inbox.raised_hand = true
    });
    assert_eq!(inbox::section(&raised, now), Section::Settled);
    assert_eq!(
        inbox::section(
            &row(|inbox| {
                inbox.pinned = true;
                inbox.working = true;
            }),
            now
        ),
        Section::Pinned
    );
    assert_eq!(
        inbox::section(&row(|inbox| inbox.working = true), now),
        Section::Working
    );
    assert_eq!(inbox::section(&row(|_| {}), now), Section::Active);
}

#[test]
fn threads_inbox_sorts_each_section() {
    let now = 1_000 * DAY;
    let rows = vec![
        with(thread("p-b", None, RowStatus::Ready), |inbox| {
            inbox.pinned = true;
            inbox.created_at = Some(5);
        }),
        with(thread("p-a", None, RowStatus::Ready), |inbox| {
            inbox.pinned = true;
            inbox.created_at = Some(9);
        }),
        with(thread("p-key2", None, RowStatus::Ready), |inbox| {
            inbox.pinned = true;
            inbox.pin_order_key = Some("b".into());
        }),
        with(thread("p-key1", None, RowStatus::Ready), |inbox| {
            inbox.pinned = true;
            inbox.pin_order_key = Some("a".into());
        }),
        with(thread("a-old", None, RowStatus::Ready), |inbox| {
            inbox.inbox_sort_at = Some(1)
        }),
        with(thread("a-new-z", None, RowStatus::Ready), |inbox| {
            inbox.inbox_sort_at = Some(7)
        }),
        with(thread("a-new-y", None, RowStatus::Ready), |inbox| {
            inbox.inbox_sort_at = Some(7)
        }),
        with(thread("w-old", None, RowStatus::Working), |inbox| {
            inbox.working = true;
            inbox.working_sort_at = Some(1);
        }),
        with(thread("w-new", None, RowStatus::Working), |inbox| {
            inbox.working = true;
            inbox.working_sort_at = Some(2);
        }),
        with(thread("s-late", None, RowStatus::Ready), |inbox| {
            inbox.snoozed_until = Some(now + 2 * HOUR)
        }),
        with(thread("s-soon", None, RowStatus::Ready), |inbox| {
            inbox.snoozed_until = Some(now + HOUR)
        }),
        with(thread("x-b", None, RowStatus::Ready), |inbox| {
            inbox.settled = true;
            inbox.settled_sort_at = Some(3);
        }),
        with(thread("x-a", None, RowStatus::Ready), |inbox| {
            inbox.settled = true;
            inbox.settled_sort_at = Some(3);
        }),
        with(thread("x-none", None, RowStatus::Ready), |inbox| {
            inbox.settled = true
        }),
        with(thread("hidden", None, RowStatus::Ready), |inbox| {
            inbox.listed = false
        }),
    ];
    let shelves = Shelves {
        snoozed_open: true,
        settled_open: true,
        ..Shelves::default()
    };
    let names = inbox::inbox_lines(&rows, now, &shelves, None)
        .into_iter()
        .map(|line| match line {
            InboxLine::Row { index, .. } => rows[index].id.clone(),
            InboxLine::Shelf {
                section,
                count,
                open,
            } => {
                format!("[{} {count} {open}]", section.shelf_label())
            }
            InboxLine::ShowMore(count) => format!("[more {count}]"),
            InboxLine::Expand { hidden, open } => format!("[expand {hidden} {open}]"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "p-key1",
            "p-key2",
            "p-a",
            "p-b", // pinned: keyed first, then newest
            "a-new-y",
            "a-new-z",
            "a-old", // active: newest, id tie-break
            "[Snoozed 2 true]",
            "s-soon",
            "s-late", // soonest wake first
            "[Settled 3 true]",
            "x-a",
            "x-b",
            "x-none",
            "w-new", // Working: its own section after the inbox, no shelf
            "w-old",
        ]
    );
}

fn inbox_names(rows: &[ThreadRow], shelves: &Shelves, shown: Option<&str>) -> Vec<String> {
    inbox::inbox_lines(rows, 1_000 * DAY, shelves, shown)
        .into_iter()
        .map(|line| match line {
            InboxLine::Row { index, .. } => rows[index].id.clone(),
            InboxLine::Shelf { section, count, .. } => {
                format!("[{} {count}]", section.shelf_label())
            }
            InboxLine::ShowMore(count) => format!("[more {count}]"),
            InboxLine::Expand { hidden, open } => format!("[expand {hidden} {open}]"),
        })
        .collect()
}

#[test]
fn threads_inbox_caps_pinned_and_active_rows() {
    let mut rows = vec![with(thread("pin", None, RowStatus::Ready), |inbox| {
        inbox.pinned = true
    })];
    rows.extend((0..11).map(|index| {
        with(
            thread(&format!("a{index:02}"), None, RowStatus::Ready),
            |inbox| inbox.inbox_sort_at = Some(100 - index),
        )
    }));
    // Past the cap but needing Jacob: stays visible.
    rows[11].inbox.status = RowStatus::Approval;
    let names = inbox_names(&rows, &Shelves::default(), Some("a09"));
    assert_eq!(
        names,
        [
            "pin",
            "a00",
            "a01",
            "a02",
            "a03",
            "a04",
            "a05",
            "a06", // 8 with the pinned row
            "a09", // on screen
            "a10", // approval
            "[expand 2 false]",
            "[Settled 0]",
        ]
    );
    let open = Shelves {
        inbox_open: true,
        ..Shelves::default()
    };
    let names = inbox_names(&rows, &open, None);
    assert_eq!(names.len(), 12 + 2, "{names:?}");
    assert_eq!(names[12], "[expand 0 true]");
}

#[test]
fn threads_inbox_has_no_expand_line_within_the_cap() {
    let rows = (0..8)
        .map(|index| thread(&format!("a{index}"), None, RowStatus::Ready))
        .collect::<Vec<_>>();
    let names = inbox_names(&rows, &Shelves::default(), None);
    assert!(
        !names.iter().any(|name| name.starts_with("[expand")),
        "{names:?}"
    );
}

#[test]
fn threads_inbox_shelves_collapse_cap_and_keep_the_shown_thread() {
    let now = 1_000 * DAY;
    let mut rows = vec![
        thread("a", None, RowStatus::Ready),
        with(thread("w", None, RowStatus::Working), |inbox| {
            inbox.working = true
        }),
    ];
    rows.extend((0..40).map(|index| {
        with(
            thread(&format!("x{index:02}"), None, RowStatus::Ready),
            |inbox| {
                inbox.settled = true;
                inbox.settled_sort_at = Some(100 - index);
            },
        )
    }));
    let closed = Shelves::default();
    let lines = inbox::inbox_lines(&rows, now, &closed, None);
    assert_eq!(
        lines[1..],
        [
            InboxLine::Shelf {
                section: Section::Settled,
                count: 40,
                open: false
            },
            InboxLine::Row {
                index: 1,
                section: Section::Working
            },
        ],
        "no Snoozed shelf without snoozed threads; Working after the inbox"
    );
    // The rest is about the Settled shelf, which then ends the list.
    rows.remove(1);
    // The thread on screen stays visible in its closed shelf.
    let lines = inbox::inbox_lines(&rows, now, &closed, Some("x30"));
    assert!(
        matches!(lines.last(), Some(InboxLine::Row { index, section: Section::Settled }) if rows[*index].id == "x30")
    );

    let mut open = Shelves {
        settled_open: true,
        ..Shelves::default()
    };
    let lines = inbox::inbox_lines(&rows, now, &open, None);
    assert_eq!(inbox::displayed_rows(&lines).len(), 1 + 10);
    assert_eq!(lines.last(), Some(&InboxLine::ShowMore(25)));
    // Beyond the cap the shown thread is added after the first ten.
    let lines = inbox::inbox_lines(&rows, now, &open, Some("x30"));
    assert_eq!(inbox::displayed_rows(&lines).len(), 1 + 11);
    assert_eq!(lines.last(), Some(&InboxLine::ShowMore(25)));
    open.show_more();
    let lines = inbox::inbox_lines(&rows, now, &open, None);
    assert_eq!(lines.last(), Some(&InboxLine::ShowMore(5)));
    open.show_more();
    let lines = inbox::inbox_lines(&rows, now, &open, None);
    assert!(matches!(lines.last(), Some(InboxLine::Row { .. })));
    assert_eq!(inbox::displayed_rows(&lines).len(), 1 + 40);

    assert!(inbox::inbox_lines(&[], now, &closed, None).is_empty());
    let unlisted = vec![with(thread("u", None, RowStatus::Ready), |inbox| {
        inbox.listed = false
    })];
    assert!(inbox::inbox_lines(&unlisted, now, &closed, None).is_empty());
}

#[test]
fn threads_inbox_row_labels_follow_the_status_rules() {
    let now = 1_000 * DAY;
    let label = |row: ThreadRow| inbox::row_label(&row, Section::Active, now).text();
    let card = |status| thread("t", None, status);
    assert_eq!(label(card(RowStatus::Approval)), "Approval");
    assert_eq!(label(card(RowStatus::Input)), "Input");
    assert_eq!(label(card(RowStatus::Request)), "Request");
    assert_eq!(label(card(RowStatus::Limited)), "Limited");
    assert_eq!(label(card(RowStatus::Failed)), "Failed");
    assert_eq!(
        label(with(card(RowStatus::Working), |inbox| inbox
            .working_started_at =
            Some(now - 42_000))),
        "Working 42s"
    );
    assert_eq!(
        label(with(card(RowStatus::Working), |inbox| {
            inbox.goal_active = true;
            inbox.working_started_at = Some(now - 2 * HOUR - 5 * MINUTE);
        })),
        "Goal 2h 5m"
    );
    assert_eq!(
        label(with(card(RowStatus::Preparing), |inbox| inbox
            .working_started_at =
            Some(now - 3_000))),
        "Setting up 3s"
    );
    assert_eq!(
        label(with(card(RowStatus::Waiting), |inbox| inbox
            .waiting_since =
            Some(now - 7 * MINUTE))),
        "Waiting 7m"
    );
    assert_eq!(label(card(RowStatus::Working)), "Working");
    // Ready: Woke, then Done, then the relative time.
    let snoozed_past = |row: ThreadRow| with(row, |inbox| inbox.snoozed_until = Some(now - MINUTE));
    assert_eq!(label(snoozed_past(card(RowStatus::Ready))), "Woke");
    assert_eq!(
        label(with(snoozed_past(card(RowStatus::Ready)), |inbox| {
            inbox.last_visited_at = Some(now)
        })),
        "",
        "visited after the wake"
    );
    assert_eq!(
        label(with(card(RowStatus::Ready), |inbox| {
            inbox.unread = true;
            inbox.activity_at = Some(now - HOUR);
        })),
        "Done"
    );
    assert_eq!(
        label(with(card(RowStatus::Ready), |inbox| inbox.activity_at =
            Some(now - 3 * HOUR))),
        "3h"
    );
    assert_eq!(
        label(with(card(RowStatus::Ready), |inbox| inbox.activity_at =
            Some(now - 10_000))),
        "just now"
    );
    // A raised hand wakes a future snooze too.
    assert_eq!(
        label(with(card(RowStatus::Ready), |inbox| {
            inbox.snoozed_until = Some(now + HOUR);
            inbox.raised_hand_woke_at = Some(now - MINUTE);
        })),
        "Woke"
    );

    // Slim rows: no status, a countdown, Woke, or a time.
    let slim = |row: ThreadRow, section| inbox::row_label(&row, section, now);
    let snoozed = with(card(RowStatus::Approval), |inbox| {
        inbox.snoozed_until = Some(now + 90 * MINUTE + 1)
    });
    assert_eq!(
        slim(snoozed.clone(), Section::Snoozed),
        RowLabel::Snoozed("2h".into())
    );
    assert_eq!(
        slim(
            with(snoozed, |inbox| inbox.snoozed_until = Some(now + 61_000)),
            Section::Snoozed
        )
        .text(),
        "◷ 2m"
    );
    let settled = with(card(RowStatus::Failed), |inbox| {
        inbox.settled = true;
        inbox.settled_sort_at = Some(now - 2 * DAY);
        inbox.activity_at = Some(now - MINUTE);
        inbox.snoozed_until = Some(now - MINUTE);
    });
    assert_eq!(
        slim(settled, Section::Settled).text(),
        "2d",
        "settled rows are never Woke"
    );
}

#[test]
fn threads_inbox_recede_and_clock() {
    let now = 1_000 * DAY;
    assert!(inbox::receded(&thread("t", None, RowStatus::Working), now));
    assert!(inbox::receded(&thread("t", None, RowStatus::Ready), now));
    assert!(!inbox::receded(&thread("t", None, RowStatus::Input), now));
    assert!(!inbox::receded(&thread("t", None, RowStatus::Failed), now));
    assert!(!inbox::receded(
        &with(thread("t", None, RowStatus::Approval), |inbox| inbox
            .unread =
            true),
        now
    ));
    // The service's flag wins, minus Woke for ready rows.
    let shipped = |status, receded| {
        with(thread("t", None, status), |inbox| {
            inbox.receded = Some(receded);
            inbox.snoozed_until = Some(now - MINUTE);
        })
    };
    assert!(!inbox::receded(&shipped(RowStatus::Working, false), now));
    assert!(
        !inbox::receded(&shipped(RowStatus::Ready, true), now),
        "woke"
    );
    assert!(inbox::receded(&shipped(RowStatus::Working, true), now));

    // Nothing clock-driven: no deadline.
    assert_eq!(
        inbox::next_clock_change(&[thread("t", None, RowStatus::Ready)], now),
        None
    );
    let rows = vec![
        with(thread("a", None, RowStatus::Ready), |inbox| {
            inbox.activity_at = Some(now - 5 * MINUTE - 1_000)
        }),
        with(thread("s", None, RowStatus::Ready), |inbox| {
            inbox.snoozed_until = Some(now + 30 * 1_000)
        }),
    ];
    assert_eq!(inbox::next_clock_change(&rows, now), Some(now + 30 * 1_000));
    let rows = vec![with(thread("w", None, RowStatus::Working), |inbox| {
        inbox.working_started_at = Some(now - 1_500)
    })];
    assert_eq!(
        inbox::next_clock_change(&rows, now),
        Some(now + 500),
        "seconds tick"
    );
    let rows = vec![with(thread("a", None, RowStatus::Ready), |inbox| {
        inbox.activity_at = Some(now - 5 * MINUTE - 1_000)
    })];
    assert_eq!(
        inbox::next_clock_change(&rows, now),
        Some(now + MINUTE - 1_000)
    );
}

#[test]
fn threads_fallback_rows_form_one_active_list_by_update_time() {
    let body = r#"[
        {"id":"old","status":"idle","updated_at":"2026-11-16T09:00:00Z"},
        {"id":"new","status":"blocked","updated_at":"2026-11-16T10:00:00Z"},
        {"id":"run","status":"running","updated_at":"2026-11-16T10:30:00Z"}
    ]"#;
    let rows = crate::client::shell::fork_threads_feed::parse_sessions(body.as_bytes()).unwrap();
    let lines = inbox::inbox_lines(&rows, inbox::now_millis(), &Shelves::default(), None);
    let order = inbox::displayed_rows(&lines)
        .into_iter()
        .map(|index| rows[index].id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(order, ["new", "old", "run"]);
    assert!(!lines.contains(&InboxLine::Shelf {
        section: Section::Working,
        count: 1,
        open: false
    }));
}

// --- the panel in the sidebar --------------------------------------------

#[test]
fn threads_panel_replaces_the_agents_section_below_the_spaces() {
    fork_threads::set_view_for_test(sample_threads(), false);
    let (state, rows) = render(threads_config(""));
    let spaces = line_of(&rows, "spaces");
    let header = line_of(&rows, "threads");
    assert!(header > spaces, "below the workspace list: {rows:#?}");
    assert!(!rows.iter().any(|row| row.contains(" agents")), "{rows:#?}");
    let t2 = line_of(&rows, "thread t2");
    assert!(rows[t2].contains("Approval"), "{:?}", rows[t2]);
    assert!(
        rows[t2 + 1].contains("PROJ-1"),
        "meta line: {:?}",
        rows[t2 + 1]
    );
    assert_eq!(thread_hits(&state), ["t2", "t4", "t3", "t1"]);
    // The thread panel uses the agents section's body for scrolling.
    assert!(state.hits.agent_body.y > header as u16);
    assert_eq!(state.hits.agent_sort_toggle, Rect::default());
}

#[test]
fn threads_panel_draws_working_section_below_the_inbox() {
    let mut rows = sample_threads();
    rows.push(with(thread("w1", None, RowStatus::Working), |inbox| {
        inbox.working = true
    }));
    rows.push(with(thread("x1", None, RowStatus::Ready), |inbox| {
        inbox.settled = true
    }));
    fork_threads::set_view_for_test(rows, false);
    let (mut state, frame) = render(threads_config(""));
    let settled = line_of(&frame, "Settled 1");
    let working = line_of(&frame, " working 1");
    let w1 = line_of(&frame, "thread w1");
    assert!(working > settled && w1 > working, "{frame:#?}");
    assert!(
        !frame.iter().any(|row| row.contains("Working 1")),
        "no shelf"
    );
    assert!(
        !frame.iter().any(|row| row.contains("thread x1")),
        "closed shelf"
    );
    assert!(!frame.iter().any(|row| row.contains("Snoozed")));
    assert!(
        state.hits.agent_body.bottom() < working as u16,
        "the inbox scrolls above the working section"
    );
    let shelf = state
        .hits
        .fork_threads
        .iter()
        .find(|hit| hit.target == fork_threads::ThreadHitTarget::Shelf(Section::Settled))
        .unwrap()
        .rect;
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: shelf.x + 2,
        row: shelf.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let frame = frame_rows(&state.compose(100, 30).unwrap());
    assert!(
        frame.iter().any(|row| row.contains("thread x1")),
        "{frame:#?}"
    );
    assert!(fork_threads::view().shelves.settled_open);
}

#[test]
fn threads_panel_expands_the_inbox_by_click() {
    let rows = (0..10)
        .map(|index| {
            with(
                thread(&format!("t{index:02}"), None, RowStatus::Ready),
                |inbox| inbox.inbox_sort_at = Some(100 - index),
            )
        })
        .collect();
    fork_threads::set_view_for_test(rows, false);
    let (mut state, frame) = render(threads_config(""));
    line_of(&frame, "Show 2 more");
    assert!(!frame.iter().any(|row| row.contains("thread t09")));
    let expand = state
        .hits
        .fork_threads
        .iter()
        .find(|hit| hit.target == fork_threads::ThreadHitTarget::ExpandInbox)
        .unwrap()
        .rect;
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: expand.x + 4,
        row: expand.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(fork_threads::view().shelves.inbox_open);
}

#[test]
fn threads_panel_marks_stale_rows_offline() {
    fork_threads::set_view_for_test(sample_threads(), true);
    let (_, rows) = render(threads_config(""));
    assert!(rows[line_of(&rows, "threads")].contains("offline"));
    line_of(&rows, "thread t2");
}

#[test]
fn threads_panel_scrolls_with_the_wheel_like_the_agents_section() {
    let rows = (0..40)
        .map(|index| {
            with(
                thread(&format!("t{index:02}"), None, RowStatus::Ready),
                |inbox| inbox.inbox_sort_at = Some(100 - index),
            )
        })
        .collect();
    fork_threads::set_view_for_test(rows, false);
    fork_threads::update_view(|view| view.shelves.inbox_open = true);
    let (mut state, rows) = render(threads_config(""));
    assert!(rows.iter().any(|row| row.contains("thread t00")));
    assert!(state.hits.agent_max_scroll > 0);
    let body = state.hits.agent_body;
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: body.x + 2,
        row: body.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let rows = frame_rows(&state.compose(100, 30).unwrap());
    assert!(
        !rows.iter().any(|row| row.contains("thread t00")),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|row| row.contains("thread t01")));
}

#[test]
fn threads_disabled_leaves_the_agents_section_alone() {
    let (state, rows) = render(ClientShellConfig::from_config(&Config::default()));
    assert!(!rows.iter().any(|row| row.contains("threads")));
    assert!(rows.iter().any(|row| row.contains(" agents")));
    assert!(state.hits.fork_threads.is_empty());
}

#[test]
fn threads_collapsed_sidebar_numbers_the_displayed_rows() {
    fork_threads::set_view_for_test(sample_threads(), false);
    let mut state = ClientShellState::new(threads_config(""));
    state.config.palette = Palette::terminal();
    state.sidebar_collapsed = true;
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let rows = frame_rows(&state.compose(100, 30).unwrap());
    assert_eq!(thread_hits(&state), ["t2", "t4", "t3", "t1"]);
    let first = state.hits.fork_threads[0].rect;
    assert!(rows[first.y as usize].starts_with('1'), "{rows:#?}");
    assert!(rows[first.y as usize + 3].starts_with('4'));
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
        .find(|hit| hit.target == fork_threads::ThreadHitTarget::Thread(thread_id.into()))
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

    fork_threads::set_view_for_test(vec![thread("t1", None, RowStatus::Ready)], false);
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
    rows.push(thread("t5", Some("PROJ-1"), RowStatus::Ready));
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
fn threads_navigate_enters_after_the_last_workspace_and_enter_opens() {
    let mut state = mapped_state();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_3");
    // Down from the last workspace enters the first thread row.
    let outcome = state.handle_input_bytes(b"\x1b[B");
    assert!(outcome.actions.is_empty());
    assert_eq!(fork_threads::view().cursor, Some(0));
    assert!(state.navigate_workspace_id.is_none());
    // Display order is t2, t4, t3, t1; move down to t3.
    state.handle_input_bytes(b"\x1b[B");
    state.handle_input_bytes(b"\x1b[B");
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
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_1");
    // Up from the first workspace wraps to the last line of the panel,
    // the (always present) Settled shelf after t2, t4, t3, t1.
    state.handle_input_bytes(b"\x1b[A");
    assert_eq!(fork_threads::view().cursor, Some(4));
    // Down from the last line wraps to the first workspace.
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(fork_threads::view().cursor, None);
    assert_eq!(
        state
            .navigate_workspace_id
            .as_ref()
            .map(|target| target.workspace_id.as_str()),
        Some("ws_1")
    );
    // Up from the first line goes back to the last workspace.
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_3");
    state.handle_input_bytes(b"\x1b[B");
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
fn threads_navigate_enter_on_a_shelf_toggles_it() {
    let mut rows = sample_threads();
    rows.push(with(thread("x1", None, RowStatus::Ready), |inbox| {
        inbox.settled = true
    }));
    fork_threads::set_view_for_test(rows, false);
    let mut state = ClientShellState::new(threads_config(""));
    state.set_snapshot(Box::new(mapped_snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 30).unwrap();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_1");
    // Lines: t2, t4, t3, t1, [Settled 1]; up from ws_1 lands on the shelf.
    state.handle_input_bytes(b"\x1b[A");
    assert_eq!(fork_threads::view().cursor, Some(4));
    let outcome = state.handle_input_bytes(b"\r");
    assert!(outcome.actions.is_empty());
    assert!(fork_threads::view().shelves.settled_open);
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state.handle_input_bytes(b"\x1b[B");
    let outcome = state.handle_input_bytes(b"\r");
    assert_eq!(run_command(&outcome).command, "t3t -r x1");
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

fn press(state: &mut ClientShellState, character: char) -> ClientShellInput {
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Key(
        crate::input::TerminalKey::new(
            KeyCode::Char(character),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ),
    )])
}

fn focused_pane(outcome: &ClientShellInput) -> Option<String> {
    outcome.actions.iter().find_map(|action| match action {
        ClientShellAction::Endpoint { request, .. } => match &request.method {
            crate::api::schema::Method::PaneFocus(target) => Some(target.pane_id.clone()),
            _ => None,
        },
        _ => None,
    })
}

#[test]
fn threads_agent_bindings_drive_the_thread_rows() {
    // Display order t2, t4, t3, t1; t4 is on screen in pane_2.
    let mut projected = mapped_snapshot();
    projected.focused_workspace_id = Some("ws_2".into());
    projected.focused_pane_id = Some("pane_2".into());
    let state = || {
        fork_threads::set_view_for_test(sample_threads(), false);
        let mut state = ClientShellState::new(threads_config(""));
        state.set_snapshot(Box::new(projected.clone()));
        state.set_pane_surface(surface());
        state.compose(100, 30).unwrap();
        state
    };
    // next_agent: t4 → t3.
    let outcome = press(&mut state(), 'j');
    assert_eq!(run_command(&outcome).command, "t3t -r t3");
    // previous_agent: t4 → t2.
    let outcome = press(&mut state(), 'k');
    assert_eq!(run_command(&outcome).command, "t3t -r t2");
    // focus_agent 2 is t4, already on screen: focus only.
    let outcome = press(&mut state(), '2');
    assert_eq!(focused_pane(&outcome).as_deref(), Some("pane_2"));
    // focus_agent 4 is t1; 5 does not exist.
    let outcome = press(&mut state(), '4');
    assert_eq!(run_command(&outcome).command, "t3t -r t1");
    let mut five = state();
    let outcome = press(&mut five, '5');
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
    use crate::input::{KeybindAction, KeybindMatch};
    assert!(
        five.indexed_navigation_target_exists(&KeybindMatch::Action(KeybindAction::FocusAgent(3)))
    );
    assert!(
        !five.indexed_navigation_target_exists(&KeybindMatch::Action(KeybindAction::FocusAgent(4)))
    );
    // Nothing on screen: next starts at the first row, previous at the last.
    let mut fresh = mapped_state();
    fresh.config.keybinds = threads_config("").keybinds;
    assert_eq!(run_command(&press(&mut fresh, 'j')).command, "t3t -r t2");
    let mut fresh = mapped_state();
    fresh.config.keybinds = threads_config("").keybinds;
    assert_eq!(run_command(&press(&mut fresh, 'k')).command, "t3t -r t1");
}

#[test]
fn threads_agent_bindings_keep_their_upstream_meaning_without_threads() {
    let mut config = Config::default();
    config.keys.next_agent = crate::config::BindingConfig::one("ctrl+alt+j");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let mut projected = mapped_snapshot();
    projected.agents = vec![agent("pane_9", "ws_1", None, false)];
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(100, 30).unwrap();
    let outcome = press(&mut state, 'j');
    assert_eq!(focused_pane(&outcome).as_deref(), Some("pane_9"));
}
