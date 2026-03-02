pub mod output;
pub mod tasks;
pub mod tree;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

use crate::app::{App, ConfirmKind, DragHandle, ExplorerOpKind, Focus, OpResult, PendingConfirm, PendingOp};

/// Run the full TUI event loop until the user quits.
pub fn run_tui(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        // Drain pending task events.
        app.drain_events();

        // Auto-exit once all tasks finish after a graceful-quit request.
        if app.pending_quit && app.all_tasks_done() {
            break;
        }

        terminal.draw(|f| draw(f, app))?;

        // Poll for input events with a short timeout so we keep draining task events.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
            Event::Mouse(mouse) => {
                // Hard overlays absorb all mouse events.
                if app.pending_quit || app.pending_confirm.is_some() || app.filter_active { continue; }

                // State explorer: only pass scroll events through.
                if app.state_explorer.is_some() {
                    let in_detail = app.state_explorer.as_ref()
                        .map(|e| e.detail_view.is_some())
                        .unwrap_or(false);
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            if in_detail { app.resource_detail_scroll(-3); }
                            else { app.state_explorer_move(-1); }
                        }
                        MouseEventKind::ScrollDown => {
                            if in_detail { app.resource_detail_scroll(3); }
                            else { app.state_explorer_move(1); }
                        }
                        _ => {}
                    }
                    continue;
                }

                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Ok(size) = terminal.size() {
                            let col = mouse.column;
                            let row = mouse.row;
                            let h_split = app.effective_h_split(size.width);
                            let v_split = app.effective_v_split(size.height);
                            // Hit-test the vertical divider (within 1 col).
                            if col + 1 == h_split || col == h_split {
                                app.dragging = Some(DragHandle::Vertical);
                            // Hit-test the horizontal divider (right panel, within 1 row).
                            } else if col >= h_split && (row + 1 == v_split || row == v_split) {
                                app.dragging = Some(DragHandle::Horizontal);
                            } else {
                                app.dragging = None;
                                if let Some(focus) = pane_for_click(col, row, size, app) {
                                    app.focus = focus;
                                }
                            }
                        }
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if let Ok(size) = terminal.size() {
                            match app.dragging {
                                Some(DragHandle::Vertical) => {
                                    app.h_split_col = Some(
                                        mouse.column.clamp(5, size.width.saturating_sub(10)),
                                    );
                                }
                                Some(DragHandle::Horizontal) => {
                                    app.v_split_row = Some(
                                        mouse.row.clamp(4, size.height.saturating_sub(4)),
                                    );
                                }
                                None => {}
                            }
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        app.dragging = None;
                    }
                    MouseEventKind::ScrollUp => match app.focus {
                        Focus::Modules => { app.move_module_selection(-1); }
                        Focus::Tasks   => { app.move_task_selection(-1); }
                        Focus::Output  => { app.scroll_output(3); }
                    },
                    MouseEventKind::ScrollDown => match app.focus {
                        Focus::Modules => { app.move_module_selection(1); }
                        Focus::Tasks   => { app.move_task_selection(1); }
                        Focus::Output  => { app.scroll_output(-3); }
                    },
                    _ => {}
                }
            }
            Event::Key(key) => {
                // Allow Ctrl-C to quit from anywhere.
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')
                {
                    break;
                }

                // Fullscreen output mode: Esc exits, scroll keys pass through, all else swallowed.
                if app.output_fullscreen {
                    match key.code {
                        KeyCode::Esc => {
                            app.output_fullscreen = false;
                            execute!(io::stdout(), EnableMouseCapture)?;
                        }
                        KeyCode::Char('j') | KeyCode::Down  => { app.scroll_output(-1); }
                        KeyCode::Char('k') | KeyCode::Up    => { app.scroll_output(1); }
                        KeyCode::Char('g') => app.go_to_first(),
                        KeyCode::Char('G') => app.go_to_last(),
                        _ => {}
                    }
                    continue;
                }

                // Graceful-quit overlay: q force-quits, Esc cancels, everything else is swallowed.
                if app.pending_quit {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Esc => app.pending_quit = false,
                        _ => {}
                    }
                    continue;
                }

                // Confirmation dialog intercepts all keys.
                if app.pending_confirm.is_some() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                            app.confirm_execute();
                        }
                        _ => {
                            app.cancel_confirm();
                        }
                    }
                    continue;
                }

                // Filter input mode.
                if app.filter_active {
                    match key.code {
                        KeyCode::Esc => {
                            app.filter_active = false;
                            app.filter.clear();
                        }
                        KeyCode::Enter => {
                            app.filter_active = false;
                        }
                        KeyCode::Backspace => {
                            app.filter.pop();
                        }
                        KeyCode::Char(c) => {
                            app.filter.push(c);
                        }
                        _ => {}
                    }
                    continue;
                }

                // Help overlay.
                if app.show_help {
                    app.show_help = false;
                    continue;
                }

                // State explorer overlay.
                if app.state_explorer.is_some() {
                    let has_op_result = app.state_explorer.as_ref()
                        .map(|e| e.op_result.is_some()).unwrap_or(false);
                    let op_confirm = app.state_explorer.as_ref()
                        .and_then(|e| e.op_confirm);
                    let has_pending_op = app.state_explorer.as_ref()
                        .map(|e| e.pending_op.is_some()).unwrap_or(false);
                    let in_detail = app.state_explorer.as_ref()
                        .map(|e| e.detail_view.is_some()).unwrap_or(false);
                    let filter_active = app.state_explorer.as_ref()
                        .map(|e| e.filter_active).unwrap_or(false);

                    if has_op_result {
                        // Any key dismisses the result overlay.
                        app.dismiss_op_result();
                    } else if op_confirm.is_some() {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => { app.start_op(); }
                            _ => { app.cancel_op_confirm(); }
                        }
                    } else if has_pending_op {
                        // Absorb all keys while the op is running.
                    } else if in_detail {
                        match key.code {
                            KeyCode::Esc => { app.close_resource_detail(); }
                            KeyCode::Char('q') => { app.close_state_explorer(); }
                            KeyCode::Char('j') | KeyCode::Down => { app.resource_detail_scroll(1); }
                            KeyCode::Char('k') | KeyCode::Up   => { app.resource_detail_scroll(-1); }
                            KeyCode::Char('g') => { app.resource_detail_go_first(); }
                            KeyCode::Char('G') => { app.resource_detail_go_last(); }
                            _ => {}
                        }
                    } else if filter_active {
                        match key.code {
                            KeyCode::Esc => { app.state_explorer_clear_filter(); }
                            KeyCode::Enter => { app.state_explorer_deactivate_filter(); }
                            KeyCode::Backspace => { app.state_explorer_filter_pop(); }
                            KeyCode::Char(c) => { app.state_explorer_filter_push(c); }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => { app.close_state_explorer(); }
                            KeyCode::Enter => { app.open_resource_detail(); }
                            KeyCode::Char('j') | KeyCode::Down => { app.state_explorer_move(1); }
                            KeyCode::Char('k') | KeyCode::Up   => { app.state_explorer_move(-1); }
                            KeyCode::Char('g') => { app.state_explorer_go_first(); }
                            KeyCode::Char('G') => { app.state_explorer_go_last(); }
                            KeyCode::Char('/') => { app.state_explorer_activate_filter(); }
                            KeyCode::Char(' ') => { app.state_explorer_toggle_select(); }
                            KeyCode::Char('c') => { app.state_explorer_clear_select(); }
                            KeyCode::Char('t') => { app.request_op_confirm(ExplorerOpKind::Taint); }
                            KeyCode::Char('D') => { app.request_op_confirm(ExplorerOpKind::StateRm); }
                            KeyCode::Char('r') => { app.refresh_state_explorer(); }
                            _ => {}
                        }
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => {
                        if app.active_tasks().is_empty() {
                            break;
                        }
                        app.pending_quit = true;
                    }
                    KeyCode::Char('h') | KeyCode::Char('?') => app.show_help = !app.show_help,
                    KeyCode::Char('r') => app.refresh_modules(),
                    KeyCode::Tab => app.cycle_focus(),

                    // Clear filter.
                    KeyCode::Esc => app.filter.clear(),

                    // Vim first/last navigation.
                    KeyCode::Char('g') => app.go_to_first(),
                    KeyCode::Char('G') => app.go_to_last(),

                    // Navigation.
                    KeyCode::Char('j') | KeyCode::Down => { match app.focus {
                        Focus::Modules => { app.move_module_selection(1); }
                        Focus::Tasks   => { app.move_task_selection(1); }
                        Focus::Output  => { app.scroll_output(-1); }
                    } }
                    KeyCode::Char('k') | KeyCode::Up => { match app.focus {
                        Focus::Modules => { app.move_module_selection(-1); }
                        Focus::Tasks   => { app.move_task_selection(-1); }
                        Focus::Output  => { app.scroll_output(1); }
                    } }

                    // Module actions.
                    KeyCode::Char(' ') => {
                        if app.focus == Focus::Modules {
                            if key.modifiers.contains(KeyModifiers::CONTROL) {
                                app.range_select();
                            } else {
                                app.toggle_multi_select();
                            }
                        }
                    }
                    KeyCode::Char('c') => app.multi_select.clear(),
                    KeyCode::Enter => {
                        if app.focus == Focus::Modules {
                            app.open_state_explorer();
                        } else if app.focus == Focus::Output {
                            app.output_fullscreen = true;
                            execute!(io::stdout(), DisableMouseCapture)?;
                        }
                        // Tasks pane: selection already managed via selected_task
                    }

                    // Terraform commands.
                    KeyCode::Char('i') => app.enqueue_command("init", vec![]),
                    KeyCode::Char('u') => app.request_init_upgrade_confirm(),
                    KeyCode::Char('p') => app.enqueue_plan(),
                    // Destructive commands require confirmation.
                    KeyCode::Char('a') => app.request_apply_confirm(),
                    KeyCode::Char('d') => app.request_destroy_confirm(),

                    // Depth limiter.
                    KeyCode::Char('[') => app.decrease_depth(),
                    KeyCode::Char(']') => app.increase_depth(),

                    // Filter.
                    KeyCode::Char('/') => {
                        app.filter_active = true;
                        app.filter.clear();
                    }

                    _ => {}
                }
            } // end Event::Key
            _ => {}
            } // end match event::read()
        }
    }
    Ok(())
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    use ratatui::layout::{Constraint, Direction, Layout};

    let area = f.area();

    if app.output_fullscreen {
        output::render(f, area, app);
        return;
    }

    // State explorer (list or detail) takes over the full window.
    if let Some(explorer) = &app.state_explorer {
        render_state_explorer(f, area, explorer);
        // Op overlays render on top of the state explorer.
        if let Some(op_kind) = explorer.op_confirm {
            render_op_confirm(f, area, op_kind, &explorer.op_targets);
        } else if let Some(pt) = &explorer.pending_op {
            render_op_progress(f, area, pt);
        } else if let Some(result) = &explorer.op_result {
            render_op_result(f, area, result);
        }
        return;
    }

    // Outer split: left (modules) | right (output + tasks)
    let left_width = app.effective_h_split(area.width);
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_width), Constraint::Min(0)])
        .split(area);

    // Right split: output (top) | tasks (bottom)
    let top_height = app.effective_v_split(area.height);
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_height), Constraint::Min(0)])
        .split(h_chunks[1]);

    tree::render(f, h_chunks[0], app);
    output::render(f, v_chunks[0], app);
    tasks::render(f, v_chunks[1], app);

    if app.pending_quit {
        render_quit_wait(f, area, app);
    }

    if app.show_help {
        render_help(f, area);
    }

    if app.filter_active {
        render_filter_bar(f, area, &app.filter);
    }

    if let Some(confirm) = &app.pending_confirm {
        render_confirm(f, area, confirm);
    }
}

fn render_help(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    use ratatui::{
        layout::{Alignment, Rect},
        style::{Color, Style},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let help_text = "\
j/k ↑/↓   Navigate lists or scroll output
g / G      Jump to first / last
Space      Toggle module multi-select
Ctrl+Space Range-select modules
c          Clear selection
Enter      State explorer (Modules) / Fullscreen (Output)
Esc        Close overlay / clear filter
i          Init selected modules
u          Init -upgrade selected modules
p          Plan selected modules
a          Apply selected modules
d          Destroy selected modules
/          Filter modules by name
[ / ]      Decrease / increase depth
r          Refresh module list
Tab        Cycle focus between panes
h / ?      Toggle this help
q / Ctrl-C Quit";

    let width = 52u16;
    let height = 22u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width.min(area.width), height.min(area.height));

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(help_text)
            .block(
                Block::default()
                    .title(" Help (any key to close) ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

/// Map a mouse click position to the pane it landed in.
fn pane_for_click(col: u16, row: u16, size: ratatui::layout::Size, app: &App) -> Option<Focus> {
    if size.width == 0 || size.height == 0 {
        return None;
    }
    let h_split = app.effective_h_split(size.width);
    if col < h_split {
        Some(Focus::Modules)
    } else {
        let v_split = app.effective_v_split(size.height);
        if row < v_split {
            Some(Focus::Output)
        } else {
            Some(Focus::Tasks)
        }
    }
}

fn render_filter_bar(f: &mut ratatui::Frame, area: ratatui::layout::Rect, filter: &str) {
    use ratatui::{
        layout::Rect,
        style::{Color, Style},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let bar = Rect::new(area.x, area.y + area.height.saturating_sub(3), area.width, 3);
    f.render_widget(Clear, bar);
    f.render_widget(
        Paragraph::new(format!("/{filter}_"))
            .block(
                Block::default()
                    .title(" Filter ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
        bar,
    );
}

fn render_confirm(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    confirm: &PendingConfirm,
) {
    use ratatui::{
        layout::{Alignment, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let label = confirm.kind.label();
    let n = confirm.targets.len();
    let noun = if n == 1 { "module" } else { "modules" };

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("  Run {label} on {n} {noun}:"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for target in &confirm.targets {
        let name_span = Span::raw(format!("    • {:<28}", target.module_name));

        let plan_span = match &confirm.kind {
            ConfirmKind::Apply => match &target.plan_age {
                Some(age) => Span::styled(
                    format!("plan from {age}"),
                    Style::default().fg(Color::Green),
                ),
                None => Span::styled(
                    "no prior plan".to_string(),
                    Style::default().fg(Color::Yellow),
                ),
            },
            ConfirmKind::Destroy | ConfirmKind::InitUpgrade => Span::raw(""),
        };

        lines.push(Line::from(vec![name_span, plan_span]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "[y] Confirm",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("[any] Cancel", Style::default().fg(Color::DarkGray)),
    ]));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = 58u16.min(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" Confirm {label} "))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_quit_wait(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    use ratatui::{
        layout::{Alignment, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let active = app.active_tasks();

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "  Waiting for tasks to finish…",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for task in &active {
        lines.push(Line::from(vec![
            Span::raw(format!("  {} ", task.status.icon())),
            Span::styled(
                format!("{:<28}", task.module_name),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(task.command.clone()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "[q] Force quit",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            "[Esc] Cancel",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
    ]));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = 58u16.min(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" Quitting — {} task(s) remaining ", active.len()))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_state_explorer(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    explorer: &crate::app::StateExplorer,
) {
    // If the user has drilled into a resource, show that view instead.
    if let Some(detail) = &explorer.detail_view {
        render_resource_detail(f, area, &explorer.module_name, detail);
        return;
    }

    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
    };
    use crate::state::{StateContent, StateResource};

    // Reserve rows: 2 border + 1 blank top + 1 blank before hint + 1 hint + 1 counter (worst case).
    let max_resource_rows = area.height.saturating_sub(7).max(3) as usize;

    let mut lines: Vec<Line> = vec![Line::from("")];

    match &explorer.content {
        StateContent::NotInitialized => {
            lines.push(Line::from(Span::styled(
                "  Not initialized",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  Run `i` to initialize this module.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        StateContent::NoState => {
            lines.push(Line::from(Span::styled(
                "  No state",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  Module is initialized but has no resources in state.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        StateContent::Resources(resources) => {
            // Apply filter, carrying unfiltered indices.
            let filter_lower = explorer.filter.to_lowercase();
            let filtered: Vec<(usize, &StateResource)> = if explorer.filter.is_empty() {
                resources.iter().enumerate().collect()
            } else {
                resources
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.address.to_lowercase().contains(&filter_lower))
                    .collect()
            };

            let total = resources.len();
            let visible_count = filtered.len();

            if filtered.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  No resources match \"{}\"", explorer.filter),
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                // Scroll window: keep selected visible at bottom of window.
                let scroll_start = explorer.selected
                    .saturating_sub(max_resource_rows.saturating_sub(1))
                    .min(visible_count.saturating_sub(max_resource_rows));

                if scroll_start > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("  ↑ {} more above", scroll_start),
                        Style::default().fg(Color::DarkGray),
                    )));
                }

                for (vi, (real_idx, resource)) in filtered
                    .iter()
                    .enumerate()
                    .skip(scroll_start)
                    .take(max_resource_rows)
                {
                    let is_selected = vi == explorer.selected;
                    let is_multi = explorer.multi_select.contains(real_idx);
                    let tainted = resource.is_tainted();

                    if is_selected {
                        let prefix = if is_multi { "●" } else { " " };
                        let taint_tag = if tainted { " [tainted]" } else { "" };
                        let full = format!(" {} {}{}", prefix, resource.address, taint_tag);
                        lines.push(Line::from(Span::styled(
                            full,
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )));
                    } else if is_multi {
                        let mut spans = vec![
                            Span::styled(
                                " ● ".to_string(),
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(resource.address.clone()),
                        ];
                        if tainted {
                            spans.push(Span::styled(
                                " [tainted]".to_string(),
                                Style::default().fg(Color::Red),
                            ));
                        }
                        lines.push(Line::from(spans));
                    } else if !explorer.filter.is_empty() {
                        let addr = format!("   {}", resource.address);
                        let mut line = highlight_filter_match(addr, &filter_lower);
                        if tainted {
                            line.spans.push(Span::styled(
                                " [tainted]".to_string(),
                                Style::default().fg(Color::Red),
                            ));
                        }
                        lines.push(line);
                    } else {
                        let mut spans = vec![Span::raw(format!("   {}", resource.address))];
                        if tainted {
                            spans.push(Span::styled(
                                " [tainted]".to_string(),
                                Style::default().fg(Color::Red),
                            ));
                        }
                        lines.push(Line::from(spans));
                    }
                }

                let shown_end = scroll_start + max_resource_rows;
                if shown_end < visible_count {
                    lines.push(Line::from(Span::styled(
                        format!("  ↓ {} more below", visible_count - shown_end),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }

            // Counter: "12 / 48" when filtering, "48" when not.
            lines.push(Line::from(""));
            if !explorer.filter.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {} / {} resources", visible_count, total),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    // Filter bar (always present for resources; shows hint or active input).
    let filter_line = if explorer.filter_active {
        Line::from(vec![
            Span::styled("  /", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(explorer.filter.clone(), Style::default().fg(Color::White)),
            Span::styled("█", Style::default().fg(Color::Cyan)),
        ])
    } else if !explorer.filter.is_empty() {
        Line::from(vec![
            Span::styled("  /", Style::default().fg(Color::DarkGray)),
            Span::styled(
                explorer.filter.clone(),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            ),
            Span::styled("  (Esc to clear)", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled("[j/k] Nav", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[Space] Select", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[t] Taint", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[D] Remove from state", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[Enter] Inspect", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[/] Filter", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[r] Refresh", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("[Esc] Close", Style::default().fg(Color::DarkGray)),
        ])
    };
    lines.push(Line::from(""));
    lines.push(filter_line);

    // Title: show resource count and selection count.
    let title_suffix = if let StateContent::Resources(r) = &explorer.content {
        let n = r.len();
        let sel = explorer.multi_select.len();
        if sel > 0 {
            format!(" — {} resource{} ({} selected)", n, if n == 1 { "" } else { "s" }, sel)
        } else {
            format!(" — {} resource{}", n, if n == 1 { "" } else { "s" })
        }
    } else {
        String::new()
    };

    let border_color = if explorer.filter_active { Color::Cyan } else { Color::Blue };

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" State: {}{} ", explorer.module_name, title_suffix))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

fn render_resource_detail(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    module_name: &str,
    detail: &crate::app::ResourceDetail,
) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
    };

    // Reserve: 2 border + 1 blank + 1 address + 1 separator + 1 blank + 1 hint.
    let max_body_rows = area.height.saturating_sub(9).max(3) as usize;
    // Separator fills the inner width (area minus 2 for borders, minus 2 for padding).
    let sep_width = (area.width as usize).saturating_sub(4);

    let total_lines = detail.lines.len();
    let scroll = detail.scroll.min(total_lines.saturating_sub(1));

    let mut lines: Vec<Line> = Vec::new();

    // Address header.
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            detail.address.clone(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
    ]));
    // Separator spanning the full inner width.
    lines.push(Line::from(Span::styled(
        "  ".to_string() + &"─".repeat(sep_width),
        Style::default().fg(Color::DarkGray),
    )));

    // Scroll-up indicator.
    if scroll > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} lines above", scroll),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // JSON body lines.
    for raw_line in detail.lines.iter().skip(scroll).take(max_body_rows) {
        lines.push(style_json_line(raw_line));
    }

    // Scroll-down indicator.
    let shown_end = scroll + max_body_rows;
    if shown_end < total_lines {
        lines.push(Line::from(Span::styled(
            format!("  ↓ {} lines below", total_lines - shown_end),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Footer hint.
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("[j/k] Scroll", Style::default().fg(Color::DarkGray)),
        Span::raw("   "),
        Span::styled("[Esc] Back to list", Style::default().fg(Color::DarkGray)),
    ]));

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" State: {} ", module_name))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

/// Syntax-highlight a single line from `serde_json::to_string_pretty` output.
fn style_json_line(line: &str) -> ratatui::text::Line<'static> {
    use ratatui::{style::{Color, Style}, text::{Line, Span}};

    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let indent = line[..indent_len].to_string();
    let trimmed_no_comma = trimmed.trim_end_matches(',');
    let trailing_comma = if trimmed.ends_with(',') { "," } else { "" };

    // Pure structure tokens.
    if matches!(trimmed_no_comma, "{" | "}" | "[" | "]") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Key-value line: `"key": value`.
    if trimmed.starts_with('"') {
        if let Some(key_end) = json_string_end(trimmed, 0) {
            let after_key = trimmed[key_end..].trim_start();
            if after_key.starts_with(':') {
                let key = trimmed[..key_end].to_string();
                let value_raw = after_key[1..].trim_start();
                let value_body = value_raw.trim_end_matches(',');
                let value_comma = if value_raw.ends_with(',') { "," } else { "" };
                return Line::from(vec![
                    Span::raw(indent),
                    Span::styled(key, Style::default().fg(Color::Yellow)),
                    Span::raw(": ".to_string()),
                    json_value_span(value_body),
                    Span::raw(value_comma.to_string()),
                ]);
            }
        }
    }

    // Bare value (inside an array).
    Line::from(vec![
        Span::raw(indent),
        json_value_span(trimmed_no_comma),
        Span::raw(trailing_comma.to_string()),
    ])
}

/// Return a styled `Span` for a JSON value token.
fn json_value_span(v: &str) -> ratatui::text::Span<'static> {
    use ratatui::{style::{Color, Style}, text::Span};
    match v {
        "true" | "false" => Span::styled(v.to_string(), Style::default().fg(Color::Magenta)),
        "null"           => Span::styled(v.to_string(), Style::default().fg(Color::DarkGray)),
        "{" | "[" | "}" | "]" => Span::styled(v.to_string(), Style::default().fg(Color::DarkGray)),
        _ if v.starts_with('"') => Span::styled(v.to_string(), Style::default().fg(Color::Green)),
        _ => Span::styled(v.to_string(), Style::default().fg(Color::Cyan)),
    }
}

/// Find the byte index just past the closing `"` of a JSON string in `s`
/// starting at `start` (which must point at the opening `"`).
/// Returns `None` if the string is not well-formed.
fn json_string_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(start) != Some(&b'"') { return None; }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => { i += 2; }      // escaped character — skip both bytes
            b'"'  => { return Some(i + 1); }
            _     => { i += 1; }
        }
    }
    None
}

fn render_op_confirm(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    kind: ExplorerOpKind,
    targets: &[String],
) {
    use ratatui::{
        layout::{Alignment, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let n = targets.len();
    let noun = if n == 1 { "resource" } else { "resources" };
    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} {} {}:", kind.confirm_verb(), n, noun),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for addr in targets {
        lines.push(Line::from(Span::styled(
            format!("    • {}", addr),
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("[y] Confirm", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled("[any] Cancel", Style::default().fg(Color::DarkGray)),
    ]));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = 64u16.min(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(kind.confirm_title())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_op_progress(f: &mut ratatui::Frame, area: ratatui::layout::Rect, pt: &PendingOp) {
    use ratatui::{
        layout::{Alignment, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let mut lines: Vec<Line> = vec![Line::from(""), Line::from("")];
    for (addr, success) in &pt.done {
        let (icon, color) = if *success { ("✓", Color::Green) } else { ("✗", Color::Red) };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(addr.clone()),
        ]));
    }
    if let Some((_, addr)) = &pt.running {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("⋯ ", Style::default().fg(Color::Yellow)),
            Span::raw(addr.clone()),
        ]));
    }
    for addr in &pt.queue {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("· ", Style::default().fg(Color::DarkGray)),
            Span::styled(addr.clone(), Style::default().fg(Color::DarkGray)),
        ]));
    }

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = 64u16.min(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(pt.kind.progress_title())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_op_result(f: &mut ratatui::Frame, area: ratatui::layout::Rect, result: &OpResult) {
    use ratatui::{
        layout::{Alignment, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let all_ok = result.entries.iter().all(|(_, ok)| *ok);
    let border_color = if all_ok { Color::Green } else { Color::Red };

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (addr, success) in &result.entries {
        let (icon, color) = if *success { ("✓", Color::Green) } else { ("✗", Color::Red) };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Span::raw(addr.clone()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [any key] Dismiss",
        Style::default().fg(Color::DarkGray),
    )));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = 64u16.min(area.width);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(result.kind.result_title(all_ok))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color)),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

/// Render an address line with the filter match portion highlighted in bold yellow.
/// `text` includes the leading "  " padding; `filter_lower` is already lowercased.
fn highlight_filter_match(text: String, filter_lower: &str) -> ratatui::text::Line<'static> {
    use ratatui::{style::{Color, Modifier, Style}, text::{Line, Span}};

    let text_lower = text.to_lowercase();
    if let Some(pos) = text_lower.find(filter_lower) {
        let end = pos + filter_lower.len();
        // pos/end are byte offsets from the lowercased string; valid for ASCII
        // filter strings (the common case). Fall back to plain for non-ASCII.
        if text.is_char_boundary(pos) && text.is_char_boundary(end) {
            return Line::from(vec![
                Span::raw(text[..pos].to_string()),
                Span::styled(
                    text[pos..end].to_string(),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::raw(text[end..].to_string()),
            ]);
        }
    }
    Line::from(Span::raw(text))
}
