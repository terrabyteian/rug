use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::app::{App, Focus};

/// Render the module tree pane.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Modules;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let visible_indices = app.visible_module_indices();
    let items: Vec<ListItem> = visible_indices
        .iter()
        .enumerate()
        .map(|(display_pos, &real_idx)| {
            let module = &app.modules[real_idx];
            let is_selected = display_pos == app.selected_module;
            let is_multi = app.multi_select.contains(&real_idx);

            let prefix = if is_multi { "● " } else { "  " };
            let name = &module.display_name;

            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_multi {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let line = Line::from(vec![Span::styled(format!("{prefix}{name}"), style)]);
            ListItem::new(line)
        })
        .collect();

    let depth_tag = match app.max_depth {
        Some(d) => format!(" [depth:{}]", d),
        None => String::new(),
    };
    let root_display = app.root.to_string_lossy();
    let title = if app.filter_active || !app.filter.is_empty() {
        format!(" {} [/{}]{} ", root_display, app.filter, depth_tag)
    } else {
        format!(" {}{} ", root_display, depth_tag)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    let mut state = ListState::default();
    if !visible_indices.is_empty() {
        state.select(Some(app.selected_module));
    }

    f.render_stateful_widget(list, area, &mut state);
}
