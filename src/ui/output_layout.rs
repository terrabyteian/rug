//! Display-row layout for the Run screen's output pane.
//!
//! The pane shows a task's output as `Vec<String>` (ANSI escapes embedded);
//! with wrap on, one source line can occupy several display rows. This
//! module owns that char-index math so the renderer and the (future)
//! mouse-selection code agree on where every character lands.
//!
//! All ranges/indices are CHAR indices into the ANSI-stripped text of a
//! line, not byte offsets.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

use crate::ui::output;
use crate::util::strip_ansi;

/// Cache key for the layout of one task's output at one geometry.
#[derive(Clone, Copy, PartialEq, Eq)]
struct LayoutKey {
    task: Option<usize>,
    width: u16,
    wrap: bool,
}

/// Display-row layout of the output pane, cached across frames.
///
/// `sync` is cheap to call every frame: it rebuilds only when the task,
/// width, or wrap setting changes (or output was replaced/shrunk), and
/// otherwise extends incrementally for newly appended lines.
#[derive(Default)]
pub struct OutputLayout {
    key: Option<LayoutKey>,
    lines_seen: usize,
    /// cum_rows[i] = total display rows of lines[0..i]; len = lines_seen + 1;
    /// cum_rows[0] = 0.
    cum_rows: Vec<usize>,
}

impl OutputLayout {
    /// Bring the cache up to date with `lines` at the given geometry.
    ///
    /// Full rebuild when the key (task/width/wrap) differs or `lines` is
    /// shorter than what was last seen (output was cleared or replaced);
    /// otherwise extends for the newly appended tail, since output only
    /// ever grows by appending.
    pub fn sync(&mut self, task: Option<usize>, lines: &[String], width: u16, wrap: bool) {
        let key = LayoutKey { task, width, wrap };
        if self.key != Some(key) || lines.len() < self.lines_seen {
            self.key = Some(key);
            self.lines_seen = 0;
            self.cum_rows = vec![0];
        }
        if self.lines_seen < lines.len() {
            for line in &lines[self.lines_seen..] {
                let stripped = strip_ansi(line);
                let rows = row_count(&stripped, width, wrap);
                let last = *self.cum_rows.last().unwrap_or(&0);
                self.cum_rows.push(last + rows);
            }
            self.lines_seen = lines.len();
        }
    }

    /// Total display rows across all lines seen so far.
    pub fn total_rows(&self) -> usize {
        *self.cum_rows.last().unwrap_or(&0)
    }

    /// Map a display row to (source line index, row index within that line).
    /// `None` when `display_row >= total_rows()`.
    pub fn locate(&self, display_row: usize) -> Option<(usize, usize)> {
        if display_row >= self.total_rows() {
            return None;
        }
        let idx = self.cum_rows.partition_point(|&c| c <= display_row);
        let line = idx - 1;
        let row_in_line = display_row - self.cum_rows[line];
        Some((line, row_in_line))
    }

    /// The display row at which `line` begins.
    #[allow(dead_code)] // Tested; reserved for future scroll-to-selection features.
    pub fn first_row_of_line(&self, line: usize) -> usize {
        self.cum_rows
            .get(line)
            .copied()
            .unwrap_or_else(|| self.total_rows())
    }
}

/// Build exactly the viewport's rows: for each visible display row, parse the
/// source line once, wrap it, slice the row's span range, and overlay the
/// selection intersection reversed.
pub fn visible_lines(
    lines: &[String],
    layout: &OutputLayout,
    first_row: usize,
    height: usize,
    width: u16,
    wrap: bool,
    selection: Option<(crate::app::SelPos, crate::app::SelPos)>, // ordered (start <= end), char coords into stripped text
) -> Vec<ratatui::text::Line<'static>> {
    let mut out = Vec::with_capacity(height);
    let Some((mut line_idx, mut row_in_line)) = layout.locate(first_row) else {
        return out;
    };

    while out.len() < height {
        let Some(raw) = lines.get(line_idx) else {
            break;
        };
        let stripped = strip_ansi(raw);
        let parsed = output::parse_ansi(raw);
        let ranges = wrap_ranges(&stripped, width, wrap);
        let char_count = stripped.chars().count();

        // Char range of `line_idx` that falls inside the selection, if any.
        let sel_range: Option<Range<usize>> = selection.and_then(|(a, b)| {
            if line_idx < a.line || line_idx > b.line {
                return None;
            }
            let start = if line_idx == a.line { a.ch } else { 0 };
            let end = if line_idx == b.line { b.ch } else { char_count };
            Some(start..end)
        });

        while row_in_line < ranges.len() && out.len() < height {
            let row_range = ranges[row_in_line].clone();
            let sel = sel_range.clone().and_then(|s| {
                let lo = s.start.max(row_range.start);
                let hi = s.end.min(row_range.end);
                (lo < hi).then_some(lo..hi)
            });
            out.push(output::slice_spans(&parsed, row_range, sel));
            row_in_line += 1;
        }

        line_idx += 1;
        row_in_line = 0;
    }

    out
}

/// Char-index range of each display row of `stripped` at `width` cells.
///
/// With `wrap == false` the whole line is one row. An empty line always
/// occupies exactly one (empty) row. Wrapping is char-based (not
/// word-based): a row breaks before the first char whose width would
/// overflow the remaining cells, except a row is never left with zero
/// chars (a wide char wider than `width` still gets its own row).
#[allow(clippy::single_range_in_vec_init)] // These are single-element Vec<Range>, not iterator ranges.
pub fn wrap_ranges(stripped: &str, width: u16, wrap: bool) -> Vec<Range<usize>> {
    let char_count = stripped.chars().count();
    if !wrap {
        return vec![0..char_count];
    }
    if char_count == 0 {
        return vec![0..0];
    }

    let width_cells = width.max(1) as usize;
    let mut ranges = Vec::new();
    let mut row_start = 0usize;
    let mut cur_width = 0usize;
    for (i, ch) in stripped.chars().enumerate() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w == 0 {
            // Zero-width (combining) chars never start a row on their own.
            continue;
        }
        if cur_width > 0 && cur_width + w > width_cells {
            ranges.push(row_start..i);
            row_start = i;
            cur_width = 0;
        }
        cur_width += w;
    }
    ranges.push(row_start..char_count);
    ranges
}

/// Number of display rows `stripped` occupies at `width` cells; equivalent
/// to `wrap_ranges(stripped, width, wrap).len()` without allocating.
pub fn row_count(stripped: &str, width: u16, wrap: bool) -> usize {
    let char_count = stripped.chars().count();
    if !wrap || char_count == 0 {
        return 1;
    }

    let width_cells = width.max(1) as usize;
    let mut rows = 1usize;
    let mut cur_width = 0usize;
    for ch in stripped.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w == 0 {
            continue;
        }
        if cur_width > 0 && cur_width + w > width_cells {
            rows += 1;
            cur_width = 0;
        }
        cur_width += w;
    }
    rows
}

/// Char index within `range` of the char occupying display `cell` (0 = the
/// row's left edge). Either cell of a wide char returns that char's index.
/// A cell past the row's rendered width clamps to `range.end`.
pub fn char_at_cell(stripped: &str, range: Range<usize>, cell: usize) -> usize {
    let mut running = 0usize;
    for (i, ch) in stripped.chars().enumerate() {
        if i < range.start {
            continue;
        }
        if i >= range.end {
            break;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w == 0 {
            continue;
        }
        if cell < running + w {
            return i;
        }
        running += w;
    }
    range.end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn nowrap_identity() {
        let lines = make_lines(5);
        let mut layout = OutputLayout::default();
        layout.sync(Some(0), &lines, 80, false);
        assert_eq!(layout.total_rows(), 5);
        for i in 0..5 {
            assert_eq!(layout.locate(i), Some((i, 0)));
        }
        assert_eq!(layout.locate(5), None);
    }

    #[test]
    fn exact_wrap_ranges() {
        let line = "a".repeat(25);
        assert_eq!(wrap_ranges(&line, 10, true), vec![0..10, 10..20, 20..25]);
        assert_eq!(row_count(&line, 10, true), 3);
    }

    #[test]
    fn wide_chars_wrap() {
        let line = "日本語日本";
        assert_eq!(wrap_ranges(line, 4, true), vec![0..2, 2..4, 4..5]);
        assert_eq!(row_count(line, 4, true), 3);
    }

    #[test]
    fn wide_char_wraps_early_on_odd_boundary() {
        let line = "a日";
        assert_eq!(wrap_ranges(line, 2, true), vec![0..1, 1..2]);
        assert_eq!(row_count(line, 2, true), 2);
    }

    #[test]
    fn zero_width_combining_char_stays_attached() {
        let line = "e\u{0301}x";
        // "e" + combining acute accent + "x" = 3 chars.
        assert_eq!(wrap_ranges(line, 1, true), vec![0..2, 2..3]);
        assert_eq!(row_count(line, 1, true), 2);
    }

    #[test]
    fn empty_line_is_one_row() {
        assert_eq!(wrap_ranges("", 10, true), vec![0..0]);
        assert_eq!(wrap_ranges("", 10, false), vec![0..0]);
        assert_eq!(row_count("", 10, true), 1);
        assert_eq!(row_count("", 10, false), 1);
    }

    #[test]
    fn width_zero_treated_as_one() {
        assert_eq!(wrap_ranges("abc", 0, true), wrap_ranges("abc", 1, true));
    }

    #[test]
    fn oversized_wide_char_does_not_hang() {
        // width 1 cell, but "日" is 2 cells wide: must still make progress.
        assert_eq!(wrap_ranges("日", 1, true), vec![0..1]);
        assert_eq!(row_count("日", 1, true), 1);
    }

    #[test]
    fn char_at_cell_ascii() {
        let stripped = "hello";
        assert_eq!(char_at_cell(stripped, 0..5, 0), 0);
        assert_eq!(char_at_cell(stripped, 0..5, 2), 2);
        assert_eq!(char_at_cell(stripped, 0..5, 4), 4);
    }

    #[test]
    fn char_at_cell_wide_char_both_cells() {
        let stripped = "日";
        assert_eq!(char_at_cell(stripped, 0..1, 0), 0);
        assert_eq!(char_at_cell(stripped, 0..1, 1), 0);
    }

    #[test]
    fn char_at_cell_past_end_clamps() {
        let stripped = "日";
        assert_eq!(char_at_cell(stripped, 0..1, 5), 1);
    }

    #[test]
    fn incremental_sync_matches_fresh_rebuild() {
        let all_lines = make_lines(5);

        let mut incremental = OutputLayout::default();
        incremental.sync(Some(0), &all_lines[..3], 80, true);
        incremental.sync(Some(0), &all_lines, 80, true);

        let mut fresh = OutputLayout::default();
        fresh.sync(Some(0), &all_lines, 80, true);

        assert_eq!(incremental.total_rows(), fresh.total_rows());
        assert_eq!(incremental.cum_rows, fresh.cum_rows);
    }

    #[test]
    fn width_change_triggers_rebuild() {
        let lines = make_lines(3);
        let mut layout = OutputLayout::default();
        layout.sync(Some(0), &lines, 80, true);
        layout.sync(Some(0), &lines, 40, true);

        let mut fresh = OutputLayout::default();
        fresh.sync(Some(0), &lines, 40, true);

        assert_eq!(layout.cum_rows, fresh.cum_rows);
    }

    #[test]
    fn shrinking_lines_triggers_rebuild() {
        let all_lines = make_lines(5);
        let mut layout = OutputLayout::default();
        layout.sync(Some(0), &all_lines, 80, true);
        layout.sync(Some(0), &all_lines[..2], 80, true);

        let mut fresh = OutputLayout::default();
        fresh.sync(Some(0), &all_lines[..2], 80, true);

        assert_eq!(layout.total_rows(), fresh.total_rows());
        assert_eq!(layout.cum_rows, fresh.cum_rows);
    }

    #[test]
    fn locate_and_first_row_of_line_consistency() {
        let lines = vec![
            "short".to_string(),
            "a".repeat(25),
            "\x1b[32mgreen and long enough to wrap over ten cells\x1b[0m".to_string(),
            "".to_string(),
        ];
        let mut layout = OutputLayout::default();
        layout.sync(Some(0), &lines, 10, true);

        let mut expected_first_row = Vec::new();
        let mut running = 0usize;
        for line in &lines {
            expected_first_row.push(running);
            running += row_count(&strip_ansi(line), 10, true);
        }
        assert_eq!(layout.total_rows(), running);

        for (i, &first_row) in expected_first_row.iter().enumerate() {
            assert_eq!(layout.first_row_of_line(i), first_row);
            assert_eq!(layout.locate(first_row), Some((i, 0)));
        }
        assert_eq!(layout.locate(running), None);
    }
}
