//! Single-line key-hint bar rendered at the bottom of the Select and Run
//! screens. Hints are `(key, label)` pairs joined by two spaces; when the row
//! is too narrow the tail is dropped and an ellipsis appended.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    Frame,
};

use crate::ui::theme;

/// Render a keybar of `(key, label)` hints into a single-row `area`,
/// truncating with `…` when it does not fit.
pub fn render_keybar(f: &mut Frame, area: Rect, hints: &[(&str, &str)]) {
    if area.height == 0 {
        return;
    }
    let max = area.width as usize;
    let mut spans: Vec<Span> = Vec::new();
    let mut width = 0usize;
    let mut truncated = false;

    for (i, (key, label)) in hints.iter().enumerate() {
        let sep = if i == 0 { 0 } else { 2 };
        // Segment cells: separator + key + one space + label.
        let seg = sep + key.chars().count() + 1 + label.chars().count();
        // Reserve one cell for a trailing `…` if more hints remain.
        let reserve = if i + 1 < hints.len() { 1 } else { 0 };
        if width + seg + reserve > max {
            truncated = true;
            break;
        }
        if sep > 0 {
            spans.push(Span::raw("  "));
        }
        spans.extend(theme::key_hint(key, label));
        width += seg;
    }

    if truncated {
        spans.push(Span::styled("…", theme::dim()));
    }

    f.render_widget(ratatui::widgets::Paragraph::new(Line::from(spans)), area);
}
