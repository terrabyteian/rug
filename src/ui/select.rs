//! Select screen: the full-window module picker. `/` search, Space multi-select,
//! Enter launches the Run screen for the current targets.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::app::{App, Modal, Screen};
use crate::ui::keybar::render_keybar;
use crate::ui::theme;
use crate::ui::widgets::count_spans;
use crate::ui::ScreenAction;

/// Render the Select screen into `area`, recording the list viewport height for
/// page navigation.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let show_spacer = area.height >= 20;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                            // header line 1
            Constraint::Length(1),                            // header line 2
            Constraint::Length(if show_spacer { 1 } else { 0 }), // spacer
            Constraint::Min(0),                               // module list
            Constraint::Length(1),                            // status line
            Constraint::Length(1),                            // keybar
        ])
        .split(area);
    let header1 = chunks[0];
    let header2 = chunks[1];
    let list_area = chunks[3];
    let status_area = chunks[4];
    let keybar_area = chunks[5];

    render_header(f, header1, header2, app);

    // Column width tiers (§2): ≥100 all, 80–99 drop command word, 60–79 drop
    // age, <60 name + spinner only.
    let w = area.width;
    let full = w >= 100;
    let show_age = w >= 80;
    let minimal = w < 60;

    let visible = app.visible_module_indices();
    let visible_count = visible.len();

    if visible.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  no modules match the current filter",
                theme::dim(),
            ))),
            list_area,
        );
    } else {
        let items: Vec<ListItem> = visible
            .iter()
            .enumerate()
            .map(|(pos, &real_idx)| {
                let module = &app.modules[real_idx];
                let is_cursor = pos == app.selected_module;
                let is_multi = app.multi_select.contains(&real_idx);

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
                if !minimal {
                    if is_multi {
                        spans.push(Span::styled("● ", theme::multi_select_marker()));
                    } else {
                        spans.push(Span::raw("  "));
                    }
                } else {
                    spans.push(Span::raw(" "));
                }
                // Name.
                let name_style = if is_multi {
                    theme::multi_select_item()
                } else {
                    Style::default()
                };
                spans.push(Span::styled(module.display_name.clone(), name_style));
                // Running activity indicator.
                if let Some((frame, command)) = app.module_activity(&module.path) {
                    let glyph = theme::SPINNER_FRAMES[frame];
                    spans.push(Span::styled(
                        format!("  {glyph}"),
                        Style::default().fg(theme::MARK),
                    ));
                    if full {
                        spans.push(Span::styled(
                            format!(" {command}"),
                            Style::default().fg(theme::MARK),
                        ));
                    }
                }
                // Cached-plan age (with a targeted marker for partial plans).
                if show_age {
                    if let Some(entry) = app.engine.plan_cache.get(&module.path) {
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
                }
                // Last-plan resource counts.
                if !minimal {
                    if let Some(counts) = app.ready_plan_counts(&module.path) {
                        spans.extend(count_spans(counts));
                    }
                }

                let line = Line::from(spans);
                if is_cursor {
                    ListItem::new(theme::lift_fg(line)).style(theme::row_cursor())
                } else {
                    ListItem::new(line)
                }
            })
            .collect();

        let mut state = ListState::default();
        state.select(Some(app.selected_module.min(visible_count.saturating_sub(1))));
        f.render_stateful_widget(List::new(items), list_area, &mut state);
        app.viewport.list_offset = state.offset() as u16;
    }

    // Status line.
    let k = app.multi_select.len();
    let status = if k > 0 {
        Line::from(Span::styled(
            format!("●{k} selected"),
            theme::multi_select_item(),
        ))
    } else {
        Line::from(Span::styled(
            format!("{visible_count} modules"),
            theme::dim(),
        ))
    };
    f.render_widget(Paragraph::new(status), status_area);

    // Keybar (or inline filter input).
    if app.filter_active {
        let line = Line::from(vec![
            Span::styled("/", Style::default().fg(theme::ACCENT)),
            Span::raw(app.filter.clone()),
            Span::styled("▌", Style::default().fg(theme::ACCENT)),
            Span::styled("  enter keep · esc clear", theme::dim()),
        ]);
        f.render_widget(Paragraph::new(line), keybar_area);
    } else {
        render_keybar(
            f,
            keybar_area,
            &[
                ("j/k", "move"),
                ("space", "select"),
                ("*", "all"),
                ("/", "filter"),
                ("enter", "run"),
                ("i/p/a/d", "act"),
                ("s", "state"),
                ("?", "help"),
                ("q", "quit"),
            ],
        );
    }

    app.viewport.list = list_area.height;
    app.viewport.list_top = list_area.y;
}

fn render_header(f: &mut Frame, header1: Rect, header2: Rect, app: &App) {
    // Line 1: app title + root.
    let h1 = Line::from(vec![
        Span::styled("rug", theme::app_title()),
        Span::raw("  "),
        Span::styled(app.root.to_string_lossy().into_owned(), theme::dim()),
    ]);
    f.render_widget(Paragraph::new(h1), header1);

    // Line 1 right: session indicator.
    if let Some((n, r)) = app.session_indicator() {
        let running = r > 0;
        let text = if header1.width < 80 {
            format!("{} {r}/{n} ⇥", if running { "⟳" } else { "✓" })
        } else if running {
            format!("⟳ session · {n} modules · {r} running  ⇥")
        } else {
            format!("✓ session · {n} modules · done  ⇥")
        };
        let style = if running {
            Style::default().fg(theme::MARK)
        } else {
            Style::default()
                .fg(theme::OK)
                .add_modifier(ratatui::style::Modifier::DIM)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style)))
                .alignment(ratatui::layout::Alignment::Right),
            header1,
        );
    }

    // Line 2: binary · N modules · depth/filter tags.
    let mut h2 = format!("{} · {} modules", app.config.binary, app.modules.len());
    if let Some(d) = app.max_depth {
        h2.push_str(&format!(" · [depth:{d}]"));
    }
    if !app.filter.is_empty() {
        h2.push_str(&format!(" · [/{}]", app.filter));
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(h2, theme::dim()))),
        header2,
    );
}

/// Keep `selected_module` within the visible list bounds.
fn clamp_selection(app: &mut App) {
    let count = app.visible_module_indices().len();
    if count == 0 {
        app.selected_module = 0;
    } else if app.selected_module >= count {
        app.selected_module = count - 1;
    }
}

/// Handle a key on the Select screen. Filter input is handled internally.
pub fn handle_key(app: &mut App, key: KeyEvent) -> ScreenAction {
    // Inline filter input mode.
    if app.filter_active {
        match key.code {
            KeyCode::Esc => {
                app.filter_active = false;
                app.filter.clear();
                clamp_selection(app);
            }
            KeyCode::Enter => {
                app.filter_active = false;
            }
            KeyCode::Backspace => {
                app.filter.pop();
                clamp_selection(app);
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                clamp_selection(app);
            }
            _ => {}
        }
        return ScreenAction::None;
    }

    match key.code {
        KeyCode::Char('q') => return ScreenAction::Quit,
        KeyCode::Char('?') => app.modal = Some(Modal::Help),

        // Navigation.
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_module_selection(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_module_selection(-1);
        }
        KeyCode::PageDown => {
            app.move_module_selection(app.viewport.list.max(1) as i32);
        }
        KeyCode::PageUp => {
            app.move_module_selection(-(app.viewport.list.max(1) as i32));
        }
        KeyCode::Char('g') => {
            app.selected_module = 0;
        }
        KeyCode::Char('G') => {
            let count = app.visible_module_indices().len();
            if count > 0 {
                app.selected_module = count - 1;
            }
        }

        // Selection.
        KeyCode::Char(' ') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                app.range_select();
            } else {
                app.toggle_multi_select();
            }
        }
        KeyCode::Char('*') => app.toggle_select_all_visible(),
        KeyCode::Char('c') => {
            app.multi_select.clear();
            app.max_depth = None;
        }

        // Filter.
        KeyCode::Char('/') => {
            app.filter_active = true;
            app.filter.clear();
        }
        KeyCode::Esc => {
            app.filter.clear();
            clamp_selection(app);
        }

        // Depth limiter.
        KeyCode::Char('[') => app.decrease_depth(),
        KeyCode::Char(']') => app.increase_depth(),

        // Refresh / reset.
        KeyCode::Char('r') => app.refresh_modules(),
        KeyCode::Char('R') => app.request_reset_confirm(),

        // State explorer for the highlighted module.
        KeyCode::Char('s') => {
            if let Some(&idx) = app.visible_module_indices().get(app.selected_module) {
                app.open_state_explorer(idx);
            }
        }

        // Enter → Run for the current targets.
        KeyCode::Enter => app.enter_run(),
        // Tab → resume the existing session (no-op if none).
        KeyCode::Tab => {
            if app.session.is_some() {
                app.screen = Screen::Run;
            }
        }

        // Action shortcuts: enter Run for the current targets, then trigger the
        // action all-scope by delegating the same key to the Run handler.
        KeyCode::Char('i')
        | KeyCode::Char('u')
        | KeyCode::Char('p')
        | KeyCode::Char('a')
        | KeyCode::Char('d')
        | KeyCode::Char('U') => {
            app.enter_run();
            if app.screen == Screen::Run {
                return crate::ui::run::handle_key(app, key);
            }
        }

        _ => {}
    }

    ScreenAction::None
}
