//! fork: ticket picker overlay (FORK.md 0005). Drawn with the same panel,
//! search line and list rows as the worktree picker.

use super::*;
use crate::client::shell::fork_tickets::{picker_lines, PickerItem, PickerLine, PickerView};

/// Draws the picker centred over `b`. Returns the popup area and the text
/// cursor of the filter line.
pub(crate) fn fork_render_ticket_picker(
    b: &mut Buffer,
    view: &PickerView,
    p: &Palette,
) -> Option<(Rect, Option<crate::protocol::CursorState>)> {
    for y in b.area.y..b.area.bottom() {
        for x in b.area.x..b.area.right() {
            let c = &mut b[(x, y)];
            c.set_style(c.style().add_modifier(Modifier::DIM));
        }
    }
    let tickets = &view.list.tickets;
    let projects = &view.list.projects;
    let lines = picker_lines(&view.list, &view.items);
    let height = (lines.len() + 8).clamp(12, 26) as u16;
    let popup = popup(b.area, 96, height)?;
    let inner = panel(b, popup, p.accent, p.panel_bg)?;
    let base = Style::default().bg(p.panel_bg);
    put_text(
        b,
        inner.x,
        inner.y,
        inner.width,
        "new thread",
        base.fg(p.text).add_modifier(Modifier::BOLD),
    );
    let search = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    let count = if view.list.loading {
        "loading".to_owned()
    } else if view.list.offline {
        "offline".to_owned()
    } else {
        let shown_tickets = view
            .items
            .iter()
            .filter(|item| matches!(item, PickerItem::Ticket(_)))
            .count();
        let shown_projects = view.items.len() - shown_tickets;
        let part = |shown: usize, total: usize, noun: &str| {
            if shown == total {
                format!("{total} {noun}")
            } else {
                format!("{shown}/{total} {noun}")
            }
        };
        let mut count = part(shown_tickets, tickets.len(), "tickets");
        if !projects.is_empty() {
            count = format!(
                "{count} · {}",
                part(shown_projects, projects.len(), "projects")
            );
        }
        count
    };
    put_text(b, search.x, search.y, search.width, " / ", base.fg(p.text));
    let cursor = text_editor::render(
        b,
        Rect::new(
            search.x + 3,
            search.y,
            search.width.saturating_sub(4 + display_width(&count)),
            1,
        ),
        &view.query,
        base.fg(p.text),
    );
    if view.query.is_empty() {
        put_text(
            b,
            search.x + 4,
            search.y,
            search.width.saturating_sub(5 + display_width(&count)),
            "filter by label, title, ref or path",
            base.fg(p.overlay0),
        );
    }
    put_right_text(b, search, search.y, &count, base.fg(p.overlay0));
    put_text(
        b,
        inner.x,
        inner.y + 2,
        inner.width,
        &"─".repeat(inner.width as usize),
        base.fg(p.surface1),
    );
    let body = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(5),
    );
    let visible = (body.height as usize).max(1);
    let selected_line = lines
        .iter()
        .position(|line| *line == PickerLine::Item(view.selected))
        .unwrap_or(0);
    // Keep a group header in view above its first entry.
    let start = selected_line
        .saturating_sub(visible - 1)
        .min(lines.len().saturating_sub(visible));
    let label_width = view
        .items
        .iter()
        .filter_map(|item| match item {
            PickerItem::Ticket(index) => Some(display_width(&tickets[*index].label)),
            PickerItem::Project(_) => None,
        })
        .max()
        .unwrap_or(0);
    for (row, line) in lines.iter().skip(start).take(visible).enumerate() {
        let rect = Rect::new(body.x, body.y + row as u16, body.width, 1);
        let position = match line {
            PickerLine::ParkedHeader | PickerLine::ProjectsHeader => {
                let text = if *line == PickerLine::ParkedHeader {
                    " parked"
                } else {
                    " projects"
                };
                put_text(
                    b,
                    rect.x,
                    rect.y,
                    rect.width,
                    text,
                    base.fg(p.overlay0).add_modifier(Modifier::DIM),
                );
                continue;
            }
            PickerLine::Item(position) => *position,
        };
        let selected = position == view.selected;
        let style = if selected {
            Style::default()
                .fg(contrast(p))
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else if view.list.offline {
            base.fg(p.overlay0).add_modifier(Modifier::DIM)
        } else {
            base.fg(p.text)
        };
        b.set_style(rect, style);
        let ticket = match view.items[position] {
            PickerItem::Ticket(index) => &tickets[index],
            PickerItem::Project(index) => {
                let project = &projects[index];
                let root_width = (display_width(&project.root) + 2).min(rect.width / 2);
                put_text(
                    b,
                    rect.x + 1,
                    rect.y,
                    rect.width.saturating_sub(2 + root_width),
                    &project.title,
                    style,
                );
                put_right_text(
                    b,
                    Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), 1),
                    rect.y,
                    &project.root,
                    if selected {
                        style
                    } else {
                        style.fg(p.overlay0)
                    },
                );
                continue;
            }
        };
        let status_color = match ticket.status.as_str() {
            "in-progress" => p.yellow,
            "in-review" => p.teal,
            "todo" => p.text,
            _ => p.overlay0,
        };
        let status_style = if selected || view.list.offline {
            style
        } else {
            style.fg(status_color)
        };
        let ext_ref = ticket.ext_ref.as_deref().unwrap_or_default();
        let ref_width = if ext_ref.is_empty() {
            0
        } else {
            display_width(ext_ref) + 2
        };
        let mut x = rect.x + 1;
        put_text(b, x, rect.y, label_width, &ticket.label, style);
        x += label_width + 1;
        put_text(b, x, rect.y, 11, &ticket.status, status_style);
        x += 12;
        put_text(
            b,
            x,
            rect.y,
            rect.right().saturating_sub(x).saturating_sub(ref_width),
            &ticket.title,
            style,
        );
        if !ext_ref.is_empty() {
            put_right_text(
                b,
                Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), 1),
                rect.y,
                ext_ref,
                if selected {
                    style
                } else {
                    style.fg(p.overlay0)
                },
            );
        }
    }
    let empty = if view.list.loading {
        Some(" loading tickets…".to_owned())
    } else if tickets.is_empty() && projects.is_empty() && view.list.offline {
        Some(format!(
            " tickets unavailable: {}",
            view.list.error.as_deref().unwrap_or("service offline")
        ))
    } else if tickets.is_empty() && projects.is_empty() {
        Some(" no open tickets or projects".to_owned())
    } else if view.items.is_empty() {
        Some(" nothing matches".to_owned())
    } else {
        None
    };
    if let Some(empty) = empty {
        put_text(b, body.x, body.y, body.width, &empty, base.fg(p.overlay0));
    }
    if let Some(action) = view.action.as_deref().filter(|action| !action.is_empty()) {
        put_text(
            b,
            inner.x,
            inner.bottom() - 2,
            inner.width,
            &format!(" ↵ {action}"),
            base.fg(p.subtext0),
        );
    }
    put_text(
        b,
        inner.x,
        inner.bottom() - 1,
        inner.width,
        " type filter · ↑↓/ctrl+n/p move · enter new thread · esc close",
        base.fg(p.overlay0),
    );
    Some((popup, cursor))
}
