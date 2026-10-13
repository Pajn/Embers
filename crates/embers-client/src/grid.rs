use std::borrow::Cow;
use std::fmt::Write;
use std::sync::Arc;

use embers_core::{
    CellAttrs, CursorShape, Hyperlink, Rect, SnapshotLine, StyledRun, TermColor, grapheme_width,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::host_terminal::HostCapabilities;

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

/// Underline shape. Discriminants match the SGR `4:n` sub-parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    #[default]
    None = 0,
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
}

impl UnderlineStyle {
    pub const fn is_none(self) -> bool {
        matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Option<TerminalColor>,
    pub bg: Option<TerminalColor>,
    pub bold: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
    /// Underline color (SGR 58); `None` follows the foreground.
    pub underline_color: Option<TerminalColor>,
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

    /// This style as a host terminal with `host`'s capabilities can show it.
    pub fn for_host(mut self, host: HostCapabilities) -> Self {
        if !host.underline_styles && !self.underline.is_none() {
            self.underline = UnderlineStyle::Single;
        }
        if !host.underline_color {
            self.underline_color = None;
        }
        self
    }
}

impl From<&crate::scripting::StyleSpec> for CellStyle {
    fn from(value: &crate::scripting::StyleSpec) -> Self {
        Self {
            fg: value.fg.map(|color| TerminalColor::Rgb(color.into())),
            bg: value.bg.map(|color| TerminalColor::Rgb(color.into())),
            bold: value.bold,
            italic: value.italic,
            underline: if value.underline {
                UnderlineStyle::Single
            } else {
                UnderlineStyle::None
            },
            underline_color: None,
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
    /// OSC 8 hyperlink from pane content. Lives on the cell rather than in
    /// [`CellStyle`] so overlays that restyle content keep the link, while
    /// anything drawn over the cell (borders, labels, popups) drops it.
    link: Option<Arc<Hyperlink>>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: " ".to_owned(),
            style: CellStyle::default(),
            continuation: false,
            link: None,
        }
    }
}

impl Cell {
    fn blank(fill: char) -> Self {
        Self {
            text: fill.to_string(),
            style: CellStyle::default(),
            continuation: false,
            link: None,
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

    /// The style of the cell at `x`, `y`, if it is inside the grid.
    pub fn cell_style(&self, x: u16, y: u16) -> Option<CellStyle> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = usize::from(y) * usize::from(self.width) + usize::from(x);
        self.cells.get(index).map(|cell| cell.style)
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
        self.put_str_linked(x, y, text, style, None);
    }

    fn put_str_linked(
        &mut self,
        x: u16,
        y: u16,
        text: &str,
        style: CellStyle,
        link: Option<&Arc<Hyperlink>>,
    ) {
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
            self.set_cell(x_pos, y, grapheme, style, width, link);
            x_pos = x_pos.saturating_add(width);
        }
    }

    /// Draw a styled snapshot line, mapping each cell to the style of the run
    /// that contains its first byte.
    ///
    /// Cells are the line's own ([`SnapshotLine::cells`]), so they cover the
    /// columns the pane's emulator gave them even where Unicode measures
    /// their text otherwise. Mirrors [`truncate`]'s column budget: when the
    /// line is wider than `width`, it draws up to `width - 1` columns then a
    /// default-styled `~` marker. Hidden runs draw spaces (keeping fg/bg) so
    /// the glyph is blanked. Linked runs keep their OSC 8 hyperlink on the
    /// drawn cells.
    pub fn put_snapshot_line(&mut self, x: u16, y: u16, width: u16, line: &SnapshotLine) {
        if width == 0 || y >= self.height || x >= self.width {
            return;
        }

        let truncated = line.width() > usize::from(width);
        let budget = if truncated {
            width.saturating_sub(1)
        } else {
            width
        };

        // Shared once per line so each linked cell holds a cheap handle.
        let links: Vec<Arc<Hyperlink>> = line.links.iter().cloned().map(Arc::new).collect();

        let mut column = 0_u16;
        let mut runs = RunCursor::new(&line.runs);
        for cell in line.cells() {
            if column.saturating_add(cell.width) > budget {
                break;
            }

            let run = runs.run_at(cell.offset);
            let style = run.map(style_for_run).unwrap_or_default();
            let hidden = run.is_some_and(|run| run.attrs.contains(CellAttrs::HIDDEN));
            let link = run
                .and_then(|run| usize::from(run.link).checked_sub(1))
                .and_then(|index| links.get(index));

            let draw_x = x.saturating_add(column);
            if hidden {
                let blank = " ".repeat(usize::from(cell.width));
                self.put_str_linked(draw_x, y, &blank, style, link);
            } else if draw_x.saturating_add(cell.width) <= self.width {
                self.clear_overlapping_cells(draw_x, y, cell.width);
                self.set_cell(draw_x, y, cell.text, style, cell.width, link);
            }

            column = column.saturating_add(cell.width);
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

    /// The rows as text with the escape sequences that style them, for a host
    /// terminal that supports everything Embers can emit.
    pub fn ansi_lines(&self) -> Vec<String> {
        self.ansi_lines_for(HostCapabilities::ALL)
    }

    /// The rows as text with escape sequences, for a host terminal with
    /// `host`'s capabilities: without underline styles every underline is
    /// plain, without underline color it follows the text, and without
    /// hyperlinks linked text is plain text. Each row is for the host's first
    /// column on: where the host could size a cell otherwise, the row puts
    /// the cursor back in its column (see `write_cell_text`).
    pub fn ansi_lines_for(&self, host: HostCapabilities) -> Vec<String> {
        (0..self.height)
            .map(|row| {
                let start = usize::from(row) * usize::from(self.width);
                let cells = &self.cells[start..start + usize::from(self.width)];
                let mut writer = RowWriter {
                    host,
                    output: String::new(),
                    style: CellStyle::default(),
                    link: None,
                };
                let mut column = 0;
                while column < cells.len() {
                    if cells[column].continuation {
                        column += 1;
                        continue;
                    }
                    if joined_next(cells, column).is_none() {
                        let width = cell_width(cells, column);
                        writer.cell(&cells[column], column, width);
                        column += width;
                    } else {
                        let joined = joined_cells(cells, column);
                        let end = joined
                            .last()
                            .map_or(column, |&last| last + cell_width(cells, last));
                        // A host that clusters graphemes joins a character to
                        // the one in the cell to its left, wherever the cursor
                        // went in between. Written right to left into cleared
                        // cells, each part has nothing to join.
                        let _ = write!(writer.output, "\x1b[{}G\x1b[{}X", column + 1, end - column);
                        for &part in joined.iter().rev() {
                            let _ = write!(writer.output, "\x1b[{}G", part + 1);
                            writer.cell(&cells[part], part, cell_width(cells, part));
                        }
                        let _ = write!(writer.output, "\x1b[{}G", end + 1);
                        column = end;
                    }
                }
                writer.finish()
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

    fn set_cell(
        &mut self,
        x: u16,
        y: u16,
        grapheme: &str,
        style: CellStyle,
        width: u16,
        link: Option<&Arc<Hyperlink>>,
    ) {
        let idx = self.index(x, y);
        self.cells[idx] = Cell {
            text: grapheme.to_owned(),
            style,
            continuation: false,
            link: link.cloned(),
        };

        for offset in 1..width {
            let idx = self.index(x + offset, y);
            self.cells[idx] = Cell {
                text: String::new(),
                style,
                continuation: true,
                link: link.cloned(),
            };
        }
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }
}

/// `text` with each control character replaced by a space. Text that comes
/// from programs in panes, such as titles set with OSC 2, goes through this
/// before it is drawn as chrome, so it cannot reach the host terminal as
/// escape sequences or move its cursor.
pub fn replace_controls(text: &str) -> Cow<'_, str> {
    if text.chars().any(char::is_control) {
        Cow::Owned(
            text.chars()
                .map(|ch| if ch.is_control() { ' ' } else { ch })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

/// Writes one row for the host terminal, tracking the style and link it has
/// set.
struct RowWriter<'a> {
    host: HostCapabilities,
    output: String,
    style: CellStyle,
    link: Option<&'a Arc<Hyperlink>>,
}

impl<'a> RowWriter<'a> {
    fn cell(&mut self, cell: &'a Cell, column: usize, width: usize) {
        let style = cell.style.for_host(self.host);
        write_style_transition(&mut self.output, self.style, style);
        self.style = style;
        if self.host.hyperlinks && cell.link.as_ref() != self.link {
            self.link = cell.link.as_ref();
            write_hyperlink(&mut self.output, self.link.map(Arc::as_ref));
        }
        write_cell_text(&mut self.output, &cell.text, column, width);
    }

    fn finish(mut self) -> String {
        // Links never span rows: the next row (or the status line) must not
        // inherit one.
        if self.link.is_some() {
            write_hyperlink(&mut self.output, None);
        }
        if !self.style.is_plain() {
            self.output.push_str("\x1b[0m");
        }
        self.output
    }
}

/// Columns the cell at `column` covers: it and its continuation cells.
fn cell_width(cells: &[Cell], column: usize) -> usize {
    1 + cells[column + 1..]
        .iter()
        .take_while(|cell| cell.continuation)
        .count()
}

/// The cell at `column` and those after it that each form one grapheme
/// cluster with the cell before: parts of a character a pane's emulator gave
/// cells of their own, such as alacritty with an emoji and its skin tone.
fn joined_cells(cells: &[Cell], column: usize) -> Vec<usize> {
    let mut joined = vec![column];
    while let Some(next) = joined_next(cells, joined[joined.len() - 1]) {
        joined.push(next);
    }
    joined
}

/// The column of the cell after the one at `column`, when the two form one
/// grapheme cluster.
fn joined_next(cells: &[Cell], column: usize) -> Option<usize> {
    let next = column + cell_width(cells, column);
    let cell = cells.get(next)?;
    let joins = !cell.text.is_ascii()
        && format!("{}{}", cells[column].text, cell.text)
            .graphemes(true)
            .nth(1)
            .is_none();
    joins.then_some(next)
}

/// Write a cell's text for the host terminal, which sizes the text by itself:
/// where it would not cover the cell's `width` columns, it could draw the
/// rest of the row in other columns than the grid.
///
/// Such a cell, which a pane's emulator laid out unlike Unicode measures it,
/// is erased first and the cursor put after it by column (`column` from the
/// row's start), so the row stays in its columns whatever the host makes of
/// the text. Text Unicode measures wider than the cell is drawn narrowed:
/// alacritty gives each part of an emoji sequence a narrow cell, and a host
/// drawing a part as wide would spill into the next cell. Hosts disagree on
/// a regional indicator on its own, half a flag: Unicode makes it one column
/// wide and Ghostty two, so one in a single column is drawn as U+FFFD.
fn write_cell_text(output: &mut String, text: &str, column: usize, width: usize) {
    if text.is_empty() {
        output.push(' ');
        return;
    }
    let measured = usize::from(grapheme_width(text));
    let regional = text.chars().any(is_regional_indicator);
    if measured == width && !regional {
        output.push_str(text);
        return;
    }
    let _ = write!(output, "\x1b[{width}X");
    if regional && width < 2 {
        output.push(REPLACEMENT);
    } else if measured <= width {
        output.push_str(text);
    } else {
        let narrowed: String = text
            .chars()
            .filter(|&ch| ch != EMOJI_PRESENTATION && ch != ZERO_WIDTH_JOINER)
            .collect();
        if usize::from(grapheme_width(&narrowed)) <= width {
            output.push_str(&narrowed);
        } else {
            output.extend(std::iter::repeat_n(REPLACEMENT, width));
        }
    }
    let _ = write!(output, "\x1b[{}G", column + width + 1);
}

fn is_regional_indicator(ch: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&ch)
}

/// Variation selector 16, which asks for a codepoint's emoji (wide) form.
const EMOJI_PRESENTATION: char = '\u{FE0F}';
const ZERO_WIDTH_JOINER: char = '\u{200D}';
const REPLACEMENT: char = '\u{FFFD}';

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

/// Map a content style run to a renderable [`CellStyle`].
pub(crate) fn style_for_run(run: &StyledRun) -> CellStyle {
    let attrs = run.attrs;
    CellStyle {
        fg: term_color_to_cell(run.fg),
        bg: term_color_to_cell(run.bg),
        bold: attrs.contains(CellAttrs::BOLD),
        italic: attrs.contains(CellAttrs::ITALIC),
        underline: underline_for_attrs(attrs),
        underline_color: term_color_to_cell(run.underline_color),
        dim: attrs.contains(CellAttrs::DIM),
        reverse: attrs.contains(CellAttrs::INVERSE),
        blink: false,
        strikeout: attrs.contains(CellAttrs::STRIKEOUT),
    }
}

fn underline_for_attrs(attrs: CellAttrs) -> UnderlineStyle {
    if !attrs.contains(CellAttrs::ANY_UNDERLINE) {
        UnderlineStyle::None
    } else if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
        UnderlineStyle::Curly
    } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
        UnderlineStyle::Dotted
    } else if attrs.contains(CellAttrs::DASHED_UNDERLINE) {
        UnderlineStyle::Dashed
    } else {
        UnderlineStyle::Single
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
    match to.underline {
        UnderlineStyle::None => {}
        // Plain `4` for a single underline: understood by every terminal.
        UnderlineStyle::Single => output.push_str("\x1b[4m"),
        style => {
            let _ = write!(output, "\x1b[4:{}m", style as u8);
        }
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
    // SGR 58 only has the colon form (`58:2::r:g:b`, `58:5:n`), the one kitty,
    // Ghostty, WezTerm, foot and VTE all accept.
    if let Some(color) = to.underline_color {
        match color {
            TerminalColor::Rgb(color) => {
                let _ = write!(
                    output,
                    "\x1b[58:2::{}:{}:{}m",
                    color.red, color.green, color.blue
                );
            }
            TerminalColor::Indexed(index) => {
                let _ = write!(output, "\x1b[58:5:{index}m");
            }
        }
    }
}

/// Open an OSC 8 hyperlink, or close the current one when `link` is `None`.
///
/// The id and URI come from programs in panes, so control characters are
/// dropped before they reach the host terminal: a stray BEL or ESC would end
/// the sequence early and let the rest run as terminal input. The id also
/// loses `:` and `;`, which delimit OSC 8 parameters.
fn write_hyperlink(output: &mut String, link: Option<&Hyperlink>) {
    output.push_str("\x1b]8;");
    if let Some(link) = link {
        if let Some(id) = link.id.as_deref().filter(|id| !id.is_empty()) {
            output.push_str("id=");
            output.extend(
                id.chars()
                    .filter(|ch| !ch.is_control() && *ch != ':' && *ch != ';'),
            );
        }
        output.push(';');
        output.extend(link.uri.chars().filter(|ch| !ch.is_control()));
    } else {
        output.push(';');
    }
    output.push_str("\x1b\\");
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

    pub const ROUNDED: Self = Self {
        top_left: '╭',
        top_right: '╮',
        bottom_left: '╰',
        bottom_right: '╯',
        horizontal: '─',
        vertical: '│',
    };

    pub const SINGLE: Self = Self {
        top_left: '┌',
        top_right: '┐',
        bottom_left: '└',
        bottom_right: '┘',
        horizontal: '─',
        vertical: '│',
    };

    pub const DOUBLE: Self = Self {
        top_left: '╔',
        top_right: '╗',
        bottom_left: '╚',
        bottom_right: '╝',
        horizontal: '═',
        vertical: '║',
    };

    pub const HEAVY: Self = Self {
        top_left: '┏',
        top_right: '┓',
        bottom_left: '┗',
        bottom_right: '┛',
        horizontal: '━',
        vertical: '┃',
    };

    pub const BLANK: Self = Self {
        top_left: ' ',
        top_right: ' ',
        bottom_left: ' ',
        bottom_right: ' ',
        horizontal: ' ',
        vertical: ' ',
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
                underline_color: TermColor::Default,
                link: 0,
            }],
            links: Vec::new(),
            cell_spans: Vec::new(),
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
                    underline_color: TermColor::Default,
                    link: 0,
                },
                StyledRun {
                    len: 1,
                    fg: TermColor::Default,
                    bg: TermColor::Default,
                    attrs: CellAttrs::empty(),
                    underline_color: TermColor::Default,
                    link: 0,
                },
            ],
            links: Vec::new(),
            cell_spans: Vec::new(),
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
