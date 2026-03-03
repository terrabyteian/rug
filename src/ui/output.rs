use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Focus};

/// Render the output viewer pane (scrollable log of the selected task).
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Output;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let lines: Vec<Line> = app
        .current_output()
        .iter()
        .map(|l| parse_ansi(l))
        .collect();

    let visible_height = area.height.saturating_sub(2) as usize;
    // auto_bottom keeps the tail visible; output_scroll pulls the view upward.
    let auto_bottom = lines.len().saturating_sub(visible_height);
    let scroll_row = auto_bottom.saturating_sub(app.output_scroll as usize) as u16;

    // Show a scroll indicator in the title when not at the bottom.
    let title = if app.output_scroll > 0 {
        format!(" {} [↑ scrolled] ", app.output_title())
    } else {
        format!(" {} ", app.output_title())
    };

    let mut para = Paragraph::new(lines)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .scroll((scroll_row, 0));

    if app.output_wrap {
        para = para.wrap(Wrap { trim: false });
    }

    f.render_widget(para, area);
}

/// Parse a raw string that may contain ANSI SGR escape sequences into a
/// ratatui `Line` made up of styled `Span`s.
///
/// Covers the subset of SGR codes that terraform/tofu actually emits:
/// resets, bold/dim, standard colours (30-37), bright colours (90-97),
/// and compound codes like `1;32`.
fn parse_ansi(s: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut text = String::new();

    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['

            // Collect up to the terminating letter.
            let mut seq = String::new();
            let mut terminator = 'm'; // default; we only care about SGR ('m')
            for ch in chars.by_ref() {
                if ch.is_ascii_alphabetic() {
                    terminator = ch;
                    break;
                }
                seq.push(ch);
            }

            if terminator == 'm' {
                // Flush accumulated text with current style before changing it.
                if !text.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut text), style));
                }
                style = apply_sgr(&seq, style);
            }
            // Non-SGR sequences (cursor movement etc.) are silently dropped.
        } else {
            text.push(c);
        }
    }

    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }

    Line::from(spans)
}

/// Apply one SGR escape sequence (the part between `\x1b[` and `m`) to a
/// `Style`, returning the updated style.  Handles compound codes like `1;32`.
fn apply_sgr(seq: &str, base: Style) -> Style {
    seq.split(';').fold(base, |s, part| match part.trim() {
        // Reset
        "" | "0" => Style::default(),
        // Attributes
        "1" => s.add_modifier(Modifier::BOLD),
        "2" => s.add_modifier(Modifier::DIM),
        "3" => s.add_modifier(Modifier::ITALIC),
        "4" => s.add_modifier(Modifier::UNDERLINED),
        "9" => s.add_modifier(Modifier::CROSSED_OUT),
        // Standard foreground colours (30-37)
        "30" => s.fg(Color::Black),
        "31" => s.fg(Color::Red),
        "32" => s.fg(Color::Green),
        "33" => s.fg(Color::Yellow),
        "34" => s.fg(Color::Blue),
        "35" => s.fg(Color::Magenta),
        "36" => s.fg(Color::Cyan),
        "37" => s.fg(Color::White),
        "39" => s.fg(Color::Reset),
        // Standard background colours (40-47)
        "40" => s.bg(Color::Black),
        "41" => s.bg(Color::Red),
        "42" => s.bg(Color::Green),
        "43" => s.bg(Color::Yellow),
        "44" => s.bg(Color::Blue),
        "45" => s.bg(Color::Magenta),
        "46" => s.bg(Color::Cyan),
        "47" => s.bg(Color::White),
        "49" => s.bg(Color::Reset),
        // Bright foreground colours (90-97)
        "90" => s.fg(Color::DarkGray),
        "91" => s.fg(Color::LightRed),
        "92" => s.fg(Color::LightGreen),
        "93" => s.fg(Color::LightYellow),
        "94" => s.fg(Color::LightBlue),
        "95" => s.fg(Color::LightMagenta),
        "96" => s.fg(Color::LightCyan),
        "97" => s.fg(Color::Gray),
        // Bright background colours (100-107)
        "100" => s.bg(Color::DarkGray),
        "101" => s.bg(Color::LightRed),
        "102" => s.bg(Color::LightGreen),
        "103" => s.bg(Color::LightYellow),
        "104" => s.bg(Color::LightBlue),
        "105" => s.bg(Color::LightMagenta),
        "106" => s.bg(Color::LightCyan),
        "107" => s.bg(Color::Gray),
        // Unknown — leave style unchanged.
        _ => s,
    })
}
