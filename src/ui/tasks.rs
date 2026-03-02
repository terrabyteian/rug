use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::app::{App, Focus};
use crate::task::{ResourceCounts, TaskStatus};

/// Render the task list pane, sorted by most recently active first.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Tasks;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let sorted = app.sorted_task_display();

    // Find which display position is currently selected (for ListState).
    let selected_display_pos = app.selected_task_id.and_then(|id| {
        sorted.iter().position(|&vi| app.tasks[vi].id == id)
    });

    let items: Vec<ListItem> = sorted
        .iter()
        .enumerate()
        .map(|(display_pos, &vec_idx)| {
            let task = &app.tasks[vec_idx];
            let is_selected = Some(display_pos) == selected_display_pos;
            let is_multi = app.task_multi_select.contains(&task.id);

            const CANCEL_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];

            let status_style = match &task.status {
                TaskStatus::Pending    => Style::default().fg(Color::DarkGray),
                TaskStatus::Running    => Style::default().fg(Color::Yellow),
                TaskStatus::Cancelling => Style::default().fg(Color::Magenta),
                TaskStatus::Success    => Style::default().fg(Color::Green),
                TaskStatus::Failed     => Style::default().fg(Color::Red),
                TaskStatus::Cancelled  => Style::default().fg(Color::DarkGray),
            };

            let icon = if task.status == TaskStatus::Cancelling {
                CANCEL_FRAMES[(app.spinner_tick as usize / 2) % CANCEL_FRAMES.len()]
            } else {
                task.status.icon()
            };
            let elapsed = task.elapsed_str();
            let elapsed_part = if elapsed.is_empty() {
                String::new()
            } else {
                format!("  {elapsed}")
            };

            let check = if is_multi { "✓ " } else { "  " };

            let mut spans = vec![
                Span::styled(check.to_string(), Style::default().fg(Color::Yellow)),
                Span::styled(format!("{icon} "), status_style),
                Span::raw(format!("{:<20} ", task.module_name)),
                Span::styled(format!("{:<8}", task.command), Style::default().fg(Color::Blue)),
                Span::styled(elapsed_part, Style::default().fg(Color::DarkGray)),
            ];

            if let Some(counts) = &task.resource_counts {
                spans.extend(count_spans(counts));
            }

            let line = Line::from(spans);

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(line).style(row_style)
        })
        .collect();

    let title = format!(" Tasks ({}) ", app.tasks.len());
    let list = List::new(items).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style),
    );

    let mut state = ListState::default();
    state.select(selected_display_pos);

    f.render_stateful_widget(list, area, &mut state);
}

/// Build coloured spans for resource operation counts.
///
/// Returns empty vec if there is nothing meaningful to show yet.
/// Shows `=` (dimmed) when a summary line confirmed no resource changes.
/// Shows coloured non-zero counts only:
///   +N  green   — add
///   ~N  yellow  — change
///   -N  red     — destroy
///   >N  cyan    — move
///   iN  cyan    — import
///   fN  gray    — forget (remove from state, OpenTofu)
fn count_spans(counts: &ResourceCounts) -> Vec<Span<'static>> {
    // No summary line seen yet — nothing to display.
    if !counts.has_summary && !counts.no_changes {
        return vec![];
    }

    // "No changes." or a real summary with everything at zero.
    if counts.no_changes || counts.all_zero() {
        return vec![Span::styled(
            "  =".to_string(),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let mut spans = Vec::new();

    let entries: &[(u32, &str, &str, Color)] = &[
        (counts.add,     "+", "add",     Color::Green),
        (counts.change,  "~", "change",  Color::Yellow),
        (counts.destroy, "-", "destroy", Color::Red),
        (counts.import,  "i", "import",  Color::Cyan),
        (counts.forget,  "f", "forget",  Color::DarkGray),
    ];

    for &(n, sym, label, color) in entries {
        if n > 0 {
            spans.push(Span::styled(
                format!("  {sym}{n} {label}"),
                Style::default().fg(color),
            ));
        }
    }

    spans
}
