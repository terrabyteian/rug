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

use crate::app::{App, ConfirmKind, Focus, PendingConfirm};

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
                // Overlays absorb all mouse events.
                if app.pending_quit || app.pending_confirm.is_some() || app.filter_active { continue; }
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Ok(size) = terminal.size() {
                            if let Some(focus) = pane_for_click(mouse.column, mouse.row, size) {
                                app.focus = focus;
                            }
                        }
                    }
                    MouseEventKind::ScrollUp => match app.focus {
                        Focus::Modules => app.move_module_selection(-1),
                        Focus::Tasks   => app.move_task_selection(-1),
                        Focus::Output  => app.scroll_output(3),
                    },
                    MouseEventKind::ScrollDown => match app.focus {
                        Focus::Modules => app.move_module_selection(1),
                        Focus::Tasks   => app.move_task_selection(1),
                        Focus::Output  => app.scroll_output(-3),
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
                        KeyCode::Char('j') | KeyCode::Down  => app.scroll_output(-1),
                        KeyCode::Char('k') | KeyCode::Up    => app.scroll_output(1),
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
                    KeyCode::Char('j') | KeyCode::Down => match app.focus {
                        Focus::Modules => app.move_module_selection(1),
                        Focus::Tasks | Focus::Output => app.move_task_selection(1),
                    },
                    KeyCode::Char('k') | KeyCode::Up => match app.focus {
                        Focus::Modules => app.move_module_selection(-1),
                        Focus::Tasks | Focus::Output => app.move_task_selection(-1),
                    },

                    // Module actions.
                    KeyCode::Char(' ') => {
                        if app.focus == Focus::Modules {
                            app.toggle_multi_select();
                        }
                    }
                    KeyCode::Char('c') => app.multi_select.clear(),
                    KeyCode::Enter => {
                        if app.focus == Focus::Output {
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

    // Outer split: left (modules) | right (output + tasks)
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(area);

    // Right split: output (top) | tasks (bottom)
    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
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
j/k ↑/↓   Navigate modules / tasks
g / G      Jump to first / last
Space      Multi-select module
c          Clear selection
Enter      Fullscreen output (Esc to exit)
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

    let width = 48u16;
    let height = 19u16;
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
///
/// Mirrors the layout constraints in `draw`:
///   horizontal: Modules 25% | right 75%
///   vertical (right side): Output 65% | Tasks 35%
fn pane_for_click(col: u16, row: u16, size: ratatui::layout::Size) -> Option<Focus> {
    if size.width == 0 || size.height == 0 {
        return None;
    }
    let modules_right_edge = size.width * 25 / 100;
    if col < modules_right_edge {
        Some(Focus::Modules)
    } else {
        let output_bottom_edge = size.height * 65 / 100;
        if row < output_bottom_edge {
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
