use std::fmt::Write;

use embers_core::{CursorShape, Rect};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub reverse: bool,
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

    pub fn is_plain(self) -> bool {
        self == Self::default()
    }
}

impl From<&crate::scripting::StyleSpec> for CellStyle {
    fn from(value: &crate::scripting::StyleSpec) -> Self {
        Self {
            fg: value.fg.map(Into::into),
            bg: value.bg.map(Into::into),
            bold: value.bold,
            italic: value.italic,
            underline: value.underline,
            dim: value.dim,
            reverse: false,
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
        let right = (rect.origin.x + i32::from(rect.size.width)).min(i32::from(self.width));
        let bottom = (rect.origin.y + i32::from(rect.size.height)).min(i32::from(self.height));

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
    if to.reverse {
        output.push_str("\x1b[7m");
    }
    if let Some(fg) = to.fg {
        let _ = write!(output, "\x1b[38;2;{};{};{}m", fg.red, fg.green, fg.blue);
    }
    if let Some(bg) = to.bg {
        let _ = write!(output, "\x1b[48;2;{};{};{}m", bg.red, bg.green, bg.blue);
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
    use super::{CellStyle, Color, GridCursor, RenderGrid};
    use embers_core::{CursorShape, Point, Rect, Size};

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
                fg: Some(Color {
                    red: 1,
                    green: 2,
                    blue: 3,
                }),
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
