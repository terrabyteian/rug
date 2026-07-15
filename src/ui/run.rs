//! Run screen: a status board of the session's modules over a live output pane
//! for the highlighted module. Actions run on the whole session (or a board
//! subset); Esc returns to Select while tasks keep running.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::task::{Task, TaskStatus};
use crate::ui::keybar::render_keybar;
use crate::ui::layout::{Breakpoints, HeightTier, WidthTier};
use crate::ui::output::parse_ansi;
use crate::ui::theme;
use crate::ui::widgets::count_spans;
use crate::ui::ScreenAction;

/// Render the Run screen (non-fullscreen).
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let bp = Breakpoints::of(area);
    let Some(session) = app.session.as_ref() else {
        return;
    };
    let n = session.modules.len() as u16;

    let show_col_header = matches!(bp.h, HeightTier::H1 | HeightTier::H2);
    let drop_output = matches!(bp.h, HeightTier::H4);

    if drop_output {
        render_collapsed(f, area, app, bp);
        return;
    }

    let col_h: u16 = if show_col_header { 1 } else { 0 };
    let avail = area.height.saturating_sub(3); // header, separator, keybar
    let board_budget = match bp.h {
        HeightTier::H1 => avail.saturating_mul(2).saturating_div(5).max(5),
        HeightTier::H2 => (avail / 3).max(4),
        HeightTier::H3 => 5,
        HeightTier::H4 => avail,
    };
    let mut board_section = (n + col_h)
        .min(board_budget)
        .max(col_h + 1)
        .min(avail.max(1));
    // H2 (20–29 rows): keep the output pane at least 6 rows tall.
    if matches!(bp.h, HeightTier::H2) {
        board_section = board_section
            .min(avail.saturating_sub(6))
            .max(col_h + 1)
            .min(avail.max(1));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),             // header
            Constraint::Length(board_section), // col header + board list
            Constraint::Length(1),             // separator
            Constraint::Min(0),                // output pane
            Constraint::Length(1),             // keybar
        ])
        .split(area);
    let header_area = chunks[0];
    let board_area = chunks[1];
    let sep_area = chunks[2];
    let output_area = chunks[3];
    let keybar_area = chunks[4];

    render_header(f, header_area, app);

    // Split the board section into optional column header + list.
    let (col_area, list_area) = if show_col_header {
        let bc = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(board_area);
        (Some(bc[0]), bc[1])
    } else {
        (None, board_area)
    };
    if let Some(col_area) = col_area {
        render_col_header(f, col_area, bp.w);
    }
    let board_offset = render_board(f, list_area, app, bp.w);

    render_separator(f, sep_area, app);
    render_output(f, output_area, app);
    render_run_keybar(f, keybar_area);

    // Record viewport heights + regions for page scrolling and mouse hit-testing.
    app.viewport.output = output_area.height;
    app.viewport.board = list_area.height;
    app.viewport.board_top = list_area.y;
    app.viewport.board_offset = board_offset;
    app.viewport.output_top = output_area.y;
}

/// H4 layout: no output pane; board fills the space with a one-line status
/// tail for the cursor module.
fn render_collapsed(f: &mut Frame, area: Rect, app: &mut App, bp: Breakpoints) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // board list
            Constraint::Length(1), // status tail
            Constraint::Length(1), // keybar
        ])
        .split(area);

    render_header(f, chunks[0], app);
    let board_offset = render_board(f, chunks[1], app, bp.w);

    // Status tail: last output line of the cursor module.
    let last = app
        .run_output_lines()
        .last()
        .map(|l| crate::util::strip_ansi(l))
        .unwrap_or_default();
    let tail = Line::from(vec![
        Span::styled(
            format!("{} ", truncate(&last, area.width.saturating_sub(12) as usize)),
            theme::dim(),
        ),
        Span::styled("⏎ output", theme::dim()),
    ]);
    f.render_widget(Paragraph::new(tail), chunks[2]);
    render_run_keybar(f, chunks[3]);

    app.viewport.board = chunks[1].height;
    app.viewport.output = 1;
    app.viewport.board_top = chunks[1].y;
    app.viewport.board_offset = board_offset;
    // No scrollable output pane in the collapsed layout: keep wheel events on
    // the board by placing the output region past the bottom of the screen.
    app.viewport.output_top = u16::MAX;
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let Some(session) = app.session.as_ref() else {
        return;
    };
    let n = session.modules.len();

    // Tally display-task states across the session.
    let mut running = 0usize;
    let mut ok = 0usize;
    let mut failed = 0usize;
    for m in &session.modules {
        if let Some((t, _)) = app.display_task_for(&m.path) {
            match t.status {
                TaskStatus::Running | TaskStatus::Pending | TaskStatus::Cancelling => running += 1,
                TaskStatus::Success => ok += 1,
                TaskStatus::Failed => failed += 1,
                TaskStatus::Cancelled => {}
            }
        }
    }

    let secs = session.created_at.elapsed().as_secs();
    let mmss = format!("{:02}:{:02}", secs / 60, secs % 60);

    let left = Line::from(vec![
        Span::styled("Run", theme::app_title()),
        Span::styled(
            format!(" · {n} modules · {running} running · "),
            theme::dim(),
        ),
        Span::styled(format!("✓{ok}"), Style::default().fg(theme::OK)),
        Span::raw(" "),
        Span::styled(format!("✗{failed}"), Style::default().fg(theme::ERR)),
        Span::styled(format!(" · {mmss}"), theme::dim()),
    ]);
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled("esc back", theme::dim())))
            .alignment(Alignment::Right),
        area,
    );
}

fn render_col_header(f: &mut Frame, area: Rect, w: WidthTier) {
    let text = match w {
        WidthTier::W1 => "  MODULE                COMMAND  STATUS      TIME  CHANGES",
        WidthTier::W2 => "  MODULE                COMMAND  STATUS      TIME",
        WidthTier::W3 => "  MODULE                STATUS      TIME",
        WidthTier::W4 => "  MODULE            STATUS",
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, theme::col_header()))),
        area,
    );
}

/// Render the board list and return its scroll offset (for mouse hit-testing).
fn render_board(f: &mut Frame, area: Rect, app: &App, w: WidthTier) -> u16 {
    let Some(session) = app.session.as_ref() else {
        return 0;
    };

    let items: Vec<ListItem> = session
        .modules
        .iter()
        .enumerate()
        .map(|(pos, sm)| {
            let is_cursor = pos == session.cursor;
            let is_multi = session.selected.contains(&pos);
            let display = app.display_task_for(&sm.path);
            let line = board_row(app, sm, is_cursor, is_multi, display, w);
            let item = ListItem::new(line);
            if is_cursor {
                item.style(theme::row_cursor())
            } else {
                item
            }
        })
        .collect();

    let mut state = ListState::default();
    if !session.modules.is_empty() {
        state.select(Some(session.cursor));
    }
    f.render_stateful_widget(List::new(items), area, &mut state);
    state.offset() as u16
}

fn board_row(
    app: &App,
    sm: &crate::app::SessionModule,
    is_cursor: bool,
    is_multi: bool,
    display: Option<(&Task, bool)>,
    w: WidthTier,
) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();

    // Cursor bar.
    if is_cursor {
        spans.push(Span::styled(
            theme::CURSOR_BAR,
            Style::default().fg(theme::ACCENT),
        ));
    } else {
        spans.push(Span::raw(" "));
    }
    // Multi-select mark.
    if is_multi {
        spans.push(Span::styled("● ", theme::multi_select_marker()));
    } else {
        spans.push(Span::raw("  "));
    }
    // Name.
    spans.push(Span::raw(sm.name.clone()));

    let task = display.map(|(t, _)| t);
    let is_prev = display.map(|(_, p)| p).unwrap_or(false);

    let show_command = matches!(w, WidthTier::W1 | WidthTier::W2);
    let show_word = matches!(w, WidthTier::W1);
    let show_elapsed = !matches!(w, WidthTier::W4);
    let show_counts = matches!(w, WidthTier::W1 | WidthTier::W2);
    let show_extras = matches!(w, WidthTier::W1); // P:{age}, ·prev

    // Command (+ target chip: describes the task itself, shown while running
    // and in the finished result row — distinct from the CACHED-PLAN P:{age}·T
    // badge below, which describes the plan-cache entry).
    if show_command {
        let cmd = task.map(|t| t.command.clone()).unwrap_or_else(|| "—".to_string());
        spans.push(Span::styled(format!("  {cmd}"), theme::command_text()));
        if let Some(t) = task {
            if !t.targets.is_empty() {
                spans.push(Span::styled(
                    format!("·T{}", t.targets.len()),
                    theme::plan_marker_targeted(),
                ));
            }
        }
    }

    // Status icon (+ word).
    let (icon, word, style) = status_parts(app, task);
    spans.push(Span::styled(format!("  {icon}"), style));
    if show_word {
        spans.push(Span::styled(format!(" {word}"), style));
    }

    // Elapsed.
    if show_elapsed {
        if let Some(t) = task {
            let e = t.elapsed_str();
            if !e.is_empty() {
                spans.push(Span::styled(format!("  {e}"), theme::dim()));
            }
        }
    }

    // Resource counts.
    if show_counts {
        if let Some(t) = task {
            if let Some(counts) = &t.resource_counts {
                spans.extend(count_spans(counts));
            }
        }
    }

    // Cached-plan age + prev tag.
    if show_extras {
        if let Some(entry) = app.engine.plan_cache.get(&sm.path) {
            let age = entry.age_str();
            if entry.is_targeted() {
                spans.push(Span::styled(
                    format!("  P:{age}·T{}", entry.targets.len()),
                    theme::plan_marker_targeted(),
                ));
            } else {
                spans.push(Span::styled(format!("  P:{age}"), theme::plan_marker()));
            }
        }
        if is_prev {
            spans.push(Span::styled("  ·prev", theme::dim()));
        }
    }

    Line::from(spans)
}

/// (icon, word, style) for a board row's display task.
fn status_parts(app: &App, task: Option<&Task>) -> (String, &'static str, Style) {
    let Some(t) = task else {
        return ("·".to_string(), "—", theme::dim());
    };
    let style = theme::status_style(&t.status);
    match t.status {
        TaskStatus::Pending => ("○".to_string(), "queued", style),
        TaskStatus::Running => (theme::spinner(app.spinner_tick).to_string(), "running", style),
        TaskStatus::Cancelling => {
            (theme::spinner(app.spinner_tick).to_string(), "cancel", style)
        }
        TaskStatus::Success => ("✓".to_string(), "done", style),
        TaskStatus::Failed => ("✗".to_string(), "failed", style),
        TaskStatus::Cancelled => ("⊘".to_string(), "cancelled", style),
    }
}

fn render_separator(f: &mut Frame, area: Rect, app: &App) {
    let title = app
        .run_cursor_path()
        .map(|p| {
            let name = app
                .session
                .as_ref()
                .and_then(|s| s.modules.iter().find(|m| m.path == p))
                .map(|m| m.name.clone())
                .unwrap_or_default();
            let (cmd, status) = match app.display_task_for(&p) {
                Some((t, _)) => (cmd_label(t), status_word(&t.status).to_string()),
                None => ("—".to_string(), "idle".to_string()),
            };
            format!(" {name} · {cmd} · {status} ")
        })
        .unwrap_or_default();
    f.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme::dim())
            .title(Span::styled(title, theme::dim())),
        area,
    );
}

/// A task's command with its target-count suffix (`apply·T2`), for the plain
/// (unstyled) titles in the separator and fullscreen headers.
fn cmd_label(t: &Task) -> String {
    if t.targets.is_empty() {
        t.command.clone()
    } else {
        format!("{}·T{}", t.command, t.targets.len())
    }
}

fn status_word(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Cancelling => "cancelling",
        TaskStatus::Success => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn render_output(f: &mut Frame, area: Rect, app: &App) {
    let wrap = app.session.as_ref().map(|s| s.output_wrap).unwrap_or(false);
    let scroll = app.session.as_ref().map(|s| s.output_scroll).unwrap_or(0);
    let out = app.run_output_lines();
    let lines: Vec<Line> = out.iter().map(|l| parse_ansi(l)).collect();

    let visible_height = area.height as usize;
    let auto_bottom = lines.len().saturating_sub(visible_height);
    let scroll_row = auto_bottom.saturating_sub(scroll as usize) as u16;

    let mut para = Paragraph::new(lines).scroll((scroll_row, 0));
    if wrap {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
}

/// Fullscreen output for the highlighted module.
pub fn render_fullscreen_output(f: &mut Frame, area: Rect, app: &mut App) {
    let wrap = app.session.as_ref().map(|s| s.output_wrap).unwrap_or(false);
    let scroll = app.session.as_ref().map(|s| s.output_scroll).unwrap_or(0);

    let title = app
        .run_cursor_path()
        .and_then(|p| {
            let name = app
                .session
                .as_ref()
                .and_then(|s| s.modules.iter().find(|m| m.path == p))
                .map(|m| m.name.clone());
            let cmd = app.display_task_for(&p).map(|(t, _)| cmd_label(t));
            name.map(|n| format!(" {n} · {} ", cmd.unwrap_or_else(|| "—".to_string())))
        })
        .unwrap_or_else(|| " output ".to_string());

    let out = app.run_output_lines();
    let lines: Vec<Line> = out.iter().map(|l| parse_ansi(l)).collect();

    let inner_h = area.height.saturating_sub(2) as usize;
    let auto_bottom = lines.len().saturating_sub(inner_h);
    let scroll_row = auto_bottom.saturating_sub(scroll as usize) as u16;

    let scroll_tag = if scroll > 0 { " [↑ scrolled]" } else { "" };
    let mut para = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!("{title}{scroll_tag}"))
                .borders(Borders::ALL)
                .border_style(theme::dim()),
        )
        .scroll((scroll_row, 0));
    if wrap {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
    app.viewport.output = area.height.saturating_sub(2);
}

fn render_run_keybar(f: &mut Frame, area: Rect) {
    render_keybar(
        f,
        area,
        &[
            ("p", "plan"),
            ("a", "apply"),
            ("i", "init"),
            ("d", "destroy"),
            ("P/A", "one"),
            ("space", "subset"),
            ("C", "cancel"),
            ("enter", "output"),
            ("s", "state"),
            ("esc", "back"),
            ("?", "help"),
        ],
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Handle a key on the Run screen.
///
/// Scope rule for all lowercase action keys: the board multi-selected subset if
/// non-empty, else ALL session modules. The Shift variant (I/P/A/D) targets the
/// highlighted row only.
pub fn handle_key(app: &mut App, key: KeyEvent) -> ScreenAction {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('q') => return ScreenAction::Quit,
        KeyCode::Char('?') => app.modal = Some(crate::app::Modal::Help),
        KeyCode::Esc => app.screen = crate::app::Screen::Select,

        // Board cursor.
        KeyCode::Char('j') | KeyCode::Down => {
            app.run_move_cursor(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.run_move_cursor(-1);
        }
        KeyCode::Char('g') => {
            let n = session_len(app);
            app.run_move_cursor(-n);
        }
        KeyCode::Char('G') => {
            let n = session_len(app);
            app.run_move_cursor(n);
        }

        // Output scroll.
        KeyCode::PageDown => {
            app.run_scroll_output(-(app.viewport.output.max(1) as i32));
        }
        KeyCode::PageUp => {
            app.run_scroll_output(app.viewport.output.max(1) as i32);
        }

        // Board selection.
        KeyCode::Char(' ') => {
            if ctrl {
                app.run_board_range();
            } else {
                app.run_board_toggle();
            }
        }
        KeyCode::Char('*') => app.run_board_toggle_all(),
        KeyCode::Char('c') => app.run_board_clear(),

        // Fullscreen output.
        KeyCode::Enter => {
            if let Some(s) = app.session.as_mut() {
                s.fullscreen = true;
            }
            return ScreenAction::EnterFullscreen;
        }
        // Toggle output wrap.
        KeyCode::Char('w') => {
            if let Some(s) = app.session.as_mut() {
                s.output_wrap = !s.output_wrap;
            }
        }
        // State explorer for the cursor module.
        KeyCode::Char('s') => {
            if let Some(idx) = app.run_highlight_index() {
                app.open_state_explorer(idx);
            }
        }

        // Terraform actions — scope = subset-or-all; Shift = highlighted.
        KeyCode::Char('i') => {
            let t = app.run_scope_indices();
            let ids = app.enqueue_command("init", vec![], &t);
            app.record_session_tasks(&ids);
        }
        KeyCode::Char('I') => {
            let t = highlight(app);
            let ids = app.enqueue_command("init", vec![], &t);
            app.record_session_tasks(&ids);
        }
        KeyCode::Char('p') => {
            let t = app.run_scope_indices();
            let ids = app.enqueue_plan(&t);
            app.record_session_tasks(&ids);
        }
        KeyCode::Char('P') => {
            let t = highlight(app);
            let ids = app.enqueue_plan(&t);
            app.record_session_tasks(&ids);
        }
        KeyCode::Char('a') => {
            let t = app.run_scope_indices();
            app.request_apply_confirm(&t);
        }
        KeyCode::Char('A') => {
            let t = highlight(app);
            app.request_apply_confirm(&t);
        }
        KeyCode::Char('d') => {
            let t = app.run_scope_indices();
            app.request_destroy_confirm(&t);
        }
        KeyCode::Char('D') => {
            let t = highlight(app);
            app.request_destroy_confirm(&t);
        }
        KeyCode::Char('u') => {
            let t = app.run_scope_indices();
            app.request_init_upgrade_confirm(&t);
        }
        KeyCode::Char('U') => {
            let t = app.run_scope_indices();
            app.request_force_unlock_confirm(&t);
        }

        // Cancel active display tasks in scope.
        KeyCode::Char('C') => {
            let t = app.run_scope_indices();
            app.request_cancel_run_scope(&t);
        }
        // Clear completed history (engine-wide).
        KeyCode::Char('x') => app.request_clear_tasks_confirm(),

        _ => {}
    }
    ScreenAction::None
}

fn session_len(app: &App) -> i32 {
    app.session.as_ref().map(|s| s.modules.len()).unwrap_or(0) as i32
}

fn highlight(app: &App) -> Vec<usize> {
    app.run_highlight_index().into_iter().collect()
}
