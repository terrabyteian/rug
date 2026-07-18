use crate::task::TaskStatus;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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

/// The brighter twin of `c`, for text drawn on the cursor row's `MUTED` bar.
///
/// `row_cursor()` paints bg = `MUTED` across the row and ratatui then draws each
/// span's fg over it (spans carry bg = None, so the bar survives underneath).
/// Any fg too dark to read against `MUTED` — above all `MUTED` itself — has to
/// be lifted. Already-bright colors and `Reset` pass through unchanged, which
/// makes `lift_color(fg) == fg` the invariant every cursor-row cell must hold.
pub(crate) fn lift_color(c: Color) -> Color {
    match c {
        Color::DarkGray => Color::Gray,
        Color::Black => Color::White,
        Color::Blue => Color::LightBlue,
        Color::Red => Color::LightRed,
        Color::Green => Color::LightGreen,
        Color::Yellow => Color::LightYellow,
        Color::Magenta => Color::LightMagenta,
        Color::Cyan => Color::LightCyan,
        other => other,
    }
}

/// Lift every span's fg to its bright twin, for a row about to be painted with
/// `row_cursor()`. Spans with no explicit fg are left alone — they inherit the
/// terminal default, which already contrasts with the bar.
pub fn lift_fg(mut line: Line<'static>) -> Line<'static> {
    for span in &mut line.spans {
        if let Some(fg) = span.style.fg {
            span.style.fg = Some(lift_color(fg));
        }
    }
    line
}

/// Full cursor-row treatment for a widget with no `ListItem` to carry the bar
/// (i.e. `Paragraph`): lift the fgs, pad to `width`, and hang `row_cursor()` off
/// the `Line`. A `Paragraph` only styles cells that hold a grapheme, so the
/// padding is what makes the bar span the row rather than stopping at the text.
/// A `List` needs none of this — `ListItem::style` fills the row itself.
pub fn cursor_row(line: Line<'static>, width: u16) -> Line<'static> {
    let mut line = lift_fg(line);
    let pad = (width as usize).saturating_sub(line.width());
    if pad > 0 {
        line.spans.push(Span::raw(" ".repeat(pad)));
    }
    line.style(row_cursor())
}

/// Column-header style: dim, underlined.
pub fn col_header() -> Style {
    Style::default()
        .fg(MUTED)
        .add_modifier(Modifier::UNDERLINED)
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
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}
pub fn multi_select_item() -> Style {
    Style::default().fg(Color::Yellow)
}
/// Marker for a resource implicitly covered by a selected module prefix.
pub fn covered_marker() -> Style {
    Style::default().fg(MUTED)
}
pub fn plan_marker() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}
/// Marker for a cached plan that was scoped with `-target=` (partial plan).
pub fn plan_marker_targeted() -> Style {
    Style::default().fg(MARK).add_modifier(Modifier::BOLD)
}
/// Plain `Blue` is the darkest non-black ANSI color and reads poorly on both the
/// cursor bar and dark terminals, so the command column starts out bright.
pub fn command_text() -> Style {
    Style::default().fg(Color::LightBlue)
}
pub fn dim() -> Style {
    muted()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour this theme can put in a foreground. `lift_color` is the one
    /// funnel cursor-row text passes through, so proving the mapping here covers
    /// every producer that feeds it.
    const PALETTE: &[Color] = &[
        ACCENT,
        MUTED,
        MARK,
        OK,
        ERR,
        WARN,
        COUNT_ADD,
        COUNT_CHANGE,
        COUNT_DESTROY,
        COUNT_IMPORT,
        COUNT_FORGET,
        COUNT_NONE,
        Color::Black,
        Color::Blue,
        Color::Reset,
        Color::White,
        Color::Gray,
        Color::LightBlue,
    ];

    /// The bug this guards: `row_cursor()` paints bg = MUTED, so any fg that
    /// stays MUTED is invisible on the highlighted row.
    #[test]
    fn lift_color_never_yields_the_cursor_background() {
        for &c in PALETTE {
            assert_ne!(
                lift_color(c),
                MUTED,
                "{c:?} lifts to the cursor bar's own colour — it would be invisible"
            );
        }
    }

    /// `lift_color(fg) == fg` is the invariant the render tests assert against
    /// every cell drawn on the bar, so lifting has to be a fixed point.
    #[test]
    fn lift_color_is_idempotent() {
        for &c in PALETTE {
            assert_eq!(lift_color(lift_color(c)), lift_color(c), "{c:?}");
        }
    }

    #[test]
    fn lift_fg_leaves_unstyled_spans_alone() {
        let line = Line::from(vec![
            Span::raw("plain"),
            Span::styled("muted", muted()),
            Span::styled("cmd", command_text()),
        ]);
        let lifted = lift_fg(line);
        assert_eq!(lifted.spans[0].style.fg, None, "no fg to lift");
        assert_eq!(lifted.spans[1].style.fg, Some(Color::Gray));
        assert_eq!(lifted.spans[2].style.fg, Some(Color::LightBlue));
    }

    /// A `Paragraph` only styles cells holding a grapheme, so the bar reaches the
    /// full row width only if the line is padded out to it.
    #[test]
    fn cursor_row_pads_to_the_full_width() {
        let row = cursor_row(Line::from(Span::raw("abc")), 10);
        assert_eq!(row.width(), 10);
        assert_eq!(row.style.bg, Some(MUTED));
    }

    /// Padding must never truncate or panic when the content already overflows.
    #[test]
    fn cursor_row_handles_content_wider_than_the_row() {
        let row = cursor_row(Line::from(Span::raw("abcdefghij")), 4);
        assert_eq!(row.width(), 10);
    }
}
