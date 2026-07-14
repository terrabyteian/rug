use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::app::{App, Focus};
use crate::task::{ResourceCounts, TaskStatus};
use crate::ui::theme;
use crate::ui::wrap::wrap_line;

/// Render the task list pane, sorted by most recently active first.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Tasks;
    let border_style = theme::pane_border(focused);

    let sorted = app.sorted_task_display();
    let inner_width = area.width.saturating_sub(2);
    let max_item_lines = area.height.saturating_sub(2).max(1) as usize;

    // Find which display position is currently selected (for ListState).
    let selected_display_pos = app
        .selected_task_id
        .and_then(|id| sorted.iter().position(|&vi| app.engine.tasks[vi].id == id));

    let items: Vec<ListItem> = sorted
        .iter()
        .enumerate()
        .map(|(display_pos, &vec_idx)| {
            let task = &app.engine.tasks[vec_idx];
            let is_selected = Some(display_pos) == selected_display_pos;
            let is_multi = app.task_multi_select.contains(&task.id);

            let status_style = theme::status_style(&task.status);

            let icon = if task.status == TaskStatus::Cancelling {
                theme::SPINNER_FRAMES[(app.spinner_tick as usize / 2) % theme::SPINNER_FRAMES.len()]
            } else {
                theme::status_icon(&task.status)
            };
            let elapsed = task.elapsed_str();
            let elapsed_part = if elapsed.is_empty() {
                String::new()
            } else {
                format!("  {elapsed}")
            };

            let check = if is_multi { "✓ " } else { "  " };
            let ready_plan = app.ready_plan_for_task(task);

            let mut spans = vec![
                Span::styled(check.to_string(), theme::multi_select_marker()),
                Span::styled(format!("{icon} "), status_style),
                Span::raw(format!("{:<20} ", task.module_name)),
                Span::styled(format!("{:<8}", task.command), theme::command_text()),
                Span::styled(elapsed_part, theme::dim()),
            ];

            if let Some(plan) = ready_plan {
                spans.push(Span::styled(format!("  P:{}", plan.age_str()), theme::plan_marker()));
            }

            if let Some(counts) = &task.resource_counts {
                spans.extend(count_spans(counts));
            }

            let line = Line::from(spans);

            let row_style = if is_selected {
                theme::selected_task_row()
            } else {
                Style::default()
            };

            ListItem::new(wrap_line(line, inner_width, 4, max_item_lines)).style(row_style)
        })
        .collect();

    let title = format!(" Tasks ({}) ", app.engine.tasks.len());
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
            Style::default().fg(theme::COUNT_NONE),
        )];
    }

    let mut spans = Vec::new();

    let entries: &[(u32, &str, &str, Color)] = &[
        (counts.add, "+", "add", theme::COUNT_ADD),
        (counts.change, "~", "change", theme::COUNT_CHANGE),
        (counts.destroy, "-", "destroy", theme::COUNT_DESTROY),
        (counts.import, "i", "import", theme::COUNT_IMPORT),
        (counts.forget, "f", "forget", theme::COUNT_FORGET),
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
