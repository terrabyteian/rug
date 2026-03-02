use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::app::{App, Focus};
use crate::task::TaskStatus;

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

            let status_style = match &task.status {
                TaskStatus::Pending   => Style::default().fg(Color::DarkGray),
                TaskStatus::Running   => Style::default().fg(Color::Yellow),
                TaskStatus::Success   => Style::default().fg(Color::Green),
                TaskStatus::Failed    => Style::default().fg(Color::Red),
                TaskStatus::Cancelled => Style::default().fg(Color::DarkGray),
            };

            let icon = task.status.icon();
            let elapsed = task.elapsed_str();
            let elapsed_part = if elapsed.is_empty() {
                String::new()
            } else {
                format!("  {elapsed}")
            };

            let line = Line::from(vec![
                Span::styled(format!(" {icon} "), status_style),
                Span::raw(format!("{:<20} ", task.module_name)),
                Span::styled(format!("{:<8}", task.command), Style::default().fg(Color::Blue)),
                Span::styled(elapsed_part, Style::default().fg(Color::DarkGray)),
            ]);

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
