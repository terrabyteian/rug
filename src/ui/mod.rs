pub mod keybar;
pub mod layout;
pub mod output;
pub mod run;
pub mod select;
pub mod theme;
pub mod widgets;

/// Outcome of a screen-level key handler that the event loop must act on.
pub enum ScreenAction {
    /// Nothing further for the loop to do.
    None,
    /// The user asked to quit (respect running tasks like the `q` binding).
    Quit,
    /// Enter fullscreen output (loop turns off mouse capture).
    EnterFullscreen,
}

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

use crate::app::{
    App, ConfirmKind, ExplorerOpKind, Modal, OpResult, PendingConfirm, PendingOp, Screen,
};

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

        terminal.draw(|f| draw(f, app))?;

        // Poll for input events with a short timeout so we keep draining task events.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Mouse(mouse) => {
                    // Hard overlays absorb all mouse events. Help does not.
                    if app.filter_active
                        || app.modal.as_ref().is_some_and(|m| !matches!(m, Modal::Help))
                    {
                        continue;
                    }

                    // State explorer: only pass scroll events through.
                    if app.state_explorer.is_some() {
                        let in_detail = app
                            .state_explorer
                            .as_ref()
                            .map(|e| e.detail_view.is_some())
                            .unwrap_or(false);
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                if in_detail {
                                    app.resource_detail_scroll(-3);
                                } else {
                                    app.state_explorer_move(-1);
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if in_detail {
                                    app.resource_detail_scroll(3);
                                } else {
                                    app.state_explorer_move(1);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Fullscreen output (Run session): wheel scrolls, nothing else.
                    if app.session.as_ref().map(|s| s.fullscreen).unwrap_or(false) {
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                app.run_scroll_output(1);
                            }
                            MouseEventKind::ScrollDown => {
                                app.run_scroll_output(-1);
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Screen-specific: wheel moves the cursor / scrolls output;
                    // a left click sets the cursor on the row it lands on.
                    match app.screen {
                        Screen::Select => match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                app.move_module_selection(-1);
                            }
                            MouseEventKind::ScrollDown => {
                                app.move_module_selection(1);
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                let top = app.viewport.list_top;
                                let h = app.viewport.list;
                                if mouse.row >= top && mouse.row < top.saturating_add(h) {
                                    let pos = app.viewport.list_offset as usize
                                        + (mouse.row - top) as usize;
                                    app.set_module_cursor(pos);
                                }
                            }
                            _ => {}
                        },
                        Screen::Run => {
                            let board_top = app.viewport.board_top;
                            let board_h = app.viewport.board;
                            let output_top = app.viewport.output_top;
                            let in_board = mouse.row >= board_top
                                && mouse.row < board_top.saturating_add(board_h);
                            let in_output = mouse.row >= output_top;
                            match mouse.kind {
                                MouseEventKind::ScrollUp => {
                                    if in_output {
                                        app.run_scroll_output(3);
                                    } else {
                                        app.run_move_cursor(-1);
                                    }
                                }
                                MouseEventKind::ScrollDown => {
                                    if in_output {
                                        app.run_scroll_output(-3);
                                    } else {
                                        app.run_move_cursor(1);
                                    }
                                }
                                MouseEventKind::Down(MouseButton::Left) => {
                                    if in_board {
                                        let pos = app.viewport.board_offset as usize
                                            + (mouse.row - board_top) as usize;
                                        app.run_set_cursor(pos);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::Key(key) => {
                    // Ctrl-C: quit immediately if no tasks are running, otherwise
                    // surface the same confirmation overlay as `q`. A second Ctrl-C
                    // while the overlay is up forces the quit through.
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        if matches!(app.modal, Some(Modal::Quit)) || app.active_tasks().is_empty()
                        {
                            break;
                        }
                        app.modal = Some(Modal::Quit);
                        continue;
                    }

                    // Min-size guard: below 40×10 the UI can't render, so swallow
                    // everything except quit (Ctrl-C handled above).
                    if terminal
                        .size()
                        .map(|s| s.width < layout::MIN_W || s.height < layout::MIN_H)
                        .unwrap_or(false)
                    {
                        if key.code == KeyCode::Char('q') {
                            if app.active_tasks().is_empty() {
                                break;
                            }
                            app.modal = Some(Modal::Quit);
                        }
                        continue;
                    }

                    // Fullscreen output mode (Run session): Esc exits, scroll keys
                    // pass through, all else swallowed.
                    if app.session.as_ref().map(|s| s.fullscreen).unwrap_or(false) {
                        match key.code {
                            KeyCode::Esc => {
                                if let Some(s) = app.session.as_mut() {
                                    s.fullscreen = false;
                                }
                                execute!(io::stdout(), EnableMouseCapture)?;
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.run_scroll_output(-1);
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.run_scroll_output(1);
                            }
                            KeyCode::PageDown => {
                                app.run_scroll_output(-(app.viewport.output.max(1) as i32));
                            }
                            KeyCode::PageUp => {
                                app.run_scroll_output(app.viewport.output.max(1) as i32);
                            }
                            KeyCode::Char('g') => {
                                let len = app.run_output_lines().len() as i32;
                                app.run_scroll_output(len);
                            }
                            KeyCode::Char('G') => {
                                app.run_scroll_output(-(app.run_output_lines().len() as i32));
                            }
                            KeyCode::Char('w') => {
                                if let Some(s) = app.session.as_mut() {
                                    s.output_wrap = !s.output_wrap;
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Modal overlay intercepts all keys: Quit (q force-quits, Esc
                    // cancels); Confirm/CancelTasks/ClearTasks/Reset (y/Y/Enter act,
                    // anything else dismisses); Help (any key closes).
                    if app.modal.is_some() {
                        if matches!(app.modal, Some(Modal::Quit)) {
                            match key.code {
                                KeyCode::Char('q') => break,
                                KeyCode::Esc => app.modal = None,
                                _ => {}
                            }
                        } else if matches!(app.modal, Some(Modal::Confirm(_))) {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                    app.confirm_execute();
                                }
                                _ => {
                                    app.cancel_confirm();
                                }
                            }
                        } else if matches!(app.modal, Some(Modal::CancelTasks(_))) {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                    app.cancel_staged_tasks();
                                }
                                _ => {
                                    app.modal = None;
                                }
                            }
                        } else if matches!(app.modal, Some(Modal::ClearTasks)) {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                    app.clear_completed_tasks();
                                }
                                _ => {
                                    app.modal = None;
                                }
                            }
                        } else if matches!(app.modal, Some(Modal::Reset)) {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                    app.reset_session();
                                }
                                _ => {
                                    app.modal = None;
                                }
                            }
                        } else {
                            // Help: any key closes.
                            app.modal = None;
                        }
                        continue;
                    }

                    // State explorer overlay.
                    if app.state_explorer.is_some() {
                        let has_op_result = app
                            .state_explorer
                            .as_ref()
                            .map(|e| e.op_result.is_some())
                            .unwrap_or(false);
                        let op_confirm = app.state_explorer.as_ref().and_then(|e| e.op_confirm);
                        let has_pending_op = app
                            .state_explorer
                            .as_ref()
                            .map(|e| e.pending_op.is_some())
                            .unwrap_or(false);
                        let in_detail = app
                            .state_explorer
                            .as_ref()
                            .map(|e| e.detail_view.is_some())
                            .unwrap_or(false);
                        let filter_active = app
                            .state_explorer
                            .as_ref()
                            .map(|e| e.filter_active)
                            .unwrap_or(false);

                        let plan_queued = app
                            .state_explorer
                            .as_ref()
                            .map(|e| e.plan_queued_notice)
                            .unwrap_or(false);

                        if has_op_result {
                            // Any key dismisses the result overlay.
                            app.dismiss_op_result();
                        } else if plan_queued {
                            if let Some(e) = app.state_explorer.as_mut() {
                                e.plan_queued_notice = false;
                            }
                        } else if op_confirm.is_some() {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    app.start_op();
                                }
                                _ => {
                                    app.cancel_op_confirm();
                                }
                            }
                        } else if has_pending_op {
                            // Absorb all keys while the op is running.
                        } else if in_detail {
                            match key.code {
                                KeyCode::Esc => {
                                    app.close_resource_detail();
                                }
                                KeyCode::Char('q') => {
                                    app.close_state_explorer();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    app.resource_detail_scroll(1);
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.resource_detail_scroll(-1);
                                }
                                KeyCode::PageDown => {
                                    app.resource_detail_scroll(
                                        app.viewport.explorer.max(1) as i32
                                    );
                                }
                                KeyCode::PageUp => {
                                    app.resource_detail_scroll(
                                        -(app.viewport.explorer.max(1) as i32),
                                    );
                                }
                                KeyCode::Char('g') => {
                                    app.resource_detail_go_first();
                                }
                                KeyCode::Char('G') => {
                                    app.resource_detail_go_last();
                                }
                                _ => {}
                            }
                        } else if filter_active {
                            match key.code {
                                KeyCode::Esc => {
                                    app.state_explorer_clear_filter();
                                }
                                KeyCode::Enter => {
                                    app.state_explorer_deactivate_filter();
                                }
                                KeyCode::Backspace => {
                                    app.state_explorer_filter_pop();
                                }
                                KeyCode::Char(c) => {
                                    app.state_explorer_filter_push(c);
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    app.close_state_explorer();
                                }
                                KeyCode::Enter => {
                                    app.open_resource_detail();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    app.state_explorer_move(1);
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.state_explorer_move(-1);
                                }
                                KeyCode::PageDown => {
                                    app.state_explorer_move(app.viewport.explorer.max(1) as i32);
                                }
                                KeyCode::PageUp => {
                                    app.state_explorer_move(
                                        -(app.viewport.explorer.max(1) as i32),
                                    );
                                }
                                KeyCode::Char('g') => {
                                    app.state_explorer_go_first();
                                }
                                KeyCode::Char('G') => {
                                    app.state_explorer_go_last();
                                }
                                KeyCode::Char('/') => {
                                    app.state_explorer_activate_filter();
                                }
                                KeyCode::Char(' ') => {
                                    app.state_explorer_toggle_select();
                                }
                                KeyCode::Char('c') => {
                                    app.state_explorer_clear_select();
                                }
                                KeyCode::Char('t') => {
                                    app.request_op_confirm(ExplorerOpKind::Taint);
                                }
                                KeyCode::Char('D') => {
                                    app.request_op_confirm(ExplorerOpKind::StateRm);
                                }
                                KeyCode::Char('p') => {
                                    app.enqueue_targeted_plan();
                                }
                                KeyCode::Char('a') => {
                                    app.request_op_confirm(ExplorerOpKind::TargetedApply);
                                }
                                KeyCode::Char('d') => {
                                    app.request_op_confirm(ExplorerOpKind::TargetedDestroy);
                                }
                                KeyCode::Char('r') => {
                                    app.refresh_state_explorer();
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }

                    let action = match app.screen {
                        Screen::Select => select::handle_key(app, key),
                        Screen::Run => run::handle_key(app, key),
                    };
                    match action {
                        ScreenAction::Quit => {
                            if app.active_tasks().is_empty() {
                                break;
                            }
                            app.modal = Some(Modal::Quit);
                        }
                        ScreenAction::EnterFullscreen => {
                            execute!(io::stdout(), DisableMouseCapture)?;
                        }
                        ScreenAction::None => {}
                    }
                } // end Event::Key
                _ => {}
            } // end match event::read()
        }
    }
    Ok(())
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();

    // Min-size guard runs before any other draw path (prevents panics in the
    // legacy split math at degenerate sizes).
    if layout::too_small(area) {
        render_too_small(f, area);
        return;
    }

    // Fullscreen output (Run session) takes over the full window.
    if app.session.as_ref().map(|s| s.fullscreen).unwrap_or(false) {
        run::render_fullscreen_output(f, area, app);
        return;
    }

    // State explorer (list or detail) takes over the full window.
    if app.state_explorer.is_some() {
        // Reserve: 2 border + 1 blank top + 1 blank before hint + 1 hint + 1 counter + 1 filter bar.
        app.viewport.explorer = area.height.saturating_sub(7);
        let spinner =
            theme::SPINNER_FRAMES[(app.spinner_tick as usize / 2) % theme::SPINNER_FRAMES.len()];
        if let Some(explorer) = &app.state_explorer {
            render_state_explorer(f, area, explorer, spinner);
            // Op overlays render on top of the state explorer.
            if let Some(op_kind) = explorer.op_confirm {
                render_op_confirm(f, area, op_kind, &explorer.op_targets);
            } else if let Some(pt) = &explorer.pending_op {
                render_op_progress(f, area, pt);
            } else if let Some(result) = &explorer.op_result {
                render_op_result(f, area, result);
            } else if explorer.plan_queued_notice {
                render_plan_queued_notice(f, area);
            }
        }
        return;
    }

    match app.screen {
        Screen::Select => select::render(f, area, app),
        Screen::Run => run::render(f, area, app),
    }

    match &app.modal {
        Some(Modal::Quit) => render_quit_wait(f, area, app),
        Some(Modal::Help) => render_help(f, area, app.screen),
        Some(Modal::Confirm(confirm)) => render_confirm(f, area, confirm),
        Some(Modal::CancelTasks(ids)) => {
            let tasks: Vec<(&str, &str)> = ids
                .iter()
                .filter_map(|&id| app.engine.task(id))
                .map(|t| (t.module_name.as_str(), t.command.as_str()))
                .collect();
            if !tasks.is_empty() {
                render_cancel_task_confirm(f, area, &tasks);
            }
        }
        Some(Modal::ClearTasks) => {
            let completed = app.completed_task_count();
            if completed > 0 {
                render_clear_tasks_confirm(
                    f,
                    area,
                    completed,
                    app.engine.tasks.len().saturating_sub(completed),
                );
            }
        }
        Some(Modal::Reset) => {
            let (plans, queued, finished) = app.reset_summary();
            render_reset_confirm(f, area, plans, queued, finished);
        }
        None => {}
    }
}

/// Render the minimum-size guard screen. No app state is mutated here.
fn render_too_small(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    use ratatui::{
        layout::{Alignment, Rect},
        text::{Line, Span},
        widgets::{Clear, Paragraph},
    };

    let lines = vec![
        Line::from(Span::styled("terminal too small", theme::title())),
        Line::from(Span::styled(
            format!(
                "need ≥ {}×{} (now {}×{})",
                layout::MIN_W,
                layout::MIN_H,
                area.width,
                area.height
            ),
            theme::dim(),
        )),
        Line::from(Span::styled("resize, or q to quit", theme::dim())),
    ];

    let h = 3u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let r = Rect::new(area.x, y, area.width, h.min(area.height.max(1)));

    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), r);
}

/// Key-hint entries for the Select screen help table.
const SELECT_HELP: &[(&str, &str)] = &[
    ("j/k ↑/↓", "move cursor"),
    ("PgUp/PgDn", "page up / down"),
    ("g / G", "first / last"),
    ("Space", "toggle select"),
    ("Ctrl+Space", "range select"),
    ("* / c", "all / clear"),
    ("/ Esc", "filter / clear"),
    ("[ / ]", "depth − / +"),
    ("Enter", "run selection"),
    ("Tab", "resume session"),
    ("i / u", "init / upgrade"),
    ("p / a", "plan / apply"),
    ("d / U", "destroy / unlock"),
    ("s", "state explorer"),
    ("r / R", "refresh / reset"),
    ("? / q", "help / quit"),
];

/// Key-hint entries for the Run screen help table.
const RUN_HELP: &[(&str, &str)] = &[
    ("j/k ↑/↓", "move cursor"),
    ("g / G", "first / last"),
    ("PgUp/PgDn", "scroll output"),
    ("Space", "toggle subset"),
    ("Ctrl+Space", "range select"),
    ("* / c", "all / clear subset"),
    ("i / p", "init / plan"),
    ("a / d", "apply / destroy"),
    ("u / U", "upgrade / unlock"),
    ("I/P/A/D", "same, this row"),
    ("C / x", "cancel / clear tasks"),
    ("Enter", "fullscreen output"),
    ("w", "wrap output"),
    ("s", "state explorer"),
    ("Esc", "back (tasks run on)"),
    ("? / q", "help / quit"),
];

fn render_help(f: &mut ratatui::Frame, area: ratatui::layout::Rect, screen: Screen) {
    use ratatui::{
        layout::Alignment,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let (entries, title): (&[(&str, &str)], &str) = match screen {
        Screen::Select => (SELECT_HELP, " Help — Select (any key to close) "),
        Screen::Run => (RUN_HELP, " Help — Run (any key to close) "),
    };

    let key_style = Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD);
    let chars = |s: &str| s.chars().count();

    // Two columns at W1–W2 (≥80), single column at W3– (<80).
    let two_col = area.width >= 80;

    let (rows, content_w): (Vec<Line>, usize) = if two_col {
        let mid = entries.len().div_ceil(2);
        let (left, right) = entries.split_at(mid);
        let lk = left.iter().map(|(k, _)| chars(k)).max().unwrap_or(0);
        let ld = left.iter().map(|(_, d)| chars(d)).max().unwrap_or(0);
        let rk = right.iter().map(|(k, _)| chars(k)).max().unwrap_or(0);
        let rd = right.iter().map(|(_, d)| chars(d)).max().unwrap_or(0);

        let mut lines = Vec::with_capacity(mid);
        for (i, &(k, d)) in left.iter().enumerate() {
            let mut spans = vec![
                Span::styled(format!("{:<lk$}", k), key_style),
                Span::styled(format!("  {:<ld$}", d), theme::dim()),
            ];
            if let Some(&(k2, d2)) = right.get(i) {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(format!("{:<rk$}", k2), key_style));
                spans.push(Span::styled(format!("  {d2}"), theme::dim()));
            }
            lines.push(Line::from(spans));
        }
        let w = lk + 2 + ld + 3 + rk + 2 + rd;
        (lines, w)
    } else {
        let kw = entries.iter().map(|(k, _)| chars(k)).max().unwrap_or(0);
        let dw = entries.iter().map(|(_, d)| chars(d)).max().unwrap_or(0);
        let lines = entries
            .iter()
            .map(|&(k, d)| {
                Line::from(vec![
                    Span::styled(format!("{:<kw$}", k), key_style),
                    Span::styled(format!("  {d}"), theme::dim()),
                ])
            })
            .collect();
        (lines, kw + 2 + dw)
    };

    let desired_h = (rows.len() + 2) as u16; // + top/bottom border
    let desired_w = (content_w + 4) as u16; // + borders + 1-cell padding each side
    let popup = layout::popup_rect(desired_w, desired_h, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(rows)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_warn()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_confirm(f: &mut ratatui::Frame, area: ratatui::layout::Rect, confirm: &PendingConfirm) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let label = confirm.kind.label();
    let n = confirm.targets.len();
    let noun = if n == 1 { "module" } else { "modules" };

    // Dynamic width: measure the longest name + annotation column.
    let name_width = confirm
        .targets
        .iter()
        .map(|t| t.module_name.len())
        .max()
        .unwrap_or(0);
    let extra_width: usize = match &confirm.kind {
        ConfirmKind::Apply => confirm
            .targets
            .iter()
            .map(|t| match &t.plan_age {
                Some(age) if !t.plan_targets.is_empty() => {
                    format!("  plan from {} · TARGETED ({})", age, t.plan_targets.len()).len()
                }
                Some(age) => format!("  plan from {}", age).len(),
                None => "  no prior plan".len(),
            })
            .max()
            .unwrap_or(0),
        ConfirmKind::ForceUnlock => confirm
            .targets
            .iter()
            .map(|t| {
                if let Some(id) = &t.lock_id {
                    let short_id = if id.len() > 8 {
                        format!("{}…", &id[..8])
                    } else {
                        id.clone()
                    };
                    let who = t.lock_who.as_deref().unwrap_or("?");
                    format!("  lock {}  by {}", short_id, who).len()
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0),
        _ => 0,
    };
    // Union of `-target=` addresses across all Apply targets whose cached plan
    // is targeted (deduped, order-preserving). Drives the warning block below.
    let mut apply_targeted_addrs: Vec<String> = Vec::new();
    if confirm.kind == ConfirmKind::Apply {
        for t in &confirm.targets {
            for addr in &t.plan_targets {
                if !apply_targeted_addrs.contains(addr) {
                    apply_targeted_addrs.push(addr.clone());
                }
            }
        }
    }
    let targeted_warn_w = if apply_targeted_addrs.is_empty() {
        0
    } else {
        apply_targeted_addrs
            .iter()
            .map(|a| format!("      • {}", a).len())
            .max()
            .unwrap_or(0)
            .max("  \u{26a0} Cached plan is TARGETED — apply covers ONLY:".len())
    };

    // 6 = "    • "; min 36 covers footer and ForceUnlock warning line.
    let content_w = (6 + name_width + extra_width)
        .max(format!("  Run {} on {} {}:", label, n, noun).len())
        .max(targeted_warn_w)
        .max(36);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("  Run {label} on {n} {noun}:"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for target in &confirm.targets {
        let name_span = Span::raw(format!(
            "    • {:<width$}",
            target.module_name,
            width = name_width
        ));

        let plan_span = match &confirm.kind {
            ConfirmKind::Apply => match &target.plan_age {
                Some(age) if !target.plan_targets.is_empty() => Span::styled(
                    format!("  plan from {age} · TARGETED ({})", target.plan_targets.len()),
                    Style::default().fg(Color::Yellow),
                ),
                Some(age) => Span::styled(
                    format!("  plan from {age}"),
                    Style::default().fg(Color::Green),
                ),
                None => Span::styled(
                    "  no prior plan".to_string(),
                    Style::default().fg(Color::Yellow),
                ),
            },
            ConfirmKind::Destroy | ConfirmKind::InitUpgrade => Span::raw(""),
            ConfirmKind::ForceUnlock => {
                if let Some(id) = &target.lock_id {
                    let short_id = if id.len() > 8 {
                        format!("{}…", &id[..8])
                    } else {
                        id.clone()
                    };
                    let who = target.lock_who.as_deref().unwrap_or("?");
                    Span::styled(
                        format!("  lock {short_id}  by {who}"),
                        Style::default().fg(Color::Yellow),
                    )
                } else {
                    Span::raw("")
                }
            }
        };

        lines.push(Line::from(vec![name_span, plan_span]));
    }

    if confirm.kind == ConfirmKind::ForceUnlock {
        lines.push(Line::from(Span::styled(
            "  \u{26a0} Force-removes the state lock",
            Style::default().fg(Color::Red),
        )));
    }

    // Targeted-plan warning: at least one cached plan is partial, so the apply
    // will only touch those addresses. Cap the whole block at ~6 lines.
    if !apply_targeted_addrs.is_empty() {
        let warn = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  \u{26a0} Cached plan is TARGETED — apply covers ONLY:",
            warn,
        )));
        // Header + up to 4 bullets + optional "… and N more" = 6 lines.
        let shown = apply_targeted_addrs.len().min(4);
        for addr in &apply_targeted_addrs[..shown] {
            lines.push(Line::from(Span::styled(format!("      • {addr}"), warn)));
        }
        let extra = apply_targeted_addrs.len() - shown;
        if extra > 0 {
            lines.push(Line::from(Span::styled(
                format!("      … and {extra} more"),
                warn,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "[y] Confirm",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("[any] Cancel", theme::dim()),
    ]));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" Confirm {label} "))
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_danger()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_cancel_task_confirm(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    tasks: &[(&str, &str)],
) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let n = tasks.len();
    let noun = if n == 1 { "task" } else { "tasks" };

    // Dynamic width: fit the longest name + command.
    let name_width = tasks.iter().map(|t| t.0.len()).max().unwrap_or(0);
    let cmd_width = tasks.iter().map(|t| t.1.len()).max().unwrap_or(0);
    // 8 = "    • " (6) + "  " gap (2); 41 = footer "  [y] Cancel task(s)   [any] Keep running".
    let content_w = (8 + name_width + cmd_width).max(41);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("  Cancel {n} {noun}:"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for &(module_name, command) in tasks {
        lines.push(Line::from(vec![
            Span::raw(format!(
                "    • {:<width$}  ",
                module_name,
                width = name_width
            )),
            Span::styled(command.to_string(), Style::default().fg(Color::Blue)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "[y] Cancel task(s)",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("[any] Keep running", theme::dim()),
    ]));

    let height = (lines.len() + 2) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" Cancel {n} Task(s) "))
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_warn()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_clear_tasks_confirm(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    completed: usize,
    active: usize,
) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let completed_noun = if completed == 1 { "task" } else { "tasks" };
    let active_noun = if active == 1 { "task" } else { "tasks" };
    let active_line = if active > 0 {
        format!("  Keeps {active} active {active_noun}.")
    } else {
        "  Task list will be empty afterward.".to_string()
    };

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Clear {completed} completed {completed_noun}?"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(active_line, theme::dim())),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "[y] Clear",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled("[any] Keep", theme::dim()),
        ]),
    ];

    let content_w = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.len())
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0)
        .max(28);
    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Clear Completed Tasks ")
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_warn()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_reset_confirm(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    plans: usize,
    queued: usize,
    finished: usize,
) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let plan_noun = if plans == 1 { "plan" } else { "plans" };
    let queued_noun = if queued == 1 { "task" } else { "tasks" };
    let finished_noun = if finished == 1 { "task" } else { "tasks" };

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Reset session?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  • Drop {plans} cached {plan_noun}"),
            theme::dim(),
        )),
        Line::from(Span::styled(
            format!("  • Cancel {queued} queued {queued_noun}"),
            theme::dim(),
        )),
        Line::from(Span::styled(
            format!("  • Drop {finished} finished {finished_noun}"),
            theme::dim(),
        )),
        Line::from(Span::styled(
            "  • Clear filter, selection, depth limit",
            theme::dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("  Running tasks continue.", theme::dim())),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "[y] Reset",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled("[any] Cancel", theme::dim()),
        ]),
    ];

    let content_w = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.len())
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0)
        .max(40);
    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Reset Session ")
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_warn()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_quit_wait(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let active = app.active_tasks();
    let all_done = active.is_empty();

    // Dynamic width: fit the longest name + command.
    let name_width = active
        .iter()
        .map(|t| t.module_name.len())
        .max()
        .unwrap_or(0);
    let cmd_width = active.iter().map(|t| t.command.len()).max().unwrap_or(0);
    // 5 = "  X " icon prefix; 31 = footer width.
    let content_w = (5 + name_width + 1 + cmd_width).max(31);

    let header = if all_done {
        "  All tasks finished — quit?"
    } else {
        "  Tasks still running — quit?"
    };

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            header,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for task in &active {
        lines.push(Line::from(vec![
            Span::raw(format!("  {} ", theme::status_icon(&task.status))),
            Span::styled(
                format!("{:<width$}", task.module_name, width = name_width),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(task.command.clone()),
        ]));
    }

    let (quit_label, quit_color) = if all_done {
        ("[q] Quit", Color::Green)
    } else {
        ("[q] Force quit", Color::Red)
    };

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            quit_label,
            Style::default().fg(quit_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            "[Esc] Cancel",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let title = if all_done {
        " Quit? ".to_string()
    } else {
        format!(" Quitting — {} task(s) remaining ", active.len())
    };

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_warn()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_state_explorer(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    explorer: &crate::app::StateExplorer,
    spinner: &str,
) {
    // If the user has drilled into a resource, show that view instead.
    if let Some(detail) = &explorer.detail_view {
        render_resource_detail(f, area, &explorer.module_name, detail);
        return;
    }

    use crate::state::StateContent;
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
    };

    // Reserve rows: 2 border + 1 blank top + 1 blank before hint + 1 hint + 1 counter (worst case).
    let max_resource_rows = area.height.saturating_sub(7).max(3) as usize;

    let mut lines: Vec<Line> = vec![Line::from("")];

    match &explorer.content {
        StateContent::Loading => {
            lines.push(Line::from(Span::styled(
                format!("  {spinner} Loading state…"),
                theme::dim(),
            )));
        }
        StateContent::Error(msg) => {
            lines.push(Line::from(Span::styled(
                "  Failed to load state",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(format!("  {msg}"), theme::dim())));
        }
        StateContent::NotInitialized => {
            lines.push(Line::from(Span::styled(
                "  Not initialized",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  Run `i` to initialize this module.",
                theme::dim(),
            )));
        }
        StateContent::NoState => {
            lines.push(Line::from(Span::styled(
                "  No state",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  Module is initialized but has no resources in state.",
                theme::dim(),
            )));
        }
        StateContent::Resources(resources) => {
            use crate::app::ExplorerRow;
            let filter_lower = explorer.filter.to_lowercase();
            let rows = explorer.rows();

            let total = resources.len();
            let visible_count = rows.len();
            // The "N / M" counter counts resource rows only (headers excluded).
            let visible_resource_count = rows
                .iter()
                .filter(|r| matches!(r, ExplorerRow::Resource { .. }))
                .count();

            if rows.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  No resources match \"{}\"", explorer.filter),
                    theme::dim(),
                )));
            } else {
                // Scroll window: keep selected visible at bottom of window.
                let scroll_start = explorer
                    .selected
                    .saturating_sub(max_resource_rows.saturating_sub(1))
                    .min(visible_count.saturating_sub(max_resource_rows));

                if scroll_start > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("  ↑ {} more above", scroll_start),
                        theme::dim(),
                    )));
                }

                for (vi, row) in rows
                    .iter()
                    .enumerate()
                    .skip(scroll_start)
                    .take(max_resource_rows)
                {
                    let is_selected = vi == explorer.selected;
                    match row {
                        ExplorerRow::ModuleHeader { prefix, count } => {
                            let marked = explorer.module_select.contains(prefix);
                            let marker = if marked { "●" } else { " " };
                            if is_selected {
                                lines.push(Line::from(Span::styled(
                                    format!(" {} ▸ {} ({})", marker, prefix, count),
                                    Style::default()
                                        .fg(Color::Black)
                                        .bg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD),
                                )));
                            } else {
                                let marker_style = if marked {
                                    theme::multi_select_marker()
                                } else {
                                    theme::title()
                                };
                                lines.push(Line::from(vec![
                                    Span::styled(format!(" {} ", marker), marker_style),
                                    Span::styled(
                                        format!("▸ {} ({})", prefix, count),
                                        theme::title(),
                                    ),
                                ]));
                            }
                        }
                        ExplorerRow::Resource { res_idx, indent } => {
                            let resource = &resources[*res_idx];
                            let indent_str = if *indent { "  " } else { "" };
                            let is_multi = explorer.multi_select.contains(res_idx);
                            let covered = !is_multi
                                && explorer
                                    .module_select
                                    .iter()
                                    .any(|p| crate::state::is_covered_by(&resource.address, p));
                            let tainted = resource.is_tainted();

                            if is_selected {
                                let marker = if is_multi || covered { "●" } else { " " };
                                let taint_tag = if tainted { " [tainted]" } else { "" };
                                let full = format!(
                                    " {} {}{}{}",
                                    marker, indent_str, resource.address, taint_tag
                                );
                                lines.push(Line::from(Span::styled(
                                    full,
                                    Style::default()
                                        .fg(Color::Black)
                                        .bg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD),
                                )));
                            } else if is_multi || covered {
                                let marker_style = if is_multi {
                                    theme::multi_select_marker()
                                } else {
                                    theme::covered_marker()
                                };
                                let mut spans = vec![
                                    Span::styled(" ● ".to_string(), marker_style),
                                    Span::raw(format!("{}{}", indent_str, resource.address)),
                                ];
                                if tainted {
                                    spans.push(Span::styled(
                                        " [tainted]".to_string(),
                                        Style::default().fg(Color::Red),
                                    ));
                                }
                                lines.push(Line::from(spans));
                            } else if !explorer.filter.is_empty() {
                                let addr = format!("   {}{}", indent_str, resource.address);
                                let mut line = highlight_filter_match(addr, &filter_lower);
                                if tainted {
                                    line.spans.push(Span::styled(
                                        " [tainted]".to_string(),
                                        Style::default().fg(Color::Red),
                                    ));
                                }
                                lines.push(line);
                            } else {
                                let mut spans = vec![Span::raw(format!(
                                    "   {}{}",
                                    indent_str, resource.address
                                ))];
                                if tainted {
                                    spans.push(Span::styled(
                                        " [tainted]".to_string(),
                                        Style::default().fg(Color::Red),
                                    ));
                                }
                                lines.push(Line::from(spans));
                            }
                        }
                    }
                }

                let shown_end = scroll_start + max_resource_rows;
                if shown_end < visible_count {
                    lines.push(Line::from(Span::styled(
                        format!("  ↓ {} more below", visible_count - shown_end),
                        theme::dim(),
                    )));
                }
            }

            // Counter: "12 / 48" when filtering, "48" when not.
            lines.push(Line::from(""));
            if !explorer.filter.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {} / {} resources", visible_resource_count, total),
                    theme::dim(),
                )));
            }
        }
    }

    // Filter bar (always present for resources; shows hint or active input).
    let filter_line = if explorer.filter_active {
        Line::from(vec![
            Span::styled(
                "  /",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(explorer.filter.clone(), Style::default().fg(Color::White)),
            Span::styled("█", Style::default().fg(Color::Cyan)),
        ])
    } else if !explorer.filter.is_empty() {
        Line::from(vec![
            Span::styled("  /", theme::dim()),
            Span::styled(
                explorer.filter.clone(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled("  (Esc to clear)", theme::dim()),
        ])
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled("[j/k] Nav", theme::dim()),
            Span::raw("  "),
            Span::styled("[Space] Select", theme::dim()),
            Span::raw("  "),
            Span::styled("[t] Taint", theme::dim()),
            Span::raw("  "),
            Span::styled("[D] Rm state", theme::dim()),
            Span::raw("  "),
            Span::styled("[p] Plan", theme::dim()),
            Span::raw("  "),
            Span::styled("[a] Apply", theme::dim()),
            Span::raw("  "),
            Span::styled("[d] Destroy", theme::dim()),
            Span::raw("  "),
            Span::styled("[Enter] Inspect", theme::dim()),
            Span::raw("  "),
            Span::styled("[/] Filter", theme::dim()),
            Span::raw("  "),
            Span::styled("[r] Refresh", theme::dim()),
            Span::raw("  "),
            Span::styled("[Esc] Close", theme::dim()),
        ])
    };
    lines.push(Line::from(""));
    lines.push(filter_line);

    // Title: show resource count and selection count.
    let title_suffix = if let StateContent::Resources(r) = &explorer.content {
        let n = r.len();
        let sel = explorer.multi_select.len() + explorer.module_select.len();
        if sel > 0 {
            format!(
                " — {} resource{} ({} selected)",
                n,
                if n == 1 { "" } else { "s" },
                sel
            )
        } else {
            format!(" — {} resource{}", n, if n == 1 { "" } else { "s" })
        }
    } else {
        String::new()
    };

    let border_style = if explorer.filter_active {
        theme::overlay_border_filter()
    } else {
        theme::overlay_border_explorer()
    };

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" State: {}{} ", explorer.module_name, title_suffix))
                    .borders(Borders::ALL)
                    .border_style(border_style),
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
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    // Separator spanning the full inner width.
    lines.push(Line::from(Span::styled(
        "  ".to_string() + &"─".repeat(sep_width),
        theme::dim(),
    )));

    // Scroll-up indicator.
    if scroll > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ↑ {} lines above", scroll),
            theme::dim(),
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
            theme::dim(),
        )));
    }

    // Footer hint.
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("[j/k] Scroll", theme::dim()),
        Span::raw("   "),
        Span::styled("[Esc] Back to list", theme::dim()),
    ]));

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" State: {} ", module_name))
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_explorer()),
            )
            .alignment(Alignment::Left),
        area,
    );
}

/// Syntax-highlight a single line from `serde_json::to_string_pretty` output.
fn style_json_line(line: &str) -> ratatui::text::Line<'static> {
    use ratatui::{
        style::{Color, Style},
        text::{Line, Span},
    };

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
            if let Some(rest) = after_key.strip_prefix(':') {
                let key = trimmed[..key_end].to_string();
                let value_raw = rest.trim_start();
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
    use ratatui::{
        style::{Color, Style},
        text::Span,
    };
    match v {
        "true" | "false" => Span::styled(v.to_string(), Style::default().fg(Color::Magenta)),
        "null" => Span::styled(v.to_string(), Style::default().fg(Color::DarkGray)),
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
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
            } // escaped character — skip both bytes
            b'"' => {
                return Some(i + 1);
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

fn render_plan_queued_notice(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    use ratatui::{
        layout::Alignment,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Targeted plan queued.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  Tab to the task list to view output.",
            theme::dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("  [any key] Dismiss", theme::dim())),
    ];

    let height = (lines.len() + 2) as u16;
    let width = 46u16.min(area.width);
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Plan Queued ")
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_success()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_op_confirm(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    kind: ExplorerOpKind,
    targets: &[String],
) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let n = targets.len();
    let noun = if n == 1 { "resource" } else { "resources" };

    // Dynamic width: fit the longest resource address.
    let addr_width = targets.iter().map(|a| a.len()).max().unwrap_or(0);
    // 6 = "    • "; 46/55 = targeted destroy/apply warning; 28 = footer.
    let content_w = (6 + addr_width)
        .max(format!("  {} {} {}:", kind.confirm_verb(), n, noun).len())
        .max(match kind {
            ExplorerOpKind::TargetedDestroy => 46,
            ExplorerOpKind::TargetedApply => 55,
            _ => 28,
        });

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
    if kind == ExplorerOpKind::TargetedDestroy {
        lines.push(Line::from(Span::styled(
            "  ⚠  This will DESTROY real infrastructure.",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    } else if kind == ExplorerOpKind::TargetedApply {
        lines.push(Line::from(Span::styled(
            "  ⚠  This will APPLY changes to real infrastructure.",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "[y] Confirm",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("[any] Cancel", theme::dim()),
    ]));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(kind.confirm_title())
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_danger()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_op_progress(f: &mut ratatui::Frame, area: ratatui::layout::Rect, pt: &PendingOp) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    // Dynamic width: fit the longest address across done/running/queued.
    let addr_width = pt
        .done
        .iter()
        .map(|(a, _)| a.len())
        .chain(pt.running.iter().map(|(_, a)| a.len()))
        .chain(pt.queue.iter().map(|a| a.len()))
        .max()
        .unwrap_or(0);
    // 4 = "  ✓ " prefix.
    let content_w = (4 + addr_width).max(20);

    let mut lines: Vec<Line> = vec![Line::from(""), Line::from("")];
    for (addr, success) in &pt.done {
        let (icon, color) = if *success {
            ("✓", Color::Green)
        } else {
            ("✗", Color::Red)
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} ", icon),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
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
            Span::styled("· ", theme::dim()),
            Span::styled(addr.clone(), theme::dim()),
        ]));
    }

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(pt.kind.progress_title())
                    .borders(Borders::ALL)
                    .border_style(theme::overlay_border_warn()),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

fn render_op_result(f: &mut ratatui::Frame, area: ratatui::layout::Rect, result: &OpResult) {
    use ratatui::{
        layout::Alignment,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    let all_ok = result.entries.iter().all(|(_, ok)| *ok);
    let border_style = if all_ok {
        theme::overlay_border_success()
    } else {
        theme::overlay_border_danger()
    };

    // Dynamic width: fit the longest address; 20 = "  [any key] Dismiss".
    let addr_width = result
        .entries
        .iter()
        .map(|(a, _)| a.len())
        .max()
        .unwrap_or(0);
    let content_w = (4 + addr_width).max(20);

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (addr, success) in &result.entries {
        let (icon, color) = if *success {
            ("✓", Color::Green)
        } else {
            ("✗", Color::Red)
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} ", icon),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(addr.clone()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  [any key] Dismiss", theme::dim())));

    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let width = (content_w + 2).min(area.width as usize) as u16;
    let popup = layout::popup_rect(width, height, area);

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(result.kind.result_title(all_ok))
                    .borders(Borders::ALL)
                    .border_style(border_style),
            )
            .alignment(Alignment::Left),
        popup,
    );
}

/// Render an address line with the filter match portion highlighted in bold yellow.
/// `text` includes the leading "  " padding; `filter_lower` is already lowercased.
fn highlight_filter_match(text: String, filter_lower: &str) -> ratatui::text::Line<'static> {
    use ratatui::{
        style::{Color, Modifier, Style},
        text::{Line, Span},
    };

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
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(text[end..].to_string()),
            ]);
        }
    }
    Line::from(Span::raw(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{RunSession, SessionModule};
    use crate::config::Config;
    use crate::module::{Module, ModuleKind};
    use crate::task::{Task, TaskStatus};
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use std::time::Instant;

    /// Sizes spanning every width/height tier plus the two responsiveness-pass
    /// checkpoints (45×12, 40×10) and the mockup targets (120×35, 80×24).
    const SIZES: &[(u16, u16)] = &[(120, 35), (80, 24), (60, 18), (45, 12), (40, 10)];

    fn demo_app(n: usize) -> App {
        let root = PathBuf::from("/tmp/rug-ui-test");
        let modules: Vec<Module> = (0..n)
            .map(|i| Module {
                path: root.join(format!("mod{i}")),
                display_name: format!("infra/network/mod{i}"),
                kind: ModuleKind::Root,
            })
            .collect();
        let config = Config {
            binary: "terraform".to_string(),
            parallelism: 2,
            ignore_dirs: Vec::new(),
            show_library_modules: false,
            ..Default::default()
        };
        App::new(config, root, modules).unwrap()
    }

    fn make_session(app: &mut App) {
        let mods: Vec<SessionModule> = app
            .modules
            .iter()
            .enumerate()
            .map(|(i, m)| SessionModule {
                module_idx: i,
                path: m.path.clone(),
                name: m.display_name.clone(),
            })
            .collect();
        app.session = Some(RunSession::new(mods));
        app.screen = Screen::Run;
    }

    fn draw_at(app: &mut App, w: u16, h: u16) {
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
    }

    /// Render and flatten the back buffer to a per-row string for substring
    /// assertions (mockup-fidelity checks).
    fn render_to_string(app: &mut App, w: u16, h: u16) -> String {
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buffer = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn select_mockup_fidelity() {
        let mut app = demo_app(6);
        for &(w, h) in &[(120u16, 30u16), (80, 24)] {
            let s = render_to_string(&mut app, w, h);
            // Header content: app title + binary/module count line.
            assert!(s.contains("rug"), "select header missing title at {w}×{h}");
            assert!(
                s.contains("terraform · 6 modules"),
                "select header missing binary/count at {w}×{h}"
            );
            // A module row is present.
            assert!(
                s.contains("infra/network/mod0"),
                "select missing module row at {w}×{h}"
            );
            // Keybar present.
            assert!(
                s.contains("j/k") && s.contains("move"),
                "select keybar missing at {w}×{h}"
            );
        }
    }

    #[test]
    fn run_mockup_fidelity() {
        let mut app = demo_app(6);
        make_session(&mut app);
        for &(w, h) in &[(120u16, 30u16), (80, 24)] {
            let s = render_to_string(&mut app, w, h);
            // Header content.
            assert!(
                s.contains("Run") && s.contains("6 modules"),
                "run header missing at {w}×{h}"
            );
            // Column header (STATUS shows at every width tier ≥ W4).
            assert!(s.contains("STATUS"), "run col header missing at {w}×{h}");
            // Board row shape: cursor bar + name + idle status glyph.
            assert!(
                s.contains(theme::CURSOR_BAR),
                "run cursor bar missing at {w}×{h}"
            );
            assert!(
                s.contains("infra/network/mod0"),
                "run board row missing at {w}×{h}"
            );
            // Keybar present.
            assert!(s.contains("plan"), "run keybar missing at {w}×{h}");
        }
    }

    /// A targeted task's `·T{n}` chip is drawn on the board row both while the
    /// task is Running and after it finishes (Success).
    #[test]
    fn board_shows_target_chip_running_and_finished() {
        for status in [TaskStatus::Running, TaskStatus::Success] {
            let mut app = demo_app(1);
            make_session(&mut app);
            let path = app.modules[0].path.clone();
            let terminal = status.is_terminal();
            app.engine.tasks.push(Task {
                id: 0,
                module_path: path.clone(),
                module_name: "mod0".to_string(),
                command: "apply".to_string(),
                status,
                output_lines: Vec::new(),
                started_at: Some(Instant::now()),
                finished_at: terminal.then(Instant::now),
                plan_output_path: None,
                targets: vec!["null_resource.a".to_string(), "null_resource.b".to_string()],
                cleanup_plan_path: None,
                resource_counts: None,
                cancel_handle: None,
            });
            app.session.as_mut().unwrap().latest_task.insert(path, 0);

            let s = render_to_string(&mut app, 120, 30);
            assert!(
                s.contains("apply"),
                "command missing (terminal={terminal})"
            );
            assert!(
                s.contains("·T2"),
                "target chip missing (terminal={terminal})"
            );
        }
    }

    #[test]
    fn help_is_screen_aware() {
        // Select help mentions Select-only keys (filter/depth); Run help
        // mentions Run-only keys (subset/back) and not depth.
        let mut app = demo_app(3);
        app.modal = Some(Modal::Help);
        let select_help = render_to_string(&mut app, 120, 30);
        assert!(
            select_help.contains("filter"),
            "select help should list filter"
        );

        let mut app = demo_app(3);
        make_session(&mut app);
        app.modal = Some(Modal::Help);
        let run_help = render_to_string(&mut app, 120, 30);
        assert!(
            run_help.contains("back"),
            "run help should list esc back"
        );
    }

    #[test]
    fn select_renders_at_all_sizes() {
        let mut app = demo_app(6);
        for &(w, h) in SIZES {
            draw_at(&mut app, w, h);
        }
    }

    #[test]
    fn run_board_and_output_render_at_all_sizes() {
        let mut app = demo_app(6);
        make_session(&mut app);
        for &(w, h) in SIZES {
            draw_at(&mut app, w, h);
        }
    }

    #[test]
    fn run_fullscreen_renders_at_all_sizes() {
        let mut app = demo_app(4);
        make_session(&mut app);
        if let Some(s) = app.session.as_mut() {
            s.fullscreen = true;
        }
        for &(w, h) in SIZES {
            draw_at(&mut app, w, h);
        }
    }

    #[test]
    fn help_modal_renders_one_and_two_column() {
        let mut app = demo_app(3);
        app.modal = Some(Modal::Help);
        // 120/80 → two-column; 60/45/40 → single-column. None may panic.
        for &(w, h) in SIZES {
            draw_at(&mut app, w, h);
        }
    }

    #[test]
    fn too_small_guard_never_panics() {
        let mut app = demo_app(3);
        for &(w, h) in &[(38u16, 9u16), (20, 8), (2, 2), (1, 1)] {
            draw_at(&mut app, w, h);
        }
    }

    /// App with an open state explorer holding a `module.net` group (2 members)
    /// and a root resource.
    fn demo_explorer_app() -> App {
        use crate::state::{StateContent, StateResource};
        let mut app = demo_app(1);
        let resources = ["aws_vpc.main", "module.net.null_resource.a", "module.net.null_resource.b"]
            .iter()
            .map(|a| StateResource {
                address: a.to_string(),
                instance: serde_json::json!({}),
            })
            .collect();
        app.state_explorer = Some(crate::app::StateExplorer {
            module_idx: 0,
            module_name: "mod0".to_string(),
            content: StateContent::Resources(resources),
            selected: 0,
            filter: String::new(),
            filter_active: false,
            detail_view: None,
            multi_select: Vec::new(),
            module_select: Vec::new(),
            op_confirm: None,
            op_targets: Vec::new(),
            pending_op: None,
            op_result: None,
            plan_queued_notice: false,
            load_rx: None,
        });
        app
    }

    #[test]
    fn explorer_renders_module_header_and_indented_member() {
        let mut app = demo_explorer_app();
        for &(w, h) in &[(120u16, 35u16), (80, 24)] {
            let s = render_to_string(&mut app, w, h);
            assert!(
                s.contains("▸ module.net (2)"),
                "explorer header missing at {w}×{h}"
            );
            // Indented member row: 2-space group indent before the address.
            assert!(
                s.contains("  module.net.null_resource.a"),
                "indented member missing at {w}×{h}"
            );
        }
    }

    #[test]
    fn explorer_renders_at_all_sizes() {
        let mut app = demo_explorer_app();
        for &(w, h) in SIZES {
            draw_at(&mut app, w, h);
        }
    }

    #[test]
    fn explorer_targeted_apply_confirm_renders() {
        let mut app = demo_explorer_app();
        {
            let ex = app.state_explorer.as_mut().unwrap();
            ex.op_confirm = Some(ExplorerOpKind::TargetedApply);
            ex.op_targets = vec![
                "module.net".to_string(),
                "null_resource.standalone".to_string(),
            ];
        }
        for &(w, h) in &[(120u16, 35u16), (80, 24)] {
            let s = render_to_string(&mut app, w, h);
            assert!(
                s.contains("Targeted Apply"),
                "confirm title missing at {w}×{h}"
            );
            assert!(
                s.contains("This will APPLY changes to real infrastructure."),
                "apply danger warning missing at {w}×{h}"
            );
            assert!(
                s.contains("module.net"),
                "module.net target missing at {w}×{h}"
            );
            assert!(
                s.contains("null_resource.standalone"),
                "null_resource.standalone target missing at {w}×{h}"
            );
        }
        // Panic safety across every tier, including sizes too small to fit
        // the popup content comfortably.
        for &(w, h) in SIZES {
            draw_at(&mut app, w, h);
        }
    }

    #[test]
    fn run_board_shows_targeted_plan_badge() {
        let mut app = demo_app(3);
        make_session(&mut app);
        let module_path = app.modules[0].path.clone();
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);
        app.engine
            .plan_cache
            .register(module_path, plan_path, 1, vec!["module.net".to_string()]);

        // The `P:{age}·T{n}` badge is a wide-tier-only extra (`show_extras`).
        let s = render_to_string(&mut app, 120, 35);
        assert!(
            s.contains("\u{b7}T1"),
            "run board missing targeted plan badge"
        );
    }

    #[test]
    fn select_list_shows_targeted_plan_badge() {
        let mut app = demo_app(3);
        let module_path = app.modules[0].path.clone();
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);
        app.engine
            .plan_cache
            .register(module_path, plan_path, 1, vec!["module.net".to_string()]);

        // The `P:{age}·T{n}` badge only renders once the list is wide enough
        // (`show_age`, width ≥ 80); use 120×35 to be safely inside that tier.
        let s = render_to_string(&mut app, 120, 35);
        assert!(
            s.contains("\u{b7}T1"),
            "select list missing targeted plan badge"
        );
    }

    #[test]
    fn apply_confirm_shows_targeted_warning() {
        let mut app = demo_app(3);
        let module_path = app.modules[0].path.clone();
        let plan_path = app.engine.plan_cache.plan_path_for(&module_path);
        app.engine.plan_cache.register(
            module_path,
            plan_path,
            1,
            vec!["module.net".to_string(), "null_resource.a".to_string()],
        );

        app.request_apply_confirm(&[0]);
        let s = render_to_string(&mut app, 120, 35);
        assert!(
            s.contains("TARGETED"),
            "apply confirm missing TARGETED warning"
        );
    }
}
