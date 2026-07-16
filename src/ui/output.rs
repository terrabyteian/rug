use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Parse a raw string that may contain ANSI SGR escape sequences into a
/// ratatui `Line` made up of styled `Span`s.
///
/// Covers the subset of SGR codes that terraform/tofu actually emits:
/// resets, bold/dim, standard colours (30-37), bright colours (90-97),
/// and compound codes like `1;32`.
pub(crate) fn parse_ansi(s: &str) -> Line<'static> {
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

/// Sub-line of `line` covering char range `range`, with the intersection of
/// `sel` restyled reversed. Both ranges are char indices over the
/// concatenation of the line's span contents (= the ANSI-stripped text).
pub(crate) fn slice_spans(
    line: &Line<'static>,
    range: std::ops::Range<usize>,
    sel: Option<std::ops::Range<usize>>,
) -> Line<'static> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut offset = 0usize;

    for span in line.spans.iter() {
        let chars: Vec<char> = span.content.chars().collect();
        let span_start = offset;
        let span_end = span_start + chars.len();
        offset = span_end;

        // Intersect this span's char range with the requested `range`.
        let lo = range.start.max(span_start);
        let hi = range.end.min(span_end);
        if lo >= hi {
            continue;
        }

        // Split the kept portion further at any `sel` boundaries that fall
        // strictly inside it, so each emitted fragment is either wholly
        // inside or wholly outside the selection.
        let mut points = vec![lo, hi];
        if let Some(sel) = &sel {
            if sel.start > lo && sel.start < hi {
                points.push(sel.start);
            }
            if sel.end > lo && sel.end < hi {
                points.push(sel.end);
            }
        }
        points.sort_unstable();
        points.dedup();

        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b {
                continue;
            }
            let text: String = chars[(a - span_start)..(b - span_start)].iter().collect();
            let mut style = span.style;
            if let Some(sel) = &sel {
                if a >= sel.start && b <= sel.end {
                    style = style.add_modifier(Modifier::REVERSED);
                }
            }
            out.push(Span::styled(text, style));
        }
    }

    Line::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_mid_span() {
        let line = Line::from(vec![Span::styled(
            "hello world".to_string(),
            Style::default().fg(Color::Green),
        )]);
        let sliced = slice_spans(&line, 3..8, None);
        assert_eq!(sliced.spans.len(), 1);
        assert_eq!(sliced.spans[0].content, "lo wo");
        assert_eq!(sliced.spans[0].style, Style::default().fg(Color::Green));
    }

    #[test]
    fn slice_across_span_boundary() {
        let line = Line::from(vec![
            Span::styled("abc".to_string(), Style::default().fg(Color::Red)),
            Span::styled("defgh".to_string(), Style::default().fg(Color::Blue)),
        ]);
        // Straddle the boundary: "bc" from the first span, "def" from the second.
        let sliced = slice_spans(&line, 1..6, None);
        assert_eq!(sliced.spans.len(), 2);
        assert_eq!(sliced.spans[0].content, "bc");
        assert_eq!(sliced.spans[0].style, Style::default().fg(Color::Red));
        assert_eq!(sliced.spans[1].content, "def");
        assert_eq!(sliced.spans[1].style, Style::default().fg(Color::Blue));
    }

    #[test]
    fn slice_full_range_equals_whole_line() {
        let line = Line::from(vec![Span::styled(
            "abcdef".to_string(),
            Style::default().fg(Color::Yellow),
        )]);
        let sliced = slice_spans(&line, 0..6, None);
        assert_eq!(sliced.spans.len(), 1);
        assert_eq!(sliced.spans[0].content, "abcdef");
    }

    #[test]
    fn slice_empty_range_produces_empty_line() {
        let line = Line::from(vec![Span::styled(
            "abcdef".to_string(),
            Style::default().fg(Color::Yellow),
        )]);
        let sliced = slice_spans(&line, 3..3, None);
        assert!(sliced.spans.is_empty());
    }

    #[test]
    fn slice_range_end_beyond_text_clamps() {
        let line = Line::from(vec![Span::styled(
            "abc".to_string(),
            Style::default().fg(Color::Yellow),
        )]);
        let sliced = slice_spans(&line, 1..1000, None);
        assert_eq!(sliced.spans.len(), 1);
        assert_eq!(sliced.spans[0].content, "bc");
    }

    #[test]
    fn slice_selection_overlay_partial() {
        let line = Line::from(vec![Span::styled(
            "hello world".to_string(),
            Style::default().fg(Color::Green),
        )]);
        // Range covers "hello world" (0..11); sel covers "llo w" (2..7).
        let sliced = slice_spans(&line, 0..11, Some(2..7));
        assert_eq!(sliced.spans.len(), 3);
        assert_eq!(sliced.spans[0].content, "he");
        assert!(!sliced.spans[0]
            .style
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(sliced.spans[0].style.fg, Some(Color::Green));

        assert_eq!(sliced.spans[1].content, "llo w");
        assert!(sliced.spans[1]
            .style
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(sliced.spans[1].style.fg, Some(Color::Green));

        assert_eq!(sliced.spans[2].content, "orld");
        assert!(!sliced.spans[2]
            .style
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(sliced.spans[2].style.fg, Some(Color::Green));
    }

    #[test]
    fn slice_selection_outside_range_no_reverse() {
        let line = Line::from(vec![Span::styled(
            "hello world".to_string(),
            Style::default(),
        )]);
        // sel is entirely outside the kept range.
        let sliced = slice_spans(&line, 0..5, Some(6..11));
        assert!(sliced
            .spans
            .iter()
            .all(|s| !s.style.add_modifier.contains(Modifier::REVERSED)));
    }

    #[test]
    fn slice_selection_none_no_reverse() {
        let line = Line::from(vec![Span::styled(
            "hello world".to_string(),
            Style::default(),
        )]);
        let sliced = slice_spans(&line, 0..11, None);
        assert!(sliced
            .spans
            .iter()
            .all(|s| !s.style.add_modifier.contains(Modifier::REVERSED)));
    }

    #[test]
    fn slice_multibyte() {
        let line = Line::from(vec![Span::styled(
            "日本語".to_string(),
            Style::default().fg(Color::Cyan),
        )]);
        let sliced = slice_spans(&line, 1..3, None);
        assert_eq!(sliced.spans.len(), 1);
        assert_eq!(sliced.spans[0].content, "本語");
    }

    /// Coordinate-space invariant: `slice_spans` walks char offsets over the
    /// concatenation of `parse_ansi`'s span contents, and callers will index
    /// into that same space using `strip_ansi`. The two must agree on what
    /// the "text" is, char-for-char, or a wrapped display row and its
    /// selection highlight will land on the wrong characters.
    #[test]
    fn parse_ansi_and_strip_ansi_agree_on_text() {
        let cases = [
            "plain text",
            "\x1b[1;32mgreen\x1b[0m mixed",
            "pre\x1b[2Kpost",
            "trailing\x1b[3",
        ];
        for s in cases {
            let parsed_text: String = parse_ansi(s)
                .spans
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect();
            let stripped = crate::util::strip_ansi(s);
            assert_eq!(
                parsed_text, stripped,
                "parse_ansi/strip_ansi text mismatch for {s:?}"
            );
        }
    }
}
