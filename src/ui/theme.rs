use crate::task::TaskStatus;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub const ACCENT: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;
pub const MARK: Color = Color::Yellow;
pub const OK: Color = Color::Green;
pub const ERR: Color = Color::Red;
pub const WARN: Color = Color::Magenta;

/// Cursor bar drawn to the left of the highlighted row.
pub const CURSOR_BAR: &str = "▍";

/// Application title style: accent, bold.
pub fn app_title() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Generic bold title style.
pub fn title() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Muted (secondary) text.
pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

/// Highlighted-row background (used on borderless list rows).
pub fn row_cursor() -> Style {
    Style::default().bg(MUTED).add_modifier(Modifier::BOLD)
}

/// Column-header style: dim, underlined.
pub fn col_header() -> Style {
    Style::default().fg(MUTED).add_modifier(Modifier::UNDERLINED)
}

/// A single key-hint fragment: bold accent key + muted label.
pub fn key_hint(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            key.to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}"), muted()),
    ]
}

/// Animated spinner frame for the given tick (Running and Cancelling both animate).
pub fn spinner(tick: u8) -> &'static str {
    SPINNER_FRAMES[(tick as usize / 2) % SPINNER_FRAMES.len()]
}

pub fn overlay_border_warn() -> Style {
    Style::default().fg(Color::Yellow)
} // cancel/clear/reset/quit/help
pub fn overlay_border_danger() -> Style {
    Style::default().fg(Color::Red)
} // destructive confirms
pub fn overlay_border_success() -> Style {
    Style::default().fg(Color::Green)
}
pub fn overlay_border_explorer() -> Style {
    Style::default().fg(Color::Blue)
}
pub fn overlay_border_filter() -> Style {
    Style::default().fg(ACCENT)
}

pub fn multi_select_marker() -> Style {
    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}
pub fn multi_select_item() -> Style {
    Style::default().fg(Color::Yellow)
}
/// Marker for a resource implicitly covered by a selected module prefix.
pub fn covered_marker() -> Style {
    Style::default().fg(MUTED)
}
pub fn plan_marker() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}
/// Marker for a cached plan that was scoped with `-target=` (partial plan).
pub fn plan_marker_targeted() -> Style {
    Style::default().fg(MARK).add_modifier(Modifier::BOLD)
}
pub fn command_text() -> Style {
    Style::default().fg(Color::Blue)
}
pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn status_style(status: &TaskStatus) -> Style {
    match status {
        TaskStatus::Pending => Style::default().fg(MUTED),
        TaskStatus::Running => Style::default().fg(MARK),
        TaskStatus::Cancelling => Style::default().fg(WARN),
        TaskStatus::Success => Style::default().fg(OK),
        TaskStatus::Failed => Style::default().fg(ERR).add_modifier(Modifier::BOLD),
        TaskStatus::Cancelled => Style::default().fg(MUTED),
    }
}

/// Presentation icon (moved out of the domain layer — delete TaskStatus::icon()).
pub fn status_icon(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "○",
        TaskStatus::Running => "⟳",
        TaskStatus::Cancelling => "◐", // overridden with animated frames in tasks.rs
        TaskStatus::Success => "✓",
        TaskStatus::Failed => "✗",
        TaskStatus::Cancelled => "⊘",
    }
}

pub const SPINNER_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];

pub const COUNT_ADD: Color = Color::Green;
pub const COUNT_CHANGE: Color = Color::Yellow;
pub const COUNT_DESTROY: Color = Color::Red;
pub const COUNT_IMPORT: Color = Color::Cyan;
pub const COUNT_FORGET: Color = Color::DarkGray;
pub const COUNT_NONE: Color = Color::DarkGray;
