use std::collections::BTreeMap;

use embers_core::{ActivityState, Point, Rect, Size};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::grid::{BorderStyle, CellStyle, GridCursor, RenderGrid};
use crate::presentation::{DividerFrame, FloatingFrame, LeafFrame, PresentationModel, TabsFrame};
use crate::scripting::{BarSegment, BarSpec};
use crate::state::{ClientState, SelectionKind, SelectionPoint, SelectionState};

#[derive(Clone, Copy, Debug, Default)]
pub struct Renderer;

impl Renderer {
    pub fn render(&self, state: &ClientState, model: &PresentationModel) -> RenderGrid {
        self.render_with_tab_bars(state, model, &BTreeMap::new())
    }

    pub fn render_with_tab_bars(
        &self,
        state: &ClientState,
        model: &PresentationModel,
        tab_bars: &BTreeMap<embers_core::NodeId, BarSpec>,
    ) -> RenderGrid {
        let mut grid = RenderGrid::new(model.viewport.width, model.viewport.height);
        self.render_into_with_tab_bars(state, model, &mut grid, tab_bars);
        grid
    }

    pub fn render_into(
        &self,
        state: &ClientState,
        model: &PresentationModel,
        grid: &mut RenderGrid,
    ) {
        self.render_into_with_tab_bars(state, model, grid, &BTreeMap::new());
    }

    pub fn render_into_with_tab_bars(
        &self,
        state: &ClientState,
        model: &PresentationModel,
        grid: &mut RenderGrid,
        tab_bars: &BTreeMap<embers_core::NodeId, BarSpec>,
    ) {
        grid.clear(' ');
        self.render_layer(state, model, grid, None, tab_bars);

        for window in &model.floating {
            self.render_floating_frame(grid, window);
            self.render_layer(state, model, grid, Some(window.floating_id), tab_bars);
        }
    }

    fn render_layer(
        &self,
        state: &ClientState,
        model: &PresentationModel,
        grid: &mut RenderGrid,
        floating_id: Option<embers_core::FloatingId>,
        tab_bars: &BTreeMap<embers_core::NodeId, BarSpec>,
    ) {
        for leaf in model
            .leaves
            .iter()
            .filter(|leaf| leaf.floating_id == floating_id)
        {
            self.render_leaf(state, grid, leaf);
        }

        for divider in model
            .dividers
            .iter()
            .filter(|divider| divider.floating_id == floating_id)
        {
            self.render_divider(grid, divider);
        }

        for tabs in model
            .tab_bars
            .iter()
            .filter(|tabs| tabs.floating_id == floating_id)
        {
            self.render_tabs(grid, tabs, tab_bars.get(&tabs.node_id));
        }
    }

    fn render_tabs(&self, grid: &mut RenderGrid, tabs: &TabsFrame, custom: Option<&BarSpec>) {
        if tabs.rect.size.height == 0 || tabs.rect.size.width == 0 {
            return;
        }

        let mut x = clamp_i32_to_u16(tabs.rect.origin.x);
        let y = clamp_i32_to_u16(tabs.rect.origin.y);
        let end_x = x.saturating_add(tabs.rect.size.width);
        grid.put_str(x, y, &" ".repeat(usize::from(tabs.rect.size.width)));

        if let Some(bar) = custom {
            let width = tabs.rect.size.width;
            render_bar_segments(grid, x, y, end_x, &bar.left);
            let right_width = bar
                .right
                .iter()
                .map(|segment| display_width(&segment.text))
                .sum::<u16>();
            let right_x = end_x.saturating_sub(right_width.min(width));
            render_bar_segments(grid, right_x, y, end_x, &bar.right);

            let center_width = bar
                .center
                .iter()
                .map(|segment| display_width(&segment.text))
                .sum::<u16>();
            let center_x = x.saturating_add(width.saturating_sub(center_width.min(width)) / 2);
            render_bar_segments(grid, center_x, y, end_x, &bar.center);
            return;
        }

        for tab in &tabs.tabs {
            if x >= end_x {
                break;
            }

            let available = end_x.saturating_sub(x);
            if available == 0 {
                break;
            }

            let label = format_tab_label(tab, available);
            grid.put_str_styled(x, y, &label, tab_style(tab.active));
            x = x.saturating_add(display_width(&label));

            if x < end_x {
                grid.put_char(x, y, ' ');
                x += 1;
            }
        }
    }

    fn render_leaf(&self, state: &ClientState, grid: &mut RenderGrid, leaf: &LeafFrame) {
        if leaf.rect.size.width == 0 || leaf.rect.size.height == 0 {
            return;
        }

        let x = clamp_i32_to_u16(leaf.rect.origin.x);
        let y = clamp_i32_to_u16(leaf.rect.origin.y);
        let width = leaf.rect.size.width;
        let height = leaf.rect.size.height;
        let blank_line = " ".repeat(usize::from(width));
        for row in 0..height {
            grid.put_str(x, y + row, &blank_line);
        }

        let activity = activity_marker(leaf.activity);
        let title = truncate(
            &format!(
                "{}{} {}",
                if leaf.focused { '>' } else { ' ' },
                activity,
                leaf.title
            ),
            width,
        );
        grid.put_str_styled(x, y, &title, leaf_title_style(leaf.focused));

        if height <= 1 {
            return;
        }

        let view_state = state.view_state(leaf.node_id);
        let lines = view_state
            .map(|view| view.visible_lines.as_slice())
            .filter(|lines| !lines.is_empty())
            .or_else(|| {
                state
                    .snapshots
                    .get(&leaf.buffer_id)
                    .map(|snapshot| snapshot.lines.as_slice())
            });

        if let Some(lines) = lines {
            for (row, line) in lines
                .iter()
                .take(usize::from(height.saturating_sub(1)))
                .enumerate()
            {
                let Some(row) = u16::try_from(row).ok() else {
                    break;
                };
                grid.put_str(x, y + 1 + row, &truncate(line, width));
            }
        }

        if let Some(view_state) = view_state
            && !view_state.alternate_screen
        {
            if let Some(search_state) = &view_state.search_state {
                render_search_overlay(
                    grid,
                    x,
                    y + 1,
                    width,
                    view_state.scroll_top_line,
                    &view_state.visible_lines,
                    search_state,
                );
            }
            if let Some(selection_state) = &view_state.selection_state {
                render_selection_overlay(
                    grid,
                    x,
                    y + 1,
                    width,
                    view_state.scroll_top_line,
                    &view_state.visible_lines,
                    selection_state,
                );
            }
            if !view_state.follow_output {
                render_scroll_indicator(
                    grid,
                    x,
                    y,
                    width,
                    view_state.scroll_top_line,
                    view_state.total_line_count,
                );
            }
        }

        if let Some(snapshot) = state.snapshots.get(&leaf.buffer_id)
            && leaf.focused
        {
            let locally_scrolled = view_state.is_some_and(|view| {
                !view.alternate_screen && view.scroll_top_line != snapshot.viewport_top_line
            });
            let selection_active = view_state.is_some_and(|view| view.selection_state.is_some());
            if !locally_scrolled
                && !selection_active
                && let Some(cursor) = snapshot.cursor
                && cursor.position.col < width
            {
                let cursor_y = y + 1 + cursor.position.row;
                if cursor_y < y.saturating_add(height) {
                    grid.set_cursor(Some(GridCursor {
                        x: x + cursor.position.col,
                        y: cursor_y,
                        shape: cursor.shape,
                    }));
                }
            }
        }
    }

    fn render_divider(&self, grid: &mut RenderGrid, divider: &DividerFrame) {
        if divider.rect.size.width == 0 || divider.rect.size.height == 0 {
            return;
        }

        let x = clamp_i32_to_u16(divider.rect.origin.x);
        let y = clamp_i32_to_u16(divider.rect.origin.y);
        match divider.direction {
            embers_core::SplitDirection::Horizontal => {
                grid.draw_hline_styled(x, y, divider.rect.size.width, '-', divider_style());
            }
            embers_core::SplitDirection::Vertical => {
                grid.draw_vline_styled(x, y, divider.rect.size.height, '|', divider_style());
            }
        }
    }

    fn render_floating_frame(&self, grid: &mut RenderGrid, floating: &FloatingFrame) {
        let blank_line = " ".repeat(usize::from(floating.rect.size.width));
        let x = clamp_i32_to_u16(floating.rect.origin.x);
        let y = clamp_i32_to_u16(floating.rect.origin.y);
        for row in 0..floating.rect.size.height {
            grid.put_str(x, y + row, &blank_line);
        }

        let border = if floating.focused {
            BorderStyle::FOCUSED
        } else {
            BorderStyle::ASCII
        };
        grid.draw_box_styled(
            floating.rect,
            border,
            floating_border_style(floating.focused),
        );

        if let Some(title) = &floating.title {
            let title_rect = Rect {
                origin: Point {
                    x: floating.rect.origin.x + 2,
                    y: floating.rect.origin.y,
                },
                size: Size {
                    width: floating.rect.size.width.saturating_sub(4),
                    height: 1,
                },
            };
            if title_rect.size.width > 0 {
                grid.put_str_styled(
                    clamp_i32_to_u16(title_rect.origin.x),
                    clamp_i32_to_u16(title_rect.origin.y),
                    &truncate(title, title_rect.size.width),
                    floating_title_style(floating.focused),
                );
            }
        }
    }
}

fn render_scroll_indicator(
    grid: &mut RenderGrid,
    x: u16,
    y: u16,
    width: u16,
    top_line: u64,
    total_lines: u64,
) {
    if width == 0 || total_lines == 0 {
        return;
    }
    let label = truncate(
        &format!("{}/{}", top_line.saturating_add(1), total_lines),
        width,
    );
    let label_width = display_width(&label).min(width);
    let origin_x = x.saturating_add(width.saturating_sub(label_width));
    grid.put_str_styled(origin_x, y, &label, scroll_indicator_style());
}

fn render_search_overlay(
    grid: &mut RenderGrid,
    x: u16,
    y: u16,
    width: u16,
    top_line: u64,
    lines: &[String],
    search_state: &crate::state::SearchState,
) {
    let Some(active_index) = search_state.active_match_index else {
        return;
    };
    for (index, search_match) in search_state.matches.iter().enumerate() {
        if search_match.line < top_line {
            continue;
        }
        let relative_row = search_match.line - top_line;
        let Some(relative_row) = u16::try_from(relative_row).ok() else {
            continue;
        };
        if relative_row >= u16::try_from(lines.len()).unwrap_or(u16::MAX) {
            continue;
        }
        let line = &lines[usize::from(relative_row)];
        overlay_display_range(
            grid,
            OverlayLine {
                x,
                y: y.saturating_add(relative_row),
                width,
                text: line,
            },
            search_match.start_column,
            search_match.end_column,
            if index == active_index {
                active_search_style()
            } else {
                search_style()
            },
        );
    }
}

fn render_selection_overlay(
    grid: &mut RenderGrid,
    x: u16,
    y: u16,
    width: u16,
    top_line: u64,
    lines: &[String],
    selection_state: &SelectionState,
) {
    for (row, line) in lines.iter().enumerate() {
        let Some(row_u16) = u16::try_from(row).ok() else {
            break;
        };
        let line_number = top_line.saturating_add(u64::try_from(row).unwrap_or(u64::MAX));
        let Some((start_column, end_column)) =
            selection_range_for_line(selection_state, line_number, width, line)
        else {
            continue;
        };
        overlay_display_range(
            grid,
            OverlayLine {
                x,
                y: y.saturating_add(row_u16),
                width,
                text: line,
            },
            start_column,
            end_column,
            selection_style(),
        );
    }
}

fn selection_range_for_line(
    selection_state: &SelectionState,
    line_number: u64,
    width: u16,
    line: &str,
) -> Option<(u16, u16)> {
    match selection_state.kind {
        SelectionKind::Line => {
            let start_line = selection_state.anchor.line.min(selection_state.cursor.line);
            let end_line = selection_state.anchor.line.max(selection_state.cursor.line);
            (start_line..=end_line)
                .contains(&line_number)
                .then_some((0, width))
        }
        SelectionKind::Block => {
            let start_line = selection_state.anchor.line.min(selection_state.cursor.line);
            let end_line = selection_state.anchor.line.max(selection_state.cursor.line);
            if !(start_line..=end_line).contains(&line_number) {
                return None;
            }
            Some((
                selection_state
                    .anchor
                    .column
                    .min(selection_state.cursor.column),
                selection_state
                    .anchor
                    .column
                    .max(selection_state.cursor.column)
                    .saturating_add(1),
            ))
        }
        SelectionKind::Character => {
            let (start, end) = ordered_points(selection_state.anchor, selection_state.cursor);
            if !(start.line..=end.line).contains(&line_number) {
                return None;
            }
            let line_width = display_width(line).max(1);
            if start.line == end.line {
                Some((start.column, end.column.saturating_add(1)))
            } else if line_number == start.line {
                Some((start.column, line_width.max(start.column.saturating_add(1))))
            } else if line_number == end.line {
                Some((0, end.column.saturating_add(1)))
            } else {
                Some((0, line_width))
            }
        }
    }
}

fn ordered_points(left: SelectionPoint, right: SelectionPoint) -> (SelectionPoint, SelectionPoint) {
    if (left.line, left.column) <= (right.line, right.column) {
        (left, right)
    } else {
        (right, left)
    }
}

struct OverlayLine<'a> {
    x: u16,
    y: u16,
    width: u16,
    text: &'a str,
}

fn overlay_display_range(
    grid: &mut RenderGrid,
    line: OverlayLine<'_>,
    start_column: u16,
    end_column: u16,
    style: CellStyle,
) {
    if start_column >= end_column || line.width == 0 {
        return;
    }

    let visible_end = end_column.min(line.width);
    let mut column = 0_u16;
    for grapheme in UnicodeSegmentation::graphemes(line.text, true) {
        let grapheme_width = display_width(grapheme).max(1);
        let next_column = column.saturating_add(grapheme_width);
        if next_column > start_column && column < visible_end {
            grid.put_str_styled(line.x.saturating_add(column), line.y, grapheme, style);
        }
        column = next_column;
        if column >= visible_end {
            return;
        }
    }

    for column in column.max(start_column)..visible_end {
        grid.put_char_styled(line.x.saturating_add(column), line.y, ' ', style);
    }
}

fn format_tab_label(tab: &crate::presentation::TabItem, width: u16) -> String {
    if width == 0 {
        return String::new();
    }

    let marker = activity_marker(tab.activity);
    let body_width = width.saturating_sub(2);
    let body = truncate(&format!("{marker}{}", tab.title), body_width);
    if tab.active {
        truncate(&format!("[{body}]"), width)
    } else {
        truncate(&format!(" {body} "), width)
    }
}

fn activity_marker(activity: ActivityState) -> char {
    match activity {
        ActivityState::Idle => ' ',
        ActivityState::Activity => '+',
        ActivityState::Bell => '!',
    }
}

fn render_bar_segments(
    grid: &mut RenderGrid,
    mut x: u16,
    y: u16,
    end_x: u16,
    segments: &[BarSegment],
) {
    for segment in segments {
        if x >= end_x {
            break;
        }
        let available = end_x.saturating_sub(x);
        if available == 0 {
            break;
        }
        let label = truncate(&segment.text, available);
        grid.put_str_styled(x, y, &label, CellStyle::from(&segment.style));
        x = x.saturating_add(display_width(&label));
    }
}

fn truncate(text: &str, width: u16) -> String {
    if width == 0 {
        return String::new();
    }

    let width = usize::from(width);
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }

    if width == 1 {
        return "~".to_owned();
    }

    let mut output = String::new();
    let mut used = 0;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
        if used + grapheme_width > width - 1 {
            break;
        }
        output.push_str(grapheme);
        used += grapheme_width;
    }
    output.push('~');
    output
}

fn clamp_i32_to_u16(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

fn clamp_usize_to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn display_width(text: &str) -> u16 {
    clamp_usize_to_u16(UnicodeWidthStr::width(text))
}

fn tab_style(active: bool) -> CellStyle {
    if active {
        CellStyle::default().with_reverse().with_bold()
    } else {
        CellStyle {
            dim: true,
            ..CellStyle::default()
        }
    }
}

fn leaf_title_style(focused: bool) -> CellStyle {
    if focused {
        CellStyle::default().with_bold()
    } else {
        CellStyle::default()
    }
}

fn divider_style() -> CellStyle {
    CellStyle {
        dim: true,
        ..CellStyle::default()
    }
}

fn floating_border_style(focused: bool) -> CellStyle {
    if focused {
        CellStyle::default().with_bold().with_reverse()
    } else {
        CellStyle {
            dim: true,
            ..CellStyle::default()
        }
    }
}

fn floating_title_style(focused: bool) -> CellStyle {
    if focused {
        CellStyle::default().with_bold()
    } else {
        CellStyle {
            dim: true,
            ..CellStyle::default()
        }
    }
}

fn scroll_indicator_style() -> CellStyle {
    CellStyle {
        dim: true,
        reverse: true,
        ..CellStyle::default()
    }
}

fn search_style() -> CellStyle {
    CellStyle {
        underline: true,
        ..CellStyle::default()
    }
}

fn active_search_style() -> CellStyle {
    CellStyle {
        underline: true,
        reverse: true,
        ..CellStyle::default()
    }
}

fn selection_style() -> CellStyle {
    CellStyle {
        reverse: true,
        ..CellStyle::default()
    }
}
