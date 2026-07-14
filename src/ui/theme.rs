use crate::task::TaskStatus;
use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Cyan;

pub fn pane_border(focused: bool) -> Style {
    if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    }
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

pub fn selected_row() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}
pub fn selected_task_row() -> Style {
    Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
}
pub fn multi_select_marker() -> Style {
    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}
pub fn multi_select_item() -> Style {
    Style::default().fg(Color::Yellow)
}
pub fn plan_marker() -> Style {
    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
}
pub fn command_text() -> Style {
    Style::default().fg(Color::Blue)
}
pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn status_style(status: &TaskStatus) -> Style {
    match status {
        TaskStatus::Pending => Style::default().fg(Color::DarkGray),
        TaskStatus::Running => Style::default().fg(Color::Yellow),
        TaskStatus::Cancelling => Style::default().fg(Color::Magenta),
        TaskStatus::Success => Style::default().fg(Color::Green),
        TaskStatus::Failed => Style::default().fg(Color::Red),
        TaskStatus::Cancelled => Style::default().fg(Color::DarkGray),
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
