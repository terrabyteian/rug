pub mod clipboard;
pub mod keybar;
pub mod layout;
pub mod output;
pub mod output_layout;
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
}

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
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
            // Drain everything already queued before the next render, so a burst
            // of input doesn't cost one full draw per event.
            loop {
                match event::read()? {
                    Event::Mouse(mouse) => handle_mouse(app, mouse),
                    Event::Key(key) => {
                        // The kitty keyboard protocol (enabled in a follow-up) can
                        // deliver Release events; Press and Repeat both act.
                        let quit = key.kind != KeyEventKind::Release
                            && matches!(handle_key_event(terminal, app, key), ScreenAction::Quit);
                        if quit {
                            return Ok(());
                        }
                    }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
}

/// Dispatch one key event against the app. Returns `ScreenAction::Quit`
/// when the event loop must exit immediately.
fn handle_key_event<B: ratatui::backend::Backend>(
    terminal: &Terminal<B>,
    app: &mut App,
    key: event::KeyEvent,
) -> ScreenAction {
    // Ctrl-C: quit immediately if no tasks are running, otherwise
    // surface the same confirmation overlay as `q`. A second Ctrl-C
    // while the overlay is up forces the quit through.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        if matches!(app.modal, Some(Modal::Quit)) || app.active_tasks().is_empty() {
            return ScreenAction::Quit;
        }
        app.modal = Some(Modal::Quit);
        return ScreenAction::None;
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
                return ScreenAction::Quit;
            }
            app.modal = Some(Modal::Quit);
        }
        return ScreenAction::None;
    }

    // Fullscreen output mode (Run session): Esc exits, scroll keys
    // pass through, all else swallowed.
    if app.session.as_ref().map(|s| s.fullscreen).unwrap_or(false) {
        match key.code {
            KeyCode::Esc => fullscreen_esc(app),
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
                app.run_scroll_to_top();
            }
            KeyCode::Char('G') => {
                app.run_scroll_to_bottom();
            }
            KeyCode::Char('w') => {
                if let Some(s) = app.session.as_mut() {
                    s.output_wrap = !s.output_wrap;
                }
            }
            KeyCode::Char('y') => {
                if let Some(text) = app.run_selected_text() {
                    let _ = crate::ui::clipboard::copy_to_clipboard(&text);
                }
            }
            KeyCode::Char('Y') => {
                if let Some(text) = app.run_all_output_text() {
                    let _ = crate::ui::clipboard::copy_to_clipboard(&text);
                }
            }
            _ => {}
        }
        return ScreenAction::None;
    }

    // Modal overlay intercepts all keys: Quit (q force-quits, Esc
    // cancels); Confirm/CancelTasks/ClearTasks/Reset (y/Y/Enter act,
    // anything else dismisses); Help (any key closes).
    if app.modal.is_some() {
        if matches!(app.modal, Some(Modal::Quit)) {
            match key.code {
                KeyCode::Char('q') => return ScreenAction::Quit,
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
        return ScreenAction::None;
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
                    app.resource_detail_scroll(app.viewport.explorer.max(1) as i32);
                }
                KeyCode::PageUp => {
                    app.resource_detail_scroll(-(app.viewport.explorer.max(1) as i32));
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
                    app.state_explorer_move(-(app.viewport.explorer.max(1) as i32));
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
        return ScreenAction::None;
    }

    let action = match app.screen {
        Screen::Select => select::handle_key(app, key),
        Screen::Run => run::handle_key(app, key),
    };
    match action {
        ScreenAction::Quit => {
            if app.active_tasks().is_empty() {
                return ScreenAction::Quit;
            }
            app.modal = Some(Modal::Quit);
        }
        ScreenAction::None => {}
    }

    ScreenAction::None
}

/// Esc while the Run session is fullscreen: clear an active selection first;
/// only exit fullscreen once there is no selection left to clear.
fn fullscreen_esc(app: &mut App) {
    if app.session.as_ref().is_some_and(|s| s.selection.is_some()) {
        app.run_clear_selection();
    } else if let Some(s) = app.session.as_mut() {
        s.fullscreen = false;
    }
}

/// Handle a single mouse event against the current app state.
fn handle_mouse(app: &mut App, mouse: event::MouseEvent) {
    // Hard overlays absorb all mouse events. Help does not.
    if app.filter_active
        || app
            .modal
            .as_ref()
            .is_some_and(|m| !matches!(m, Modal::Help))
    {
        return;
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
        return;
    }

    // An in-progress drag (or a still-pending press anchor that a drag would
    // promote) owns the gesture regardless of pointer position: a drag that
    // wanders over the board must not click board rows, and a release
    // anywhere must end it.
    let gesture_active = app
        .session
        .as_ref()
        .is_some_and(|s| s.pending_sel.is_some() || s.selection.is_some_and(|sel| sel.dragging));
    if gesture_active
        && matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        )
    {
        handle_output_mouse(app, &mouse);
        return;
    }

    // Fullscreen output (Run session): selection gestures, then wheel scroll.
    if app.session.as_ref().map(|s| s.fullscreen).unwrap_or(false) {
        if handle_output_mouse(app, &mouse) {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                app.run_scroll_output(1);
            }
            MouseEventKind::ScrollDown => {
                app.run_scroll_output(-1);
            }
            _ => {}
        }
        return;
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
                    let pos = app.viewport.list_offset as usize + (mouse.row - top) as usize;
                    app.set_module_cursor(pos);
                }
            }
            _ => {}
        },
        Screen::Run => {
            // Selection gestures in the output pane take priority; a miss
            // (e.g. the board, or dead space) falls through unconsumed.
            if handle_output_mouse(app, &mouse) {
                return;
            }
            let board_top = app.viewport.board_top;
            let board_h = app.viewport.board;
            let output_top = app.viewport.output_top;
            let in_board = mouse.row >= board_top && mouse.row < board_top.saturating_add(board_h);
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
                        let pos =
                            app.viewport.board_offset as usize + (mouse.row - board_top) as usize;
                        app.run_set_cursor(pos);
                    }
                }
                _ => {}
            }
        }
    }
}

/// True when `(mouse.column, mouse.row)` falls inside the output pane's
/// geometry from the last draw (independent of whether it lands on content).
fn point_in_output_region(app: &App, mouse: &event::MouseEvent) -> bool {
    let vp = app.viewport;
    if vp.output_top == u16::MAX || vp.output_width == 0 {
        return false;
    }
    mouse.row >= vp.output_top
        && mouse.row < vp.output_top.saturating_add(vp.output)
        && mouse.column >= vp.output_left
        && mouse.column < vp.output_left.saturating_add(vp.output_width)
}

/// Selection gestures (press/drag/release) over the Run output pane, shared
/// between the fullscreen viewer and the normal Run screen's output region.
/// Returns whether the event was consumed.
fn handle_output_mouse(app: &mut App, mouse: &event::MouseEvent) -> bool {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(pos) = app.output_hit_test(mouse.column, mouse.row, false) {
                // Arm a press anchor but don't select yet — a plain click
                // (no drag) must not create a (zero-length) selection.
                app.run_selection_arm(pos);
                true
            } else if point_in_output_region(app, mouse) {
                // A click inside the pane that didn't land on content (e.g.
                // below the last line): drop any stale selection.
                app.run_clear_selection();
                true
            } else {
                false
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let dragging = app
                .session
                .as_ref()
                .and_then(|s| s.selection)
                .is_some_and(|sel| sel.dragging);
            if !dragging && !app.run_selection_begin_from_pending() {
                return false;
            }
            // Edge auto-scroll: dragging past the top/bottom of the pane
            // reveals more content in that direction.
            let vp = app.viewport;
            if vp.output_top != u16::MAX {
                if mouse.row < vp.output_top {
                    app.run_scroll_output(1);
                } else if mouse.row >= vp.output_top.saturating_add(vp.output) {
                    app.run_scroll_output(-1);
                }
            }
            if let Some(pos) = app.output_hit_test(mouse.column, mouse.row, true) {
                app.run_selection_drag(pos);
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let dragging = app
                .session
                .as_ref()
                .and_then(|s| s.selection)
                .is_some_and(|sel| sel.dragging);
            if dragging {
                app.run_selection_end();
                // Auto-copy on mouse release.
                if let Some(text) = app.run_selected_text() {
                    let _ = crate::ui::clipboard::copy_to_clipboard(&text);
                }
                true
            } else {
                // No drag happened: consume the release iff a press anchor
                // is still pending (a plain click).
                app.run_discard_pending_sel()
            }
        }
        _ => false,
    }
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
    ("drag / y", "select / copy output"),
    ("Y", "copy all output"),
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
    // Inner width of the bordered block — `theme::cursor_row` pads to this so the
    // selected row's bar spans the popup instead of stopping at the text.
    let inner_width = area.width.saturating_sub(2);

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
                            let (marker, marker_style) = if marked {
                                (" ● ", theme::multi_select_marker())
                            } else {
                                ("   ", theme::title())
                            };
                            let line = Line::from(vec![
                                Span::styled(marker.to_string(), marker_style),
                                Span::styled(format!("▸ {} ({})", prefix, count), theme::title()),
                            ]);
                            lines.push(if is_selected {
                                theme::cursor_row(line, inner_width)
                            } else {
                                line
                            });
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
                            // Build the row once, then let `cursor_row` highlight
                            // it: the marker, the filter match and the taint tag
                            // keep their colours on the selected row.
                            let (marker, marker_style) = if is_multi {
                                (" ● ", theme::multi_select_marker())
                            } else if covered {
                                (" ● ", theme::covered_marker())
                            } else {
                                ("   ", Style::default())
                            };
                            let addr = format!("{}{}", indent_str, resource.address);
                            let mut spans = vec![Span::styled(marker.to_string(), marker_style)];
                            if explorer.filter.is_empty() {
                                spans.push(Span::raw(addr));
                            } else {
                                spans.extend(highlight_filter_match(addr, &filter_lower).spans);
                            }
                            if resource.is_tainted() {
                                spans.push(Span::styled(
                                    " [tainted]".to_string(),
                                    Style::default().fg(Color::Red),
                                ));
                            }
                            let line = Line::from(spans);
                            lines.push(if is_selected {
                                theme::cursor_row(line, inner_width)
                            } else {
                                line
                            });
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
    use crate::app::{OutputSelection, RunSession, SelPos, SessionModule};
    use crate::config::Config;
    use crate::module::{Module, ModuleKind};
    use crate::task::{ResourceCounts, Task, TaskStatus};
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

    /// `n` output lines of `len` chars each; the LAST line ends with `marker`
    /// (padded with `a` to `len` total) so tail-visibility assertions have a
    /// unique needle regardless of wrap/scroll math.
    fn make_output_lines(n: usize, len: usize, marker: &str) -> Vec<String> {
        (0..n)
            .map(|i| {
                if i + 1 == n {
                    let pad = len.saturating_sub(marker.len());
                    format!("{}{}", "a".repeat(pad), marker)
                } else {
                    "b".repeat(len)
                }
            })
            .collect()
    }

    /// Attach `lines` as a running task's output for the session's (single)
    /// module, and record it as that module's latest/display task.
    fn attach_output(app: &mut App, lines: Vec<String>) {
        let path = app.modules[0].path.clone();
        let id = app.engine.tasks.len();
        app.engine.tasks.push(Task {
            id,
            module_path: path.clone(),
            module_name: "mod0".to_string(),
            command: "apply".to_string(),
            status: TaskStatus::Running,
            output_lines: lines,
            started_at: Some(Instant::now()),
            finished_at: None,
            plan_output_path: None,
            targets: Vec::new(),
            cleanup_plan_path: None,
            resource_counts: None,
            cancel_handle: None,
        });
        app.session.as_mut().unwrap().latest_task.insert(path, id);
    }

    fn draw_at(app: &mut App, w: u16, h: u16) {
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
    }

    /// Draw and return the back buffer, for tests that inspect cell styling
    /// (e.g. the selection's REVERSED overlay) rather than just symbols.
    fn draw_buffer(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    /// A left-button mouse event at `(column, row)`, no modifiers.
    fn left_mouse(kind: MouseEventKind, column: u16, row: u16) -> event::MouseEvent {
        event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
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

    /// The cursor bar is bg = MUTED, and ratatui draws span foregrounds over it
    /// (spans carry bg = None), so a MUTED fg lands MUTED-on-MUTED and vanishes —
    /// which is what made the Run board's elapsed timer invisible.
    ///
    /// The invariant checked is `lift_color(fg) == fg`: every fg on the bar must
    /// already be a lifted colour. That is stricter than `fg != bg` — it also
    /// rejects merely-dark foregrounds such as plain `Blue` — and it needs no
    /// hand-maintained list of styles, so any span later drawn on the bar by any
    /// producer is covered automatically.
    ///
    /// Reports every offending cell at once: the first one is usually the cursor
    /// bar glyph, which would otherwise mask the columns further right.
    fn assert_cursor_bar_is_lifted(buffer: &ratatui::buffer::Buffer, w: u16, h: u16, ctx: &str) {
        let mut bad: Vec<String> = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let cell = &buffer[(x, y)];
                if cell.bg != theme::MUTED || cell.symbol().trim().is_empty() {
                    continue;
                }
                if theme::lift_color(cell.fg) != cell.fg {
                    bad.push(format!(
                        "({x},{y}) {:?} fg {:?} should be {:?}",
                        cell.symbol(),
                        cell.fg,
                        theme::lift_color(cell.fg)
                    ));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "{ctx} at {w}x{h}: {} cell(s) on the cursor bar carry an unlifted fg:\n  {}",
            bad.len(),
            bad.join("\n  ")
        );
    }

    /// Give module 0 a finished task with every optional column populated, and
    /// modules 1/2 the two MUTED-styled statuses (queued, cancelled).
    fn attach_board_tasks(app: &mut App) {
        let counts = ResourceCounts {
            add: 2,
            change: 1,
            destroy: 1,
            import: 1,
            forget: 1, // COUNT_FORGET is DarkGray — invisible on the bar before the fix
            no_changes: false,
            has_summary: true,
        };
        let statuses = [
            (TaskStatus::Success, Some(counts)),
            (TaskStatus::Pending, None),   // status_style() -> MUTED
            (TaskStatus::Cancelled, None), // status_style() -> MUTED
        ];
        for (i, (status, resource_counts)) in statuses.into_iter().enumerate() {
            let path = app.modules[i].path.clone();
            let id = app.engine.tasks.len();
            app.engine.tasks.push(Task {
                id,
                module_path: path.clone(),
                module_name: format!("mod{i}"),
                command: "apply".to_string(), // command_text()
                status,
                output_lines: Vec::new(),
                started_at: Some(Instant::now()), // drives elapsed_str() -> dim()
                finished_at: None,
                plan_output_path: None,
                targets: Vec::new(),
                cleanup_plan_path: None,
                resource_counts,
                cancel_handle: None,
            });
            app.session.as_mut().unwrap().latest_task.insert(path, id);
        }
    }

    #[test]
    fn run_board_cursor_row_stays_legible() {
        let mut app = demo_app(3);
        make_session(&mut app);
        attach_board_tasks(&mut app);

        // Walk the cursor over every row: each carries a different status, and
        // only the highlighted one is drawn on the bar.
        for cursor in 0..3 {
            app.session.as_mut().unwrap().cursor = cursor;
            for &(w, h) in SIZES {
                let buffer = draw_buffer(&mut app, w, h);
                assert_cursor_bar_is_lifted(&buffer, w, h, &format!("run board row {cursor}"));
            }
        }
    }

    #[test]
    fn select_cursor_row_stays_legible() {
        let mut app = demo_app(3);
        make_session(&mut app);
        attach_board_tasks(&mut app);
        app.screen = Screen::Select;

        for selected in 0..3 {
            app.selected_module = selected;
            for &(w, h) in SIZES {
                let buffer = draw_buffer(&mut app, w, h);
                assert_cursor_bar_is_lifted(&buffer, w, h, &format!("select row {selected}"));
            }
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
    fn run_help_shows_drag_copy_hint() {
        let mut app = demo_app(3);
        app.screen = Screen::Run;
        app.enter_run();
        app.modal = Some(Modal::Help);
        let s = render_to_string(&mut app, 120u16, 35u16);
        // Check that the drag/copy help text is rendered.
        assert!(
            s.contains("drag / y") || s.contains("drag/y"),
            "Run help missing drag/y key hint"
        );
        assert!(
            s.contains("select / copy"),
            "Run help missing select/copy description"
        );
        // Check that the Y (copy all) help text is rendered.
        assert!(
            s.contains("Y"),
            "Run help missing Y key hint"
        );
        assert!(
            s.contains("copy all output"),
            "Run help missing copy all output description"
        );
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

    /// The explorer's selected row used to be one flat Black-on-Cyan span. Now it
    /// shares the boards' bar, so it has to hold the same invariant — including
    /// for `covered_marker()`, which is MUTED and only ever reaches the bar via
    /// this screen.
    #[test]
    fn explorer_selected_row_stays_legible() {
        let mut app = demo_explorer_app();
        {
            let ex = app.state_explorer.as_mut().unwrap();
            ex.multi_select = vec![0];
            ex.module_select = vec!["module.net".to_string()]; // -> covered_marker() on members
        }
        // Rows: module header, its 2 members, the root resource.
        for selected in 0..4 {
            app.state_explorer.as_mut().unwrap().selected = selected;
            for &(w, h) in SIZES {
                let buffer = draw_buffer(&mut app, w, h);
                assert_cursor_bar_is_lifted(&buffer, w, h, &format!("explorer row {selected}"));
            }
        }
    }

    /// A tainted resource keeps its red tag and a marked one its yellow marker on
    /// the selected row — both were flattened to black by the old Cyan highlight.
    #[test]
    fn explorer_selected_row_keeps_column_colours() {
        use crate::state::{StateContent, StateResource};
        let mut app = demo_explorer_app();
        {
            let ex = app.state_explorer.as_mut().unwrap();
            ex.content = StateContent::Resources(vec![StateResource {
                address: "aws_vpc.main".to_string(),
                instance: serde_json::json!({ "status": "tainted" }),
            }]);
            ex.multi_select = vec![0];
            ex.selected = 0;
        }
        let buffer = draw_buffer(&mut app, 120, 35);
        let on_bar = |want: ratatui::style::Color| {
            (0..35).any(|y| {
                (0..120u16).any(|x| {
                    let c = &buffer[(x, y)];
                    c.bg == theme::MUTED && c.fg == want
                })
            })
        };
        assert!(
            on_bar(ratatui::style::Color::LightRed),
            "the [tainted] tag should stay red on the selected row"
        );
        assert!(
            on_bar(ratatui::style::Color::LightYellow),
            "the multi-select marker should stay yellow on the selected row"
        );
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

    // ── Output pane: wrap + resize scroll math (WP3) ─────────────────────────

    /// With wrap on, the tail of the last output line must be visible at
    /// `output_scroll == 0` (tail-follow). Regression test for the bug where
    /// `auto_bottom` was computed from the unwrapped source-line count instead
    /// of the layout's wrapped display-row count, pinning the view above the
    /// real tail whenever wrapping expanded the content past the pane height.
    #[test]
    fn wrap_tail_visible_after_draw() {
        let mut app = demo_app(1);
        make_session(&mut app);
        let marker = "TAILMARK9182";
        attach_output(&mut app, make_output_lines(30, 100, marker));
        {
            let s = app.session.as_mut().unwrap();
            s.output_wrap = true;
            s.output_scroll = 0;
        }

        let s = render_to_string(&mut app, 40, 24);
        assert!(
            s.contains(marker),
            "tail marker not visible at scroll=0 with wrap on"
        );
    }

    /// Growing the terminal after scrolling up must reclamp `output_scroll`
    /// to the new (smaller) max — not leave it pinned at a stale value that
    /// now points above the top of the content.
    #[test]
    fn resize_grow_reclamps_scroll() {
        let mut app = demo_app(1);
        make_session(&mut app);
        let marker = "TAILRESIZE42";
        attach_output(&mut app, make_output_lines(50, 20, marker));

        // Small draw: H3 tier (40x16) still renders a real output pane —
        // H4 (height < 15) drops it entirely for a one-line status tail.
        draw_at(&mut app, 40, 16);
        app.run_scroll_output(i32::MAX);
        let scrolled_small = app.session.as_ref().unwrap().output_scroll;
        assert!(scrolled_small > 0, "scroll should have moved off the tail");

        // Grow the terminal: the output pane gets much taller, so max_scroll
        // shrinks well below the stale `scrolled_small` value.
        draw_at(&mut app, 40, 30);
        let total = app.output_layout.total_rows();
        let new_viewport = app.viewport.output as usize;
        let max_scroll = total.saturating_sub(new_viewport);
        let after = app.session.as_ref().unwrap().output_scroll;
        assert!(
            after <= max_scroll,
            "resize should reclamp output_scroll ({after}) to the new max ({max_scroll}); \
             old code left it at the stale small-viewport value ({scrolled_small})"
        );

        // Scrolled back to the tail, the marker line is visible again.
        app.session.as_mut().unwrap().output_scroll = 0;
        let s = render_to_string(&mut app, 40, 30);
        assert!(
            s.contains(marker),
            "tail marker not visible after resize+follow"
        );
    }

    /// `run_scroll_output`'s upper clamp is `total_rows - viewport height`,
    /// not the raw output-line count.
    #[test]
    fn scroll_clamp_bound() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(&mut app, make_output_lines(50, 20, "END"));

        draw_at(&mut app, 40, 30);
        let total = app.output_layout.total_rows();
        let viewport = app.viewport.output as usize;
        assert!(viewport > 0, "expected a real output pane at 40x30");

        app.run_scroll_output(i32::MAX);
        let scroll = app.session.as_ref().unwrap().output_scroll;
        assert_eq!(scroll, total.saturating_sub(viewport));
        assert_ne!(
            scroll, 50,
            "clamp must use display rows minus viewport height, not the raw line count"
        );
    }

    /// Same tail-visibility guarantee as `wrap_tail_visible_after_draw`, in
    /// the fullscreen renderer — geometry is measured off the bordered
    /// block's inner rect, so this also exercises the border inset.
    #[test]
    fn fullscreen_wrap_tail_visible() {
        let mut app = demo_app(1);
        make_session(&mut app);
        let marker = "FSTAILMARK77";
        attach_output(&mut app, make_output_lines(30, 100, marker));
        {
            let s = app.session.as_mut().unwrap();
            s.fullscreen = true;
            s.output_wrap = true;
            s.output_scroll = 0;
        }

        let s = render_to_string(&mut app, 40, 24);
        assert!(
            s.contains(marker),
            "tail marker not visible in fullscreen wrap mode"
        );
    }

    /// Direct unit test of `visible_lines`' selection overlay: a multi-line
    /// selection reverses exactly the intersection of each row with the
    /// (start, end) span — partial on the first and last lines, whole on any
    /// line strictly between them.
    #[test]
    fn visible_lines_selection_overlay_multiline() {
        use crate::app::SelPos;
        use crate::ui::output_layout::{visible_lines, OutputLayout};
        use ratatui::style::Modifier;

        let lines = vec![
            "hello world".to_string(),
            "middle line".to_string(),
            "goodbye all".to_string(),
        ];
        let mut layout = OutputLayout::default();
        layout.sync(Some(0), &lines, 80, false);

        // line 0 char 6 → line 2 char 7: tail of line 0 ("world"), all of
        // line 1, head of line 2 ("goodbye").
        let sel = Some((SelPos { line: 0, ch: 6 }, SelPos { line: 2, ch: 7 }));
        let rows = visible_lines(&lines, &layout, 0, 3, 80, false, sel);
        assert_eq!(rows.len(), 3);

        let reversed_text = |spans: &[ratatui::text::Span]| -> String {
            spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
                .map(|s| s.content.as_ref())
                .collect()
        };
        let plain_text = |spans: &[ratatui::text::Span]| -> String {
            spans
                .iter()
                .filter(|s| !s.style.add_modifier.contains(Modifier::REVERSED))
                .map(|s| s.content.as_ref())
                .collect()
        };

        assert_eq!(plain_text(&rows[0].spans), "hello ");
        assert_eq!(reversed_text(&rows[0].spans), "world");

        assert_eq!(plain_text(&rows[1].spans), "");
        assert_eq!(reversed_text(&rows[1].spans), "middle line");

        assert_eq!(plain_text(&rows[2].spans), " all");
        assert_eq!(reversed_text(&rows[2].spans), "goodbye");
    }

    // ── Mouse-driven output selection (WP4) ──────────────────────────────────

    /// Press-drag over the output pane records the anchor/head content
    /// coordinates and the subsequent render reverses exactly the selected
    /// cells (and nothing outside them).
    #[test]
    fn mouse_drag_selects_and_renders_reversed() {
        use ratatui::style::Modifier;

        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(
            &mut app,
            vec![
                "hello world".to_string(),
                "second row here".to_string(),
                "third row texts".to_string(),
            ],
        );
        draw_at(&mut app, 80, 30);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;
        assert_ne!(top, u16::MAX, "expected a real output pane at 80x30");

        // Press at line 0, char 6 ('w' of "world").
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left + 6, top),
        );
        // Drag down two rows to line 2, char 5 (the space before "row").
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Drag(MouseButton::Left), left + 5, top + 2),
        );

        let sel = app
            .session
            .as_ref()
            .unwrap()
            .selection
            .expect("selection should exist after a drag");
        assert_eq!(sel.anchor, SelPos { line: 0, ch: 6 });
        assert_eq!(sel.head, SelPos { line: 2, ch: 5 });

        let buffer = draw_buffer(&mut app, 80, 30);
        assert!(
            buffer[(left + 6, top)]
                .modifier
                .contains(Modifier::REVERSED),
            "expected the selected cell ('w' of world) to be reversed"
        );
        assert!(
            !buffer[(left, top)].modifier.contains(Modifier::REVERSED),
            "expected a cell outside the selection ('h' of hello) to not be reversed"
        );
    }

    /// `run_selected_text` slices by CHAR index, not byte index — a wide char
    /// on the first line must not corrupt the offset — and joins a
    /// partial-first/whole-middle/partial-last selection with '\n'.
    #[test]
    fn selected_text_multiline() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(
            &mut app,
            vec![
                "abc日def".to_string(),
                "whole middle line".to_string(),
                "xyz last END".to_string(),
            ],
        );

        let task = app.run_display_task_id();
        app.session.as_mut().unwrap().selection = Some(OutputSelection {
            task,
            anchor: SelPos { line: 0, ch: 3 },
            head: SelPos { line: 2, ch: 3 },
            dragging: false,
        });

        let text = app.run_selected_text().expect("expected selected text");
        assert_eq!(text, "日def\nwhole middle line\nxyz");
    }

    /// `run_all_output_text` strips ANSI codes from all lines and joins them
    /// with '\n'. Returns None when there are no output lines.
    #[test]
    fn run_all_output_text_strips_ansi_and_joins() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(
            &mut app,
            vec![
                "\x1b[32mgreen text\x1b[0m".to_string(),
                "plain line".to_string(),
                "\x1b[1mbold日unicode\x1b[0m".to_string(),
            ],
        );

        let text = app.run_all_output_text().expect("expected all output text");
        assert_eq!(text, "green text\nplain line\nbold日unicode");
    }

    /// `run_all_output_text` returns None when there are no output lines.
    #[test]
    fn run_all_output_text_empty() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(&mut app, vec![]);

        let text = app.run_all_output_text();
        assert_eq!(text, None);
    }

    /// With wrap on, clicking display row 2 of a single long wrapped line must
    /// resolve to the char offset of THAT row, not the row-0 offset (i.e. the
    /// hit-test must walk through `layout.locate`, not just index the line).
    #[test]
    fn drag_maps_through_wrapped_rows() {
        let mut app = demo_app(1);
        make_session(&mut app);
        if let Some(s) = app.session.as_mut() {
            s.output_wrap = true;
        }
        // Draw once (empty output) purely to learn the pane's content width.
        draw_at(&mut app, 60, 30);
        let width = app.viewport.output_width as usize;
        assert!(width > 0, "expected a real output pane at 60x30");

        // 3 full rows + 1 char into a 4th, so display row 2 is a full middle row.
        let long_line = "x".repeat(width * 3 + 1);
        attach_output(&mut app, vec![long_line]);
        draw_at(&mut app, 60, 30);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;

        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left + 4, top + 2),
        );
        // A plain press only arms a pending anchor; a drag materializes it.
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Drag(MouseButton::Left), left + 4, top + 2),
        );

        let sel = app
            .session
            .as_ref()
            .unwrap()
            .selection
            .expect("selection should exist");
        assert_eq!(
            sel.anchor,
            SelPos {
                line: 0,
                ch: width * 2 + 4
            },
            "expected the wrapped row-2 offset, not the row-0 offset"
        );
    }

    /// H4 (collapsed layout, e.g. 40×10) drops the output pane entirely — a
    /// click anywhere must not create a selection.
    #[test]
    fn h4_collapsed_no_selection() {
        let mut app = demo_app(3);
        make_session(&mut app);
        draw_at(&mut app, 40, 10);
        assert_eq!(
            app.viewport.output_top,
            u16::MAX,
            "expected the collapsed H4 layout to have no output pane"
        );

        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), 5, 5),
        );

        assert!(app.session.as_ref().unwrap().selection.is_none());
    }

    /// Fullscreen output is measured off the bordered block's inner rect —
    /// a click at the recorded `(output_left, output_top)` must map to the
    /// very first visible char, one cell in from the window edge.
    #[test]
    fn fullscreen_selection_border_offset() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(
            &mut app,
            vec!["ABCDEFGH".to_string(), "second line".to_string()],
        );
        if let Some(s) = app.session.as_mut() {
            s.fullscreen = true;
        }
        draw_at(&mut app, 40, 20);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;
        assert_eq!(top, 1, "fullscreen pane should be inset by the top border");
        assert_eq!(
            left, 1,
            "fullscreen pane should be inset by the left border"
        );

        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left, top),
        );
        // A plain press only arms a pending anchor; a drag materializes it.
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Drag(MouseButton::Left), left, top),
        );

        let sel = app
            .session
            .as_ref()
            .unwrap()
            .selection
            .expect("selection should exist");
        assert_eq!(sel.anchor, SelPos { line: 0, ch: 0 });
    }

    /// Moving the board cursor (even via keyboard, not just clicking a
    /// different row) drops any in-progress or completed selection, since it
    /// no longer refers to the now-displayed task's content.
    #[test]
    fn cursor_move_clears_selection() {
        let mut app = demo_app(2);
        make_session(&mut app);
        attach_output(
            &mut app,
            vec!["one line".to_string(), "two line".to_string()],
        );
        draw_at(&mut app, 80, 30);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left + 1, top),
        );
        // A plain press only arms a pending anchor; a drag materializes it.
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Drag(MouseButton::Left), left + 1, top),
        );
        assert!(
            app.session.as_ref().unwrap().selection.is_some(),
            "expected a selection after the drag"
        );

        app.run_move_cursor(1);
        assert!(
            app.session.as_ref().unwrap().selection.is_none(),
            "moving the board cursor should clear the selection"
        );
    }

    /// A selection's content coordinates are anchored to line/char indices,
    /// not screen position, so streamed output appended after the selection
    /// was made (tail-follow, scroll == 0) must not disturb it.
    #[test]
    fn selection_survives_append() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(
            &mut app,
            vec!["hello world".to_string(), "second line".to_string()],
        );
        draw_at(&mut app, 80, 30);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;

        // Select "world" (chars 6..11) on line 0.
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left + 6, top),
        );
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Drag(MouseButton::Left), left + 11, top),
        );
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Up(MouseButton::Left), left + 11, top),
        );

        let before = app.run_selected_text();
        assert_eq!(before.as_deref(), Some("world"));

        // Append more lines directly to the same still-running task.
        let path = app.modules[0].path.clone();
        if let Some(task) = app.engine.tasks.iter_mut().find(|t| t.module_path == path) {
            task.output_lines.push("third line".to_string());
            task.output_lines.push("fourth line".to_string());
        }
        draw_at(&mut app, 80, 30);

        assert_eq!(
            app.session
                .as_ref()
                .unwrap()
                .selection
                .map(|s| (s.anchor, s.head)),
            Some((SelPos { line: 0, ch: 6 }, SelPos { line: 0, ch: 11 })),
            "selection coordinates should survive an append"
        );
        assert_eq!(app.run_selected_text().as_deref(), before.as_deref());
    }

    /// Dragging above the top of the output pane auto-scrolls up (toward
    /// older content) and the drag head clamps to the new topmost visible row.
    #[test]
    fn drag_above_pane_autoscrolls() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(&mut app, make_output_lines(50, 20, "TAILEND"));
        draw_at(&mut app, 40, 20);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;
        assert_eq!(app.session.as_ref().unwrap().output_scroll, 0);

        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left, top),
        );
        // A plain press only arms a pending anchor — no selection yet.
        assert!(app.session.as_ref().unwrap().selection.is_none());
        assert!(app.session.as_ref().unwrap().pending_sel.is_some());

        // Drag one row above the pane's top edge. Even though this is the
        // first drag event and it already left the content area, it must
        // still promote the pending anchor into a selection anchored at the
        // press point, then extend to the clamped position.
        handle_mouse(
            &mut app,
            left_mouse(
                MouseEventKind::Drag(MouseButton::Left),
                left,
                top.saturating_sub(1),
            ),
        );

        let scroll_after = app.session.as_ref().unwrap().output_scroll;
        assert!(scroll_after > 0, "dragging above the pane should scroll up");

        let total = app.output_layout.total_rows();
        let viewport_h = app.viewport.output as usize;
        let max_scroll = total.saturating_sub(viewport_h);
        let expected_first_row = max_scroll.saturating_sub(scroll_after);

        let head = app.session.as_ref().unwrap().selection.unwrap().head;
        assert_eq!(
            head.line, expected_first_row,
            "head should clamp to the new top visible row"
        );
        assert_eq!(
            head.ch, 0,
            "clamped column should map to the row's first char"
        );
    }

    /// Esc while fullscreen clears a selection before it exits fullscreen —
    /// only a second Esc (with nothing left to clear) leaves fullscreen.
    #[test]
    fn esc_clears_selection_before_exiting_fullscreen() {
        let mut app = demo_app(1);
        make_session(&mut app);
        attach_output(&mut app, vec!["hello world".to_string()]);
        if let Some(s) = app.session.as_mut() {
            s.fullscreen = true;
        }
        draw_at(&mut app, 40, 20);

        let top = app.viewport.output_top;
        let left = app.viewport.output_left;
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Down(MouseButton::Left), left, top),
        );
        // A plain press only arms a pending anchor; a drag materializes it.
        handle_mouse(
            &mut app,
            left_mouse(MouseEventKind::Drag(MouseButton::Left), left, top),
        );
        assert!(app.session.as_ref().unwrap().selection.is_some());
        assert!(app.session.as_ref().unwrap().fullscreen);

        fullscreen_esc(&mut app);
        assert!(
            app.session.as_ref().unwrap().selection.is_none(),
            "first Esc should clear the selection"
        );
        assert!(
            app.session.as_ref().unwrap().fullscreen,
            "first Esc should not exit fullscreen yet"
        );

        fullscreen_esc(&mut app);
        assert!(
            !app.session.as_ref().unwrap().fullscreen,
            "second Esc should exit fullscreen"
        );
    }
}
