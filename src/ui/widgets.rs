//! Small shared render helpers used by more than one screen.

use ratatui::{
    style::{Color, Style},
    text::Span,
};

use crate::task::ResourceCounts;
use crate::ui::theme;

/// Build coloured spans for resource operation counts.
///
/// Returns empty vec if there is nothing meaningful to show yet.
/// Shows `=` (dimmed) when a summary line confirmed no resource changes.
/// Shows coloured non-zero counts only:
///   +N  green   — add
///   ~N  yellow  — change
///   -N  red     — destroy
///   iN  cyan    — import
///   fN  gray    — forget (remove from state, OpenTofu)
pub fn count_spans(counts: &ResourceCounts) -> Vec<Span<'static>> {
    // No summary line seen yet — nothing to display.
    if !counts.has_summary && !counts.no_changes {
        return vec![];
    }

    // "No changes." or a real summary with everything at zero.
    if counts.no_changes || counts.all_zero() {
        return vec![Span::styled(
            "  =".to_string(),
            Style::default().fg(theme::COUNT_NONE),
        )];
    }

    let mut spans = Vec::new();

    let entries: &[(u32, &str, &str, Color)] = &[
        (counts.add, "+", "add", theme::COUNT_ADD),
        (counts.change, "~", "change", theme::COUNT_CHANGE),
        (counts.destroy, "-", "destroy", theme::COUNT_DESTROY),
        (counts.import, "i", "import", theme::COUNT_IMPORT),
        (counts.forget, "f", "forget", theme::COUNT_FORGET),
    ];

    for &(n, sym, label, color) in entries {
        if n > 0 {
            spans.push(Span::styled(
                format!("  {sym}{n} {label}"),
                Style::default().fg(color),
            ));
        }
    }

    spans
}
