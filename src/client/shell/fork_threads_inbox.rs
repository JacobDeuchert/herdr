//! fork: the thread inbox rules of t3code's sidebar (FORK.md 0005).
//!
//! Pure functions over the rows the reader delivered and the wall clock:
//! which section a thread is in, the order inside each section, which lines
//! the panel shows (shelves, caps), and the label of each row. The service
//! ships everything that changes only on a thread event precomputed in the
//! `inbox` object; what flips with the clock (snooze expiry, "Woke", relative
//! times, durations) is decided here against `now`, so it changes without a
//! new fetch.

use super::fork_threads_feed::{RowStatus, ThreadRow};
use std::cmp::Ordering;

/// Settled rows shown when the shelf opens.
pub(crate) const SETTLED_INITIAL: usize = 10;
/// Settled rows each "Show N more" adds.
pub(crate) const SETTLED_PAGE: usize = 25;

const SECOND: i64 = 1_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Wall clock in Unix milliseconds.
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Section {
    Pinned,
    Active,
    Working,
    Snoozed,
    Settled,
}

impl Section {
    pub(crate) fn shelf_label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Snoozed => "Snoozed",
            Self::Settled => "Settled",
            Self::Pinned => "Pinned",
            Self::Active => "Active",
        }
    }

    /// Snoozed and Settled rows are slim: one line, no status label.
    pub(crate) fn slim(self) -> bool {
        matches!(self, Self::Snoozed | Self::Settled)
    }
}

/// Client-local shelf state; not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shelves {
    pub(crate) working_open: bool,
    pub(crate) snoozed_open: bool,
    pub(crate) settled_open: bool,
    pub(crate) settled_limit: usize,
}

impl Default for Shelves {
    fn default() -> Self {
        Self {
            working_open: false,
            snoozed_open: false,
            settled_open: false,
            settled_limit: SETTLED_INITIAL,
        }
    }
}

impl Shelves {
    pub(crate) fn is_open(&self, section: Section) -> bool {
        match section {
            Section::Working => self.working_open,
            Section::Snoozed => self.snoozed_open,
            Section::Settled => self.settled_open,
            Section::Pinned | Section::Active => true,
        }
    }

    pub(crate) fn toggle(&mut self, section: Section) {
        match section {
            Section::Working => self.working_open = !self.working_open,
            Section::Snoozed => self.snoozed_open = !self.snoozed_open,
            Section::Settled => self.settled_open = !self.settled_open,
            Section::Pinned | Section::Active => {}
        }
    }

    pub(crate) fn show_more(&mut self) {
        self.settled_limit = self.settled_limit.saturating_add(SETTLED_PAGE);
    }
}

/// `effectiveSnoozed`: a snooze in the future, unless a raised hand woke it.
pub(crate) fn effectively_snoozed(row: &ThreadRow, now: i64) -> bool {
    row.inbox.snoozed_until.is_some_and(|until| until > now) && !row.inbox.raised_hand
}

/// Precedence snoozed → settled → pinned → working → active.
pub(crate) fn section(row: &ThreadRow, now: i64) -> Section {
    let inbox = &row.inbox;
    if effectively_snoozed(row, now) {
        Section::Snoozed
    } else if inbox.settled {
        Section::Settled
    } else if inbox.pinned {
        Section::Pinned
    } else if inbox.working {
        Section::Working
    } else {
        Section::Active
    }
}

/// `threadWokeAt` with the clock-dependent half done here: the service's
/// raised-hand wake time, else the snooze end once it has passed.
pub(crate) fn woke_at(row: &ThreadRow, now: i64) -> Option<i64> {
    let until = row.inbox.snoozed_until?;
    row.inbox
        .raised_hand_woke_at
        .or_else(|| (until <= now).then_some(until))
}

/// The row's "Woke" state: woke after the last visit, and not settled.
pub(crate) fn is_woke(row: &ThreadRow, now: i64) -> bool {
    woke_at(row, now).is_some_and(|woke| {
        row.inbox
            .last_visited_at
            .is_none_or(|visited| visited < woke)
    }) && !row.inbox.settled
}

fn by_id(left: &ThreadRow, right: &ThreadRow) -> Ordering {
    left.id.cmp(&right.id)
}

fn newest_first(left: Option<i64>, right: Option<i64>) -> Ordering {
    right.unwrap_or(0).cmp(&left.unwrap_or(0))
}

fn sort_section(section: Section, indices: &mut [usize], rows: &[ThreadRow]) {
    let compare =
        |left: &usize, right: &usize| {
            let (left, right) = (&rows[*left], &rows[*right]);
            let (a, b) = (&left.inbox, &right.inbox);
            match section {
                Section::Pinned => match (&a.pin_order_key, &b.pin_order_key) {
                    (Some(x), Some(y)) => x.cmp(y),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => newest_first(a.created_at, b.created_at),
                }
                .then_with(|| by_id(left, right)),
                Section::Active => {
                    newest_first(a.inbox_sort_at, b.inbox_sort_at).then_with(|| by_id(left, right))
                }
                Section::Working => newest_first(a.working_sort_at, b.working_sort_at)
                    .then_with(|| by_id(left, right)),
                // Soonest wake first; ties keep the service order.
                Section::Snoozed => a
                    .snoozed_until
                    .unwrap_or(0)
                    .cmp(&b.snoozed_until.unwrap_or(0)),
                Section::Settled => newest_first(a.settled_sort_at, b.settled_sort_at)
                    .then_with(|| by_id(left, right)),
            }
        };
    indices.sort_by(compare);
}

/// One line of the thread panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboxLine {
    /// A thread, by index into the row list.
    Row { index: usize, section: Section },
    /// "Working N", "Snoozed N", "Settled N".
    Shelf {
        section: Section,
        count: usize,
        open: bool,
    },
    /// "Show N more" under the Settled rows.
    ShowMore(usize),
}

/// Lines top to bottom: pinned rows, active rows, then the Working and
/// Snoozed shelves (only when non-empty) and the Settled shelf (always,
/// unless the list is empty). A closed shelf still shows `shown`, the thread
/// on screen; the open Settled shelf is capped at `settled_limit`.
pub(crate) fn inbox_lines(
    rows: &[ThreadRow],
    now: i64,
    shelves: &Shelves,
    shown: Option<&str>,
) -> Vec<InboxLine> {
    let mut sections: [Vec<usize>; 5] = Default::default();
    let slot = |section: Section| match section {
        Section::Pinned => 0,
        Section::Active => 1,
        Section::Working => 2,
        Section::Snoozed => 3,
        Section::Settled => 4,
    };
    for (index, row) in rows.iter().enumerate() {
        if row.inbox.listed {
            sections[slot(section(row, now))].push(index);
        }
    }
    let order = [
        Section::Pinned,
        Section::Active,
        Section::Working,
        Section::Snoozed,
        Section::Settled,
    ];
    for section in order {
        sort_section(section, &mut sections[slot(section)], rows);
    }
    if sections.iter().all(Vec::is_empty) {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for section in order {
        let members = &sections[slot(section)];
        let row = |index: usize| InboxLine::Row { index, section };
        let is_shown = |index: &&usize| shown == Some(rows[**index].id.as_str());
        match section {
            Section::Pinned | Section::Active => lines.extend(members.iter().map(|i| row(*i))),
            Section::Working | Section::Snoozed if members.is_empty() => {}
            Section::Working | Section::Snoozed | Section::Settled => {
                let open = shelves.is_open(section);
                lines.push(InboxLine::Shelf {
                    section,
                    count: members.len(),
                    open,
                });
                if !open {
                    lines.extend(members.iter().filter(is_shown).map(|i| row(*i)));
                } else if section != Section::Settled {
                    lines.extend(members.iter().map(|i| row(*i)));
                } else {
                    let limit = shelves.settled_limit.min(members.len());
                    lines.extend(members[..limit].iter().map(|i| row(*i)));
                    let beyond = members[limit..].iter().find(is_shown);
                    lines.extend(beyond.map(|i| row(*i)));
                    let visible = limit + usize::from(beyond.is_some());
                    let hidden = members.len().saturating_sub(visible);
                    if hidden > 0 {
                        lines.push(InboxLine::ShowMore(hidden.min(SETTLED_PAGE)));
                    }
                }
            }
        }
    }
    lines
}

/// Row indices in display order; next/previous and focus N walk this.
pub(crate) fn displayed_rows(lines: &[InboxLine]) -> Vec<usize> {
    lines
        .iter()
        .filter_map(|line| match line {
            InboxLine::Row { index, .. } => Some(*index),
            _ => None,
        })
        .collect()
}

/// `just now`, `Nm`, `Nh`, `Nd`.
pub(crate) fn relative_time(at: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(at).max(0);
    if elapsed < MINUTE {
        "just now".into()
    } else if elapsed < HOUR {
        format!("{}m", elapsed / MINUTE)
    } else if elapsed < DAY {
        format!("{}h", elapsed / HOUR)
    } else {
        format!("{}d", elapsed / DAY)
    }
}

/// `Ns`, `Nm`, `Nh Mm`.
pub(crate) fn duration(since: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(since).max(0);
    if elapsed < MINUTE {
        format!("{}s", elapsed / SECOND)
    } else if elapsed < HOUR {
        format!("{}m", elapsed / MINUTE)
    } else {
        format!("{}h {}m", elapsed / HOUR, (elapsed % HOUR) / MINUTE)
    }
}

/// Snooze countdown: `Nm` under an hour, `Nh` under a day, else `Nd`,
/// rounded up.
pub(crate) fn snooze_countdown(until: i64, now: i64) -> String {
    let remaining = until.saturating_sub(now).max(0);
    let ceil = |unit: i64| (remaining + unit - 1) / unit;
    if remaining < HOUR {
        format!("{}m", ceil(MINUTE).max(1))
    } else if remaining < DAY {
        format!("{}h", ceil(HOUR))
    } else {
        format!("{}d", ceil(DAY))
    }
}

/// What a row shows on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowLabel {
    Approval,
    Input,
    Request,
    Waiting(Option<String>),
    SettingUp(Option<String>),
    Working(Option<String>),
    Goal(Option<String>),
    Limited,
    Failed,
    Woke,
    Done,
    /// A snoozed row's wake countdown.
    Snoozed(String),
    /// Relative time, no status.
    Time(String),
    Nothing,
}

impl RowLabel {
    pub(crate) fn text(&self) -> String {
        let with = |name: &str, detail: &Option<String>| match detail {
            Some(detail) => format!("{name} {detail}"),
            None => name.to_owned(),
        };
        match self {
            Self::Approval => "Approval".into(),
            Self::Input => "Input".into(),
            Self::Request => "Request".into(),
            Self::Waiting(detail) => with("Waiting", detail),
            Self::SettingUp(detail) => with("Setting up", detail),
            Self::Working(detail) => with("Working", detail),
            Self::Goal(detail) => with("Goal", detail),
            Self::Limited => "Limited".into(),
            Self::Failed => "Failed".into(),
            Self::Woke => "Woke".into(),
            Self::Done => "Done".into(),
            Self::Snoozed(countdown) => format!("◷ {countdown}"),
            Self::Time(time) => time.clone(),
            Self::Nothing => String::new(),
        }
    }
}

/// The label of a row in `section`: the status of a card, else Woke, Done,
/// or the relative time. Slim rows have no status: the snooze countdown,
/// Woke, or a relative time.
pub(crate) fn row_label(row: &ThreadRow, section: Section, now: i64) -> RowLabel {
    let inbox = &row.inbox;
    let time = |at: Option<i64>| {
        at.map_or(RowLabel::Nothing, |at| {
            RowLabel::Time(relative_time(at, now))
        })
    };
    if section.slim() {
        if section == Section::Snoozed {
            if let Some(until) = inbox.snoozed_until.filter(|until| *until > now) {
                return RowLabel::Snoozed(snooze_countdown(until, now));
            }
        }
        if is_woke(row, now) {
            return RowLabel::Woke;
        }
        return time(if section == Section::Settled {
            inbox.settled_sort_at.or(inbox.activity_at)
        } else {
            inbox.activity_at
        });
    }
    let since = |at: Option<i64>| at.map(|at| duration(at, now));
    match inbox.status {
        RowStatus::Approval => RowLabel::Approval,
        RowStatus::Input => RowLabel::Input,
        RowStatus::Request => RowLabel::Request,
        RowStatus::Waiting => RowLabel::Waiting(since(inbox.waiting_since)),
        RowStatus::Preparing => RowLabel::SettingUp(since(inbox.working_started_at)),
        RowStatus::Working if inbox.goal_active => RowLabel::Goal(since(inbox.working_started_at)),
        RowStatus::Working => RowLabel::Working(since(inbox.working_started_at)),
        RowStatus::Limited => RowLabel::Limited,
        RowStatus::Failed => RowLabel::Failed,
        RowStatus::Ready if is_woke(row, now) => RowLabel::Woke,
        RowStatus::Ready if inbox.unread => RowLabel::Done,
        RowStatus::Ready => time(inbox.activity_at),
    }
}

/// Recede (draw dimmed): always while preparing, working or waiting; a ready
/// or approval row only when it is neither unread nor woke. Never the row on
/// screen or under the cursor; the caller checks those.
pub(crate) fn receded(row: &ThreadRow, now: i64) -> bool {
    match row.inbox.status {
        RowStatus::Preparing | RowStatus::Working | RowStatus::Waiting => true,
        RowStatus::Ready | RowStatus::Approval => !row.inbox.unread && !is_woke(row, now),
        RowStatus::Input | RowStatus::Request | RowStatus::Limited | RowStatus::Failed => false,
    }
}

/// Next instant something clock-driven changes: a snooze ends (section,
/// Woke), or a relative time, duration or countdown ticks. `None` when
/// nothing on the list depends on the clock.
pub(crate) fn next_clock_change(rows: &[ThreadRow], now: i64) -> Option<i64> {
    let next_tick = |at: i64, fine: bool| {
        let elapsed = now.saturating_sub(at).max(0);
        let unit = if elapsed < MINUTE {
            if fine {
                SECOND
            } else {
                MINUTE
            }
        } else if elapsed < HOUR || fine {
            MINUTE
        } else if elapsed < DAY {
            HOUR
        } else {
            DAY
        };
        at + (elapsed / unit + 1) * unit
    };
    let mut next: Option<i64> = None;
    let mut consider = |at: i64| {
        if at > now {
            next = Some(next.map_or(at, |next| next.min(at)));
        }
    };
    for row in rows.iter().filter(|row| row.inbox.listed) {
        let inbox = &row.inbox;
        if let Some(until) = inbox.snoozed_until {
            if until > now {
                consider(until);
                // The countdown rounds up, so it changes on whole units
                // before the end.
                let remaining = until - now;
                let unit = if remaining < HOUR {
                    MINUTE
                } else if remaining < DAY {
                    HOUR
                } else {
                    DAY
                };
                consider(now + (remaining - 1) % unit + 1);
            }
        }
        for at in [inbox.activity_at, inbox.settled_sort_at]
            .into_iter()
            .flatten()
        {
            consider(next_tick(at, false));
        }
        for at in [inbox.working_started_at, inbox.waiting_since]
            .into_iter()
            .flatten()
        {
            consider(next_tick(at, true));
        }
    }
    next
}
