use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Wrap a styled line to the available cell width, indenting continuation lines.
pub(crate) fn wrap_line(
    line: Line<'static>,
    width: u16,
    continuation_indent: usize,
    max_lines: usize,
) -> Vec<Line<'static>> {
    let width = width as usize;
    if width == 0 || max_lines == 0 {
        return vec![Line::default()];
    }

    let continuation_indent = if width > 1 {
        continuation_indent.min(width - 1)
    } else {
        0
    };

    let line_style = line.style;
    let alignment = line.alignment;
    let mut wrapped = Vec::new();
    let mut current = WrappedLine::new(width, None);

    for span in line.spans {
        append_wrapped_span(
            &mut wrapped,
            &mut current,
            span,
            width,
            continuation_indent,
            line_style,
            alignment,
            max_lines,
        );
        if wrapped.len() >= max_lines {
            return wrapped;
        }
    }

    if current.has_content || wrapped.is_empty() {
        wrapped.push(current.into_line(line_style, alignment));
    }

    wrapped.truncate(max_lines);
    wrapped
}

#[allow(clippy::too_many_arguments)]
fn append_wrapped_span(
    wrapped: &mut Vec<Line<'static>>,
    current: &mut WrappedLine,
    span: Span<'static>,
    first_width: usize,
    continuation_indent: usize,
    line_style: Style,
    alignment: Option<ratatui::layout::Alignment>,
    max_lines: usize,
) {
    let style = span.style;
    let mut parts = span.content.split('\n').peekable();

    while let Some(part) = parts.next() {
        for grapheme in UnicodeSegmentation::graphemes(part, true) {
            let grapheme_width = grapheme.width();
            if current.is_trimmable_leading_whitespace(grapheme) {
                continue;
            }

            if current.has_content && current.width + grapheme_width > current.available_width {
                current.trim_trailing_whitespace();
                wrapped.push(
                    std::mem::replace(
                        current,
                        WrappedLine::continuation(first_width, continuation_indent),
                    )
                    .into_line(line_style, alignment),
                );
                if wrapped.len() >= max_lines {
                    return;
                }
                if current.is_trimmable_leading_whitespace(grapheme) {
                    continue;
                }
            }

            current.push(grapheme, style, grapheme_width);
        }

        if parts.peek().is_some() {
            current.trim_trailing_whitespace();
            wrapped.push(
                std::mem::replace(
                    current,
                    WrappedLine::continuation(first_width, continuation_indent),
                )
                .into_line(line_style, alignment),
            );
            if wrapped.len() >= max_lines {
                return;
            }
        }
    }
}

struct WrappedLine {
    spans: Vec<Span<'static>>,
    width: usize,
    available_width: usize,
    indent_width: usize,
    has_content: bool,
}

impl WrappedLine {
    fn new(available_width: usize, indent: Option<usize>) -> Self {
        let mut line = Self {
            spans: Vec::new(),
            width: 0,
            available_width,
            indent_width: 0,
            has_content: false,
        };

        if let Some(indent) = indent.filter(|&indent| indent > 0) {
            line.spans.push(Span::raw(" ".repeat(indent)));
            line.width = indent;
            line.indent_width = indent;
        }

        line
    }

    fn continuation(first_width: usize, indent: usize) -> Self {
        Self::new(first_width, Some(indent))
    }

    fn is_trimmable_leading_whitespace(&self, grapheme: &str) -> bool {
        self.indent_width > 0 && !self.has_content && grapheme.chars().all(char::is_whitespace)
    }

    fn trim_trailing_whitespace(&mut self) {
        while self.width > self.indent_width {
            let Some(last) = self.spans.last_mut() else {
                break;
            };
            let Some(ch) = last.content.chars().next_back() else {
                self.spans.pop();
                continue;
            };
            if !ch.is_whitespace() {
                break;
            }

            let mut buf = [0; 4];
            self.width = self.width.saturating_sub(ch.encode_utf8(&mut buf).width());
            last.content.to_mut().pop();

            if last.content.is_empty() {
                self.spans.pop();
            }
        }

        self.has_content = self.width > self.indent_width;
    }

    fn push(&mut self, grapheme: &str, style: Style, width: usize) {
        if let Some(last) = self.spans.last_mut().filter(|span| span.style == style) {
            last.content.to_mut().push_str(grapheme);
        } else {
            self.spans.push(Span::styled(grapheme.to_string(), style));
        }
        self.width += width;
        self.has_content = true;
    }

    fn into_line(
        self,
        style: Style,
        alignment: Option<ratatui::layout::Alignment>,
    ) -> Line<'static> {
        Line {
            spans: self.spans,
            style,
            alignment,
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Color, text::Span};

    use super::*;

    #[test]
    fn wraps_long_lines_with_continuation_indent() {
        let lines = wrap_line(Line::from("abcdefghij"), 4, 2, 10);
        let rendered: Vec<String> = lines.into_iter().map(String::from).collect();

        assert_eq!(rendered, vec!["abcd", "  ef", "  gh", "  ij"]);
    }

    #[test]
    fn trims_padding_at_wrap_boundaries() {
        let lines = wrap_line(Line::from("ab    cd"), 4, 2, 10);
        let rendered: Vec<String> = lines.into_iter().map(String::from).collect();

        assert_eq!(rendered, vec!["ab", "  cd"]);
    }

    #[test]
    fn preserves_span_styles_across_wraps() {
        let style = Style::default().fg(Color::Green);
        let lines = wrap_line(Line::from(vec![Span::styled("abcdef", style)]), 3, 1, 10);

        assert_eq!(lines[0].spans[0].style, style);
        assert_eq!(lines[1].spans[1].style, style);
    }

    #[test]
    fn caps_wrapped_line_count() {
        let lines = wrap_line(Line::from("abcdefghij"), 3, 1, 2);
        let rendered: Vec<String> = lines.into_iter().map(String::from).collect();

        assert_eq!(rendered, vec!["abc", " de"]);
    }
}
