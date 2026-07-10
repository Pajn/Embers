use std::fmt::Write;

use embers_core::{CellAttrs, CursorShape, Rect, SnapshotLine, StyledRun, TermColor};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl From<crate::scripting::RgbColor> for Color {
    fn from(value: crate::scripting::RgbColor) -> Self {
        Self {
            red: value.red,
            green: value.green,
            blue: value.blue,
        }
    }
}

/// A cell color as it travels toward SGR output.
///
/// Indexed colors emit `38;5;n` / `48;5;n` so the outer terminal's palette
/// resolves them; only true-color values carry explicit RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColor {
    Rgb(Color),
    Indexed(u8),
}

impl From<Color> for TerminalColor {
    fn from(value: Color) -> Self {
        Self::Rgb(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Option<TerminalColor>,
    pub bg: Option<TerminalColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub reverse: bool,
    pub blink: bool,
    pub strikeout: bool,
}

impl CellStyle {
    pub const fn with_reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    pub const fn with_bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub const fn with_italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub const fn with_blink(mut self) -> Self {
        self.blink = true;
        self
    }

    pub fn is_plain(self) -> bool {
        self == Self::default()
    }
}

impl From<&crate::scripting::StyleSpec> for CellStyle {
    fn from(value: &crate::scripting::StyleSpec) -> Self {
        Self {
            fg: value.fg.map(|color| TerminalColor::Rgb(color.into())),
            bg: value.bg.map(|color| TerminalColor::Rgb(color.into())),
            bold: value.bold,
            italic: value.italic,
            underline: value.underline,
            dim: value.dim,
            reverse: false,
            blink: value.blink,
            strikeout: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridCursor {
    pub x: u16,
    pub y: u16,
    pub shape: CursorShape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    text: String,
    style: CellStyle,
    continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: " ".to_owned(),
            style: CellStyle::default(),
            continuation: false,
        }
    }
}

impl Cell {
    fn blank(fill: char) -> Self {
        Self {
            text: fill.to_string(),
            style: CellStyle::default(),
            continuation: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderGrid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
    cursor: Option<GridCursor>,
}

impl RenderGrid {
    pub fn new(width: u16, height: u16) -> Self {
        let len = usize::from(width) * usize::from(height);
        Self {
            width,
            height,
            cells: vec![Cell::default(); len],
            cursor: None,
        }
    }

    pub fn clear(&mut self, fill: char) {
        self.cells.fill(Cell::blank(fill));
        self.cursor = None;
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cursor(&self) -> Option<GridCursor> {
        self.cursor
    }

    pub fn set_cursor(&mut self, cursor: Option<GridCursor>) {
        self.cursor = cursor.filter(|cursor| cursor.x < self.width && cursor.y < self.height);
    }

    pub fn put_char(&mut self, x: u16, y: u16, ch: char) {
        self.put_char_styled(x, y, ch, CellStyle::default());
    }

    pub fn put_char_styled(&mut self, x: u16, y: u16, ch: char, style: CellStyle) {
        self.put_str_styled(x, y, &ch.to_string(), style);
    }

    pub fn put_str(&mut self, x: u16, y: u16, text: &str) {
        self.put_str_styled(x, y, text, CellStyle::default());
    }

    pub fn put_str_styled(&mut self, x: u16, y: u16, text: &str, style: CellStyle) {
        if y >= self.height || x >= self.width {
            return;
        }

        let mut x_pos = x;
        for grapheme in UnicodeSegmentation::graphemes(text, true) {
            if x_pos >= self.width {
                break;
            }
            let width = grapheme_width(grapheme);
            if width == 0 {
                continue;
            }
            if x_pos.saturating_add(width) > self.width {
                break;
            }

            self.clear_overlapping_cells(x_pos, y, width);
            self.set_cell(x_pos, y, grapheme, style, width);
            x_pos = x_pos.saturating_add(width);
        }
    }

    /// Draw a styled snapshot line, mapping each grapheme to the style of the
    /// run that contains its first byte.
    ///
    /// Mirrors [`truncate`]'s column budget: when the text is wider than `width`,
    /// it draws up to `width - 1` columns then a default-styled `~` marker.
    /// Hidden runs draw spaces (keeping fg/bg) so the glyph is blanked.
    pub fn put_snapshot_line(&mut self, x: u16, y: u16, width: u16, line: &SnapshotLine) {
        if width == 0 || y >= self.height || x >= self.width {
            return;
        }

        let truncated = UnicodeWidthStr::width(line.text.as_str()) > usize::from(width);
        let budget = if truncated {
            width.saturating_sub(1)
        } else {
            width
        };

        let mut column = 0_u16;
        let mut byte_offset = 0_usize;
        let mut runs = RunCursor::new(&line.runs);
        for grapheme in UnicodeSegmentation::graphemes(line.text.as_str(), true) {
            let grapheme_width = grapheme_width(grapheme);
            if column.saturating_add(grapheme_width) > budget {
                break;
            }

            let run = runs.run_at(byte_offset);
            let style = run.map(style_for_run).unwrap_or_default();
            let hidden = run.is_some_and(|run| run.attrs.contains(CellAttrs::HIDDEN));

            let draw_x = x.saturating_add(column);
            if hidden {
                self.put_str_styled(draw_x, y, &" ".repeat(usize::from(grapheme_width)), style);
            } else {
                self.put_str_styled(draw_x, y, grapheme, style);
            }

            column = column.saturating_add(grapheme_width);
            byte_offset += grapheme.len();
        }

        if truncated {
            self.put_char_styled(x.saturating_add(column), y, '~', CellStyle::default());
        }
    }

    /// Recolor an existing span of cells in place by applying `restyle` to each
    /// cell's current style.
    ///
    /// Columns are pane-relative to `x`. The span snaps outward across wide-char
    /// continuation cells so a lead and its continuation are always restyled
    /// together. Used by overlays (selection, search) so they compose on top of
    /// content styling instead of replacing it.
    pub fn restyle_range(
        &mut self,
        x: u16,
        y: u16,
        start_col: u16,
        end_col: u16,
        restyle: impl Fn(CellStyle) -> CellStyle,
    ) {
        if y >= self.height || start_col >= end_col {
            return;
        }
        let abs_start = x.saturating_add(start_col).min(self.width);
        let abs_end = x.saturating_add(end_col).min(self.width);
        if abs_start >= abs_end {
            return;
        }

        let mut start = abs_start;
        while start > 0 && self.cells[self.index(start, y)].continuation {
            start -= 1;
        }
        let mut end = abs_end;
        while end < self.width && self.cells[self.index(end, y)].continuation {
            end += 1;
        }

        for column in start..end {
            let idx = self.index(column, y);
            self.cells[idx].style = restyle(self.cells[idx].style);
        }
    }

    pub fn draw_hline(&mut self, x: u16, y: u16, width: u16, ch: char) {
        self.draw_hline_styled(x, y, width, ch, CellStyle::default());
    }

    pub fn draw_hline_styled(&mut self, x: u16, y: u16, width: u16, ch: char, style: CellStyle) {
        for offset in 0..width {
            self.put_char_styled(x.saturating_add(offset), y, ch, style);
        }
    }

    pub fn draw_vline(&mut self, x: u16, y: u16, height: u16, ch: char) {
        self.draw_vline_styled(x, y, height, ch, CellStyle::default());
    }

    pub fn draw_vline_styled(&mut self, x: u16, y: u16, height: u16, ch: char, style: CellStyle) {
        for offset in 0..height {
            self.put_char_styled(x, y.saturating_add(offset), ch, style);
        }
    }

    pub fn fill_rect(&mut self, rect: Rect, fill: char, style: CellStyle) {
        let Some(rect) = self.clip_rect(rect) else {
            return;
        };
        let x = clamp_i32_to_u16(rect.origin.x);
        let y = clamp_i32_to_u16(rect.origin.y);
        for row in 0..rect.size.height {
            for col in 0..rect.size.width {
                self.put_char_styled(x.saturating_add(col), y.saturating_add(row), fill, style);
            }
        }
    }

    pub fn draw_box(&mut self, rect: Rect, border: BorderStyle) {
        self.draw_box_styled(rect, border, CellStyle::default());
    }

    pub fn draw_box_styled(&mut self, rect: Rect, border: BorderStyle, style: CellStyle) {
        let Some(rect) = self.clip_rect(rect) else {
            return;
        };

        let x = clamp_i32_to_u16(rect.origin.x);
        let y = clamp_i32_to_u16(rect.origin.y);
        let width = rect.size.width;
        let height = rect.size.height;
        let right = x.saturating_add(width.saturating_sub(1));
        let bottom = y.saturating_add(height.saturating_sub(1));

        self.put_char_styled(x, y, border.top_left, style);
        self.put_char_styled(right, y, border.top_right, style);
        self.put_char_styled(x, bottom, border.bottom_left, style);
        self.put_char_styled(right, bottom, border.bottom_right, style);

        if width > 2 {
            self.draw_hline_styled(x.saturating_add(1), y, width - 2, border.horizontal, style);
            self.draw_hline_styled(
                x.saturating_add(1),
                bottom,
                width - 2,
                border.horizontal,
                style,
            );
        }

        if height > 2 {
            self.draw_vline_styled(x, y.saturating_add(1), height - 2, border.vertical, style);
            self.draw_vline_styled(
                right,
                y.saturating_add(1),
                height - 2,
                border.vertical,
                style,
            );
        }
    }

    fn clip_rect(&self, rect: Rect) -> Option<Rect> {
        let left = rect.origin.x.max(0);
        let top = rect.origin.y.max(0);
        let right = (i64::from(rect.origin.x) + i64::from(rect.size.width))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let bottom = (i64::from(rect.origin.y) + i64::from(rect.size.height))
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let right = right.min(i32::from(self.width));
        let bottom = bottom.min(i32::from(self.height));

        if right <= left || bottom <= top {
            return None;
        }

        Some(Rect {
            origin: embers_core::Point { x: left, y: top },
            size: embers_core::Size {
                width: u16::try_from(right - left).unwrap_or(0),
                height: u16::try_from(bottom - top).unwrap_or(0),
            },
        })
    }

    pub fn lines(&self) -> Vec<String> {
        (0..self.height)
            .map(|row| {
                let start = usize::from(row) * usize::from(self.width);
                let end = start + usize::from(self.width);
                let mut output = String::new();
                for cell in &self.cells[start..end] {
                    if cell.continuation {
                        continue;
                    }
                    if cell.text.is_empty() {
                        output.push(' ');
                    } else {
                        output.push_str(&cell.text);
                    }
                }
                output
            })
            .collect()
    }

    pub fn ansi_lines(&self) -> Vec<String> {
        (0..self.height)
            .map(|row| {
                let start = usize::from(row) * usize::from(self.width);
                let end = start + usize::from(self.width);
                let mut output = String::new();
                let mut current_style = CellStyle::default();
                for cell in &self.cells[start..end] {
                    if cell.continuation {
                        continue;
                    }
                    write_style_transition(&mut output, current_style, cell.style);
                    current_style = cell.style;
                    if cell.text.is_empty() {
                        output.push(' ');
                    } else {
                        output.push_str(&cell.text);
                    }
                }
                if !current_style.is_plain() {
                    output.push_str("\x1b[0m");
                }
                output
            })
            .collect()
    }

    pub fn render(&self) -> String {
        self.lines().join("\n")
    }

    fn clear_overlapping_cells(&mut self, x: u16, y: u16, width: u16) {
        let mut start = x;
        while start > 0 && self.cells[self.index(start, y)].continuation {
            start -= 1;
        }

        let mut end = x.saturating_add(width);
        while end < self.width && self.cells[self.index(end, y)].continuation {
            end += 1;
        }

        for clear_x in start..end {
            let idx = self.index(clear_x, y);
            self.cells[idx] = Cell::default();
        }
    }

    fn set_cell(&mut self, x: u16, y: u16, grapheme: &str, style: CellStyle, width: u16) {
        let idx = self.index(x, y);
        self.cells[idx] = Cell {
            text: grapheme.to_owned(),
            style,
            continuation: false,
        };

        for offset in 1..width {
            let idx = self.index(x + offset, y);
            self.cells[idx] = Cell {
                text: String::new(),
                style,
                continuation: true,
            };
        }
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }
}

fn grapheme_width(grapheme: &str) -> u16 {
    let width = UnicodeWidthStr::width(grapheme);
    u16::try_from(width.max(1)).unwrap_or(u16::MAX)
}

/// Walks run-length style annotations in step with a byte cursor that only
/// advances, resolving the run covering a given byte offset in amortized O(1).
struct RunCursor<'a> {
    runs: &'a [StyledRun],
    index: usize,
    run_end: usize,
}

impl<'a> RunCursor<'a> {
    fn new(runs: &'a [StyledRun]) -> Self {
        let run_end = runs.first().map_or(0, |run| run.len as usize);
        Self {
            runs,
            index: 0,
            run_end,
        }
    }

    /// The run containing `byte_offset`, or `None` past the last run (plain).
    fn run_at(&mut self, byte_offset: usize) -> Option<&'a StyledRun> {
        while self.index < self.runs.len() && byte_offset >= self.run_end {
            self.index += 1;
            if let Some(run) = self.runs.get(self.index) {
                self.run_end += run.len as usize;
            }
        }
        self.runs.get(self.index)
    }
}

/// Map a content style run to a renderable [`CellStyle`]. Double underline folds
/// to a single underline (the client has no distinct double-underline SGR).
pub(crate) fn style_for_run(run: &StyledRun) -> CellStyle {
    let attrs = run.attrs;
    CellStyle {
        fg: term_color_to_cell(run.fg),
        bg: term_color_to_cell(run.bg),
        bold: attrs.contains(CellAttrs::BOLD),
        italic: attrs.contains(CellAttrs::ITALIC),
        underline: attrs.contains(CellAttrs::UNDERLINE)
            || attrs.contains(CellAttrs::DOUBLE_UNDERLINE),
        dim: attrs.contains(CellAttrs::DIM),
        reverse: attrs.contains(CellAttrs::INVERSE),
        blink: false,
        strikeout: attrs.contains(CellAttrs::STRIKEOUT),
    }
}

fn term_color_to_cell(color: TermColor) -> Option<TerminalColor> {
    match color {
        TermColor::Default => None,
        TermColor::Indexed(index) => Some(TerminalColor::Indexed(index)),
        TermColor::Rgb { r, g, b } => Some(TerminalColor::Rgb(Color {
            red: r,
            green: g,
            blue: b,
        })),
    }
}

fn write_style_transition(output: &mut String, from: CellStyle, to: CellStyle) {
    if from == to {
        return;
    }
    output.push_str("\x1b[0m");
    if to.bold {
        output.push_str("\x1b[1m");
    }
    if to.dim {
        output.push_str("\x1b[2m");
    }
    if to.italic {
        output.push_str("\x1b[3m");
    }
    if to.underline {
        output.push_str("\x1b[4m");
    }
    if to.blink {
        output.push_str("\x1b[5m");
    }
    if to.reverse {
        output.push_str("\x1b[7m");
    }
    if to.strikeout {
        output.push_str("\x1b[9m");
    }
    if let Some(fg) = to.fg {
        match fg {
            TerminalColor::Rgb(color) => {
                let _ = write!(
                    output,
                    "\x1b[38;2;{};{};{}m",
                    color.red, color.green, color.blue
                );
            }
            TerminalColor::Indexed(index) => {
                let _ = write!(output, "\x1b[38;5;{index}m");
            }
        }
    }
    if let Some(bg) = to.bg {
        match bg {
            TerminalColor::Rgb(color) => {
                let _ = write!(
                    output,
                    "\x1b[48;2;{};{};{}m",
                    color.red, color.green, color.blue
                );
            }
            TerminalColor::Indexed(index) => {
                let _ = write!(output, "\x1b[48;5;{index}m");
            }
        }
    }
}

fn clamp_i32_to_u16(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorderStyle {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
}

impl BorderStyle {
    pub const ASCII: Self = Self {
        top_left: '+',
        top_right: '+',
        bottom_left: '+',
        bottom_right: '+',
        horizontal: '-',
        vertical: '|',
    };

    pub const FOCUSED: Self = Self {
        top_left: '#',
        top_right: '#',
        bottom_left: '#',
        bottom_right: '#',
        horizontal: '#',
        vertical: '#',
    };
}

#[cfg(test)]
mod tests {
    use super::{Cell, CellStyle, Color, GridCursor, RenderGrid, TerminalColor};
    use embers_core::{
        CellAttrs, CursorShape, Point, Rect, Size, SnapshotLine, StyledRun, TermColor,
    };

    /// Read a cell directly (the test module descends from the grid module, so
    /// the private cell storage is visible here).
    fn cell(grid: &RenderGrid, x: u16, y: u16) -> &Cell {
        &grid.cells[grid.index(x, y)]
    }

    fn one_run_line(text: &str, fg: TermColor, attrs: u16) -> SnapshotLine {
        SnapshotLine {
            text: text.to_owned(),
            runs: vec![StyledRun {
                len: text.len() as u32,
                fg,
                bg: TermColor::Default,
                attrs: CellAttrs(attrs),
            }],
        }
    }

    #[test]
    fn put_snapshot_line_truncates_with_default_styled_marker() {
        let mut grid = RenderGrid::new(4, 1);
        // Whole line is styled red and overflows width 4, so it draws three
        // styled columns then a default-styled `~` in the reserved last column.
        grid.put_snapshot_line(0, 0, 4, &one_run_line("abcdef", TermColor::Indexed(5), 0));

        assert_eq!(grid.lines()[0], "abc~");
        assert_eq!(cell(&grid, 0, 0).text, "a");
        assert_eq!(
            cell(&grid, 0, 0).style.fg,
            Some(TerminalColor::Indexed(5)),
            "styled content keeps its color"
        );
        assert_eq!(cell(&grid, 3, 0).text, "~");
        assert_eq!(
            cell(&grid, 3, 0).style,
            CellStyle::default(),
            "the truncation marker is default-styled"
        );
    }

    #[test]
    fn put_snapshot_line_blanks_hidden_run_preserving_width() {
        let mut grid = RenderGrid::new(4, 1);
        // A hidden wide grapheme followed by a visible plain one. The wide char
        // is blanked to spaces but still occupies two columns, so `x` lands at
        // column 2; the blanked cells keep the run's foreground.
        let line = SnapshotLine {
            text: "界x".to_owned(),
            runs: vec![
                StyledRun {
                    len: "界".len() as u32,
                    fg: TermColor::Indexed(1),
                    bg: TermColor::Default,
                    attrs: CellAttrs(CellAttrs::HIDDEN),
                },
                StyledRun {
                    len: 1,
                    fg: TermColor::Default,
                    bg: TermColor::Default,
                    attrs: CellAttrs::empty(),
                },
            ],
        };
        grid.put_snapshot_line(0, 0, 4, &line);

        assert_eq!(grid.lines()[0], "  x ");
        assert_eq!(cell(&grid, 0, 0).text, " ");
        assert_eq!(cell(&grid, 1, 0).text, " ");
        assert!(
            !cell(&grid, 0, 0).continuation && !cell(&grid, 1, 0).continuation,
            "blanked cells are independent spaces, not a wide-char pair"
        );
        assert_eq!(cell(&grid, 0, 0).style.fg, Some(TerminalColor::Indexed(1)));
        assert_eq!(cell(&grid, 1, 0).style.fg, Some(TerminalColor::Indexed(1)));
        assert_eq!(cell(&grid, 2, 0).text, "x");
        assert_eq!(cell(&grid, 2, 0).style, CellStyle::default());
    }

    #[test]
    fn restyle_range_snaps_both_sides_of_a_wide_grapheme() {
        let mark = |style: CellStyle| CellStyle {
            reverse: true,
            ..style
        };

        // Snap the start backward: the range names only the continuation column
        // (2), so the lead (1) is pulled in and both halves are restyled.
        let mut grid = RenderGrid::new(6, 1);
        grid.put_str(0, 0, "a界b");
        grid.restyle_range(0, 0, 2, 3, mark);
        assert!(!cell(&grid, 0, 0).style.reverse, "'a' untouched");
        assert!(cell(&grid, 1, 0).style.reverse, "wide lead restyled");
        assert!(
            cell(&grid, 2, 0).style.reverse,
            "wide continuation restyled"
        );
        assert!(!cell(&grid, 1, 0).continuation && cell(&grid, 2, 0).continuation);
        assert!(!cell(&grid, 3, 0).style.reverse, "'b' untouched");

        // Snap the end forward: the range names only the lead column (1), so the
        // continuation (2) is pulled in and both halves are restyled.
        let mut grid = RenderGrid::new(6, 1);
        grid.put_str(0, 0, "a界b");
        grid.restyle_range(0, 0, 1, 2, mark);
        assert!(!cell(&grid, 0, 0).style.reverse, "'a' untouched");
        assert!(cell(&grid, 1, 0).style.reverse, "wide lead restyled");
        assert!(
            cell(&grid, 2, 0).style.reverse,
            "wide continuation restyled"
        );
        assert!(!cell(&grid, 3, 0).style.reverse, "'b' untouched");
    }

    #[test]
    fn render_preserves_plain_text_rows() {
        let mut grid = RenderGrid::new(6, 2);
        grid.put_str(1, 0, "embers");
        grid.put_str(0, 1, "ok");

        assert_eq!(grid.render(), " ember\nok    ");
    }

    #[test]
    fn ansi_lines_include_style_sequences() {
        let mut grid = RenderGrid::new(4, 1);
        grid.put_str_styled(
            0,
            0,
            "ab",
            CellStyle {
                fg: Some(TerminalColor::Rgb(Color {
                    red: 1,
                    green: 2,
                    blue: 3,
                })),
                bold: true,
                ..CellStyle::default()
            },
        );

        let line = &grid.ansi_lines()[0];
        assert!(line.contains("\x1b[1m"));
        assert!(line.contains("\x1b[38;2;1;2;3m"));
        assert!(line.contains("ab"));
    }

    #[test]
    fn ansi_lines_emit_indexed_and_strikeout() {
        let mut grid = RenderGrid::new(4, 1);
        grid.put_str_styled(
            0,
            0,
            "x",
            CellStyle {
                fg: Some(TerminalColor::Indexed(5)),
                bg: Some(TerminalColor::Indexed(12)),
                strikeout: true,
                ..CellStyle::default()
            },
        );

        let line = &grid.ansi_lines()[0];
        assert!(line.contains("\x1b[38;5;5m"), "line: {line:?}");
        assert!(line.contains("\x1b[48;5;12m"), "line: {line:?}");
        assert!(line.contains("\x1b[9m"), "line: {line:?}");
    }

    #[test]
    fn wide_graphemes_preserve_cell_alignment() {
        let mut grid = RenderGrid::new(4, 1);
        grid.put_str(0, 0, "界a");

        assert_eq!(grid.lines()[0], "界a ");
    }

    #[test]
    fn overwriting_a_wide_grapheme_clears_its_trailing_continuation() {
        let mut grid = RenderGrid::new(4, 1);
        grid.put_str(0, 0, "界");
        grid.put_char(0, 0, 'a');

        assert_eq!(grid.lines()[0], "a   ");
    }

    #[test]
    fn overwriting_inside_a_wide_grapheme_clears_the_lead_cell() {
        let mut grid = RenderGrid::new(4, 1);
        grid.put_str(0, 0, "界");
        grid.put_char(1, 0, 'a');

        assert_eq!(grid.lines()[0], " a  ");
    }

    #[test]
    fn cursor_is_clamped_to_the_grid() {
        let mut grid = RenderGrid::new(4, 2);
        grid.set_cursor(Some(GridCursor {
            x: 1,
            y: 1,
            shape: CursorShape::Beam,
        }));
        assert_eq!(
            grid.cursor(),
            Some(GridCursor {
                x: 1,
                y: 1,
                shape: CursorShape::Beam
            })
        );

        grid.set_cursor(Some(GridCursor {
            x: 5,
            y: 1,
            shape: CursorShape::Block,
        }));
        assert_eq!(grid.cursor(), None);
    }

    #[test]
    fn fill_rect_clips_negative_origin_to_visible_bounds() {
        let mut grid = RenderGrid::new(4, 2);
        grid.fill_rect(
            Rect {
                origin: Point { x: -1, y: 0 },
                size: Size {
                    width: 3,
                    height: 1,
                },
            },
            '#',
            CellStyle::default(),
        );

        assert_eq!(grid.lines(), vec!["##  ".to_owned(), "    ".to_owned()]);
    }
}
