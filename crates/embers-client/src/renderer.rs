use std::collections::BTreeMap;

use embers_core::{ActivityState, CursorShape, Point, Rect, Size, SnapshotLine};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::grid::{
    BorderStyle, CellStyle, GridCursor, RenderGrid, UnderlineStyle, replace_controls,
};
use crate::presentation::{DividerFrame, FloatingFrame, LeafFrame, PresentationModel, TabsFrame};
use crate::scripting::{BarSegment, BarSpec, BarTarget, PaneBorderColors};
use crate::state::{ClientState, SelectionKind, SelectionPoint, SelectionState};

/// The most columns the session name takes in the default root tab bar.
const SESSION_LABEL_WIDTH: u16 = 10;

#[derive(Clone, Copy, Debug, Default)]
pub struct Renderer {
    /// Divider colors from the theme.
    pub pane_borders: PaneBorderColors,
}

impl Renderer {
    pub fn new(pane_borders: PaneBorderColors) -> Self {
        Self { pane_borders }
    }
}

/// Follow-output viewport positioning shared by the renderer and the hints scan
/// so overlays align with drawn content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FollowOutputView {
    /// Leading rendered lines to skip so the bottom `content_rows` are shown.
    pub display_offset: usize,
    /// Absolute line number of the first displayed line.
    pub displayed_top_line: u64,
}

/// Compute the follow-output display offset and top line for a pane. When
/// `follow_output` is false the offset is zero (the caller's scroll position is
/// used as-is).
pub(crate) fn follow_output_position(
    lines: &[SnapshotLine],
    content_rows: usize,
    scroll_top_line: u64,
    follow_output: bool,
) -> FollowOutputView {
    let display_offset = if follow_output {
        let significant = lines
            .iter()
            .rposition(|line| !line.text.is_empty())
            .map(|index| index + 1)
            .unwrap_or(0);
        significant.saturating_sub(content_rows)
    } else {
        0
    };
    FollowOutputView {
        display_offset,
        displayed_top_line: scroll_top_line.saturating_add(display_offset as u64),
    }
}

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

        let focused = model
            .focused_leaf()
            .filter(|leaf| leaf.floating_id == floating_id)
            .map(|leaf| leaf.rect);
        for divider in model
            .dividers
            .iter()
            .filter(|divider| divider.floating_id == floating_id)
        {
            self.render_divider(grid, divider, focused);
        }

        for tabs in model
            .tab_bars
            .iter()
            .filter(|tabs| tabs.floating_id == floating_id)
        {
            self.render_tabs(grid, tabs, tab_bars.get(&tabs.node_id), state, model);
        }
    }

    /// What a click at `point` selects in the tab bars
    /// [`Self::render_with_tab_bars`] draws with the same `tab_bars`: the
    /// target of the segment under it. The default bars target each tab's
    /// label; a formatter's segments target what they were tagged with.
    /// `None` off the tab bars, on untagged segments, and where a floating
    /// window covers the bar.
    pub fn tab_bar_target_at(
        &self,
        state: &ClientState,
        model: &PresentationModel,
        tab_bars: &BTreeMap<embers_core::NodeId, BarSpec>,
        point: Point,
    ) -> Option<BarTarget> {
        let layer = model
            .floating_at(point)
            .map(|floating| floating.floating_id);
        let tabs = model
            .tab_bars
            .iter()
            .rev()
            .filter(|tabs| tabs.floating_id == layer)
            .find(|tabs| tabs.rect.contains(point))?;
        let x = u16::try_from(point.x).ok()?;
        layout_tabs(tabs, tab_bars.get(&tabs.node_id), state, model)
            .into_iter()
            .find(|cells| x >= cells.x && x < cells.x.saturating_add(display_width(&cells.text)))
            .and_then(|cells| cells.target)
    }

    fn render_tabs(
        &self,
        grid: &mut RenderGrid,
        tabs: &TabsFrame,
        custom: Option<&BarSpec>,
        state: &ClientState,
        model: &PresentationModel,
    ) {
        if tabs.rect.size.height == 0 || tabs.rect.size.width == 0 {
            return;
        }
        let x = clamp_i32_to_u16(tabs.rect.origin.x);
        let y = clamp_i32_to_u16(tabs.rect.origin.y);
        grid.put_str(x, y, &" ".repeat(usize::from(tabs.rect.size.width)));
        for cells in layout_tabs(tabs, custom, state, model) {
            grid.put_str_styled(cells.x, y, &cells.text, cells.style);
        }
    }

    fn render_leaf(&self, state: &ClientState, grid: &mut RenderGrid, leaf: &LeafFrame) {
        if leaf.rect.size.width == 0 || leaf.rect.size.height == 0 {
            return;
        }

        let blank_line = " ".repeat(usize::from(leaf.rect.size.width));
        for row in 0..leaf.rect.size.height {
            grid.put_str(
                clamp_i32_to_u16(leaf.rect.origin.x),
                clamp_i32_to_u16(leaf.rect.origin.y).saturating_add(row),
                &blank_line,
            );
        }

        if let Some(title_rect) = leaf.title_rect {
            let activity = activity_marker(leaf.activity);
            let title = truncate(
                &format!(
                    "{}{} {}",
                    if leaf.focused { '>' } else { ' ' },
                    activity,
                    leaf.title
                ),
                title_rect.size.width,
            );
            grid.put_str_styled(
                clamp_i32_to_u16(title_rect.origin.x),
                clamp_i32_to_u16(title_rect.origin.y),
                &title,
                leaf_title_style(leaf.focused),
            );
        }

        // Everything below is drawn in the content area: `y` is its first row.
        let x = clamp_i32_to_u16(leaf.content_rect.origin.x);
        let y = clamp_i32_to_u16(leaf.content_rect.origin.y);
        let width = leaf.content_rect.size.width;
        let height = leaf.content_rect.size.height;
        if height == 0 {
            return;
        }

        let view_state = state.view_state(leaf.node_id);
        let rendered_lines = view_state
            .map(|view| {
                if view.visible_lines.is_empty() {
                    state
                        .snapshots
                        .get(&leaf.buffer_id)
                        .map(|snapshot| snapshot.lines.as_slice())
                        .unwrap_or(&[])
                } else {
                    view.visible_lines.as_slice()
                }
            })
            .or_else(|| {
                state
                    .snapshots
                    .get(&leaf.buffer_id)
                    .map(|snapshot| snapshot.lines.as_slice())
            });

        let content_rows = usize::from(height);
        let position = view_state
            .map(|view| {
                follow_output_position(
                    rendered_lines.unwrap_or(&[]),
                    content_rows,
                    view.scroll_top_line,
                    view.follow_output,
                )
            })
            .unwrap_or_else(|| {
                // No view state yet (e.g. a snapshot arrived before the reducer
                // built one): default to following output at the bottom of the
                // rendered content, matching a freshly-created view, instead of
                // pinning to the top of the buffer.
                follow_output_position(rendered_lines.unwrap_or(&[]), content_rows, 0, true)
            });
        let display_offset = position.display_offset;
        let displayed_top_line = position.displayed_top_line;
        let displayed_view_lines =
            rendered_lines.map(|lines| &lines[display_offset.min(lines.len())..]);

        if let Some(lines) = rendered_lines {
            for (row, line) in lines
                .iter()
                .skip(display_offset)
                .take(content_rows)
                .enumerate()
            {
                let Some(row) = u16::try_from(row).ok() else {
                    break;
                };
                grid.put_snapshot_line(x, y + row, width, line);
            }
        }

        if let Some(view_state) = view_state
            && !view_state.alternate_screen
        {
            if let Some(search_state) = &view_state.search_state {
                render_search_overlay(
                    grid,
                    x,
                    y,
                    width,
                    displayed_top_line,
                    displayed_view_lines.unwrap_or(rendered_lines.unwrap_or(&[])),
                    search_state,
                );
            }
            if let Some(selection_state) = &view_state.selection_state {
                render_selection_overlay(
                    grid,
                    x,
                    y,
                    width,
                    displayed_top_line,
                    displayed_view_lines.unwrap_or(rendered_lines.unwrap_or(&[])),
                    selection_state,
                );
            }
            if let Some(hints_state) = &view_state.hints_state {
                render_hints_overlay(
                    grid,
                    x,
                    y,
                    width,
                    content_rows,
                    displayed_top_line,
                    hints_state,
                );
            }
            if !view_state.follow_output {
                // On the title row, or over the content's top-right corner
                // like tmux's position indicator.
                let indicator_y = leaf
                    .title_rect
                    .map_or(y, |title| clamp_i32_to_u16(title.origin.y));
                render_scroll_indicator(
                    grid,
                    x,
                    indicator_y,
                    width,
                    view_state.scroll_top_line,
                    view_state.total_line_count,
                );
            }
        }

        // In copy mode the cursor shows the copy-mode position (a selection's
        // free end while selecting) instead of the program's cursor.
        let copy_cursor = view_state
            .filter(|view| !view.alternate_screen)
            .and_then(|view| {
                view.copy_cursor.map(|cursor| {
                    view.selection_state
                        .as_ref()
                        .map_or(cursor, |selection| selection.cursor)
                })
            });
        if let Some(cursor) = copy_cursor {
            if leaf.focused
                && let Some(row) = cursor.line.checked_sub(displayed_top_line)
                && row < content_rows as u64
                && cursor.column < width
            {
                grid.set_cursor(Some(GridCursor {
                    x: x + cursor.column,
                    y: y + row as u16,
                    shape: CursorShape::Block,
                }));
            }
        } else if let Some(snapshot) = state.snapshots.get(&leaf.buffer_id)
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
                let cursor_y = y + cursor.position.row;
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

    /// Draw a divider, coloring the cells that border `focused`, the focused
    /// pane's rect, with the active border color.
    fn render_divider(&self, grid: &mut RenderGrid, divider: &DividerFrame, focused: Option<Rect>) {
        if divider.rect.size.width == 0 || divider.rect.size.height == 0 {
            return;
        }

        let rect = divider.rect;
        let (ch, cells) = match divider.direction {
            embers_core::SplitDirection::Horizontal => ('-', rect.size.width),
            embers_core::SplitDirection::Vertical => ('|', rect.size.height),
        };
        for offset in 0..cells {
            let (x, y) = match divider.direction {
                embers_core::SplitDirection::Horizontal => {
                    (rect.origin.x + i32::from(offset), rect.origin.y)
                }
                embers_core::SplitDirection::Vertical => {
                    (rect.origin.x, rect.origin.y + i32::from(offset))
                }
            };
            let active = focused.is_some_and(|pane| borders(pane, divider.direction, x, y));
            let color = if active {
                self.pane_borders.active.or(self.pane_borders.inactive)
            } else {
                self.pane_borders.inactive
            };
            let style = match color {
                Some(color) => CellStyle {
                    fg: Some(color),
                    ..CellStyle::default()
                },
                None => divider_style(),
            };
            grid.put_char_styled(clamp_i32_to_u16(x), clamp_i32_to_u16(y), ch, style);
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
    lines: &[SnapshotLine],
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
        let start_column = search_match.start_column.min(width);
        let end_column = search_match.end_column.min(width);
        let overlay = if index == active_index {
            active_search_style()
        } else {
            search_style()
        };
        // Search highlights compose: OR the overlay's attributes onto the content
        // style and only override fg/bg where the overlay defines them.
        grid.restyle_range(
            x,
            y.saturating_add(relative_row),
            start_column,
            end_column,
            |base| compose_overlay(base, overlay),
        );
    }
}

fn render_selection_overlay(
    grid: &mut RenderGrid,
    x: u16,
    y: u16,
    width: u16,
    top_line: u64,
    lines: &[SnapshotLine],
    selection_state: &SelectionState,
) {
    for (row, line) in lines.iter().enumerate() {
        let Some(row_u16) = u16::try_from(row).ok() else {
            break;
        };
        let line_number = top_line.saturating_add(u64::try_from(row).unwrap_or(u64::MAX));
        let Some((start_column, end_column)) =
            selection_range_for_line(selection_state, line_number, width, &line.text)
        else {
            continue;
        };
        // Selection toggles reverse on the underlying content style so colored
        // text stays colored while the selection reads as inverted.
        grid.restyle_range(
            x,
            y.saturating_add(row_u16),
            start_column.min(width),
            end_column.min(width),
            |base| CellStyle {
                reverse: !base.reverse,
                ..base
            },
        );
    }
}

/// Compose an overlay style onto a base content style: attributes are OR-ed and
/// the overlay's colors win only where it defines them.
fn compose_overlay(base: CellStyle, overlay: CellStyle) -> CellStyle {
    CellStyle {
        fg: overlay.fg.or(base.fg),
        bg: overlay.bg.or(base.bg),
        bold: base.bold || overlay.bold,
        italic: base.italic || overlay.italic,
        underline: if overlay.underline.is_none() {
            base.underline
        } else {
            overlay.underline
        },
        underline_color: overlay.underline_color.or(base.underline_color),
        dim: base.dim || overlay.dim,
        reverse: base.reverse || overlay.reverse,
        blink: base.blink || overlay.blink,
        strikeout: base.strikeout || overlay.strikeout,
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

/// A run of cells a tab bar draws, and what a click on it selects.
struct BarCells {
    x: u16,
    text: String,
    style: CellStyle,
    target: Option<BarTarget>,
}

/// Lay out the cells of `tabs`'s bar: `custom`'s segments, or the default
/// bar. Drawing and clicks both go through here, so a click selects what is
/// drawn under it.
fn layout_tabs(
    tabs: &TabsFrame,
    custom: Option<&BarSpec>,
    state: &ClientState,
    model: &PresentationModel,
) -> Vec<BarCells> {
    let mut cells = Vec::new();
    if tabs.rect.size.height == 0 || tabs.rect.size.width == 0 {
        return cells;
    }

    let mut x = clamp_i32_to_u16(tabs.rect.origin.x);
    let end_x = x.saturating_add(tabs.rect.size.width);

    if let Some(bar) = custom {
        // Formatters build segments from titles programs set.
        let sanitize = |segments: &[BarSegment]| -> Vec<BarSegment> {
            segments
                .iter()
                .map(|segment| BarSegment {
                    text: replace_controls(&segment.text).into_owned(),
                    ..segment.clone()
                })
                .collect()
        };
        let bar = BarSpec {
            left: sanitize(&bar.left),
            center: sanitize(&bar.center),
            right: sanitize(&bar.right),
        };
        let width = tabs.rect.size.width;
        let left_width = bar
            .left
            .iter()
            .map(|segment| display_width(&segment.text))
            .sum::<u16>()
            .min(width);
        let right_width = bar
            .right
            .iter()
            .map(|segment| display_width(&segment.text))
            .sum::<u16>()
            .min(width);
        let left_end = x.saturating_add(left_width);
        let right_start = end_x.saturating_sub(right_width);
        layout_bar_segments(&mut cells, x, left_end, &bar.left);
        layout_bar_segments(&mut cells, right_start, end_x, &bar.right);

        let center_span = right_start.saturating_sub(left_end);
        let center_width = bar
            .center
            .iter()
            .map(|segment| display_width(&segment.text))
            .sum::<u16>()
            .min(center_span);
        let center_x = left_end.saturating_add(center_span.saturating_sub(center_width) / 2);
        layout_bar_segments(
            &mut cells,
            center_x,
            center_x.saturating_add(center_width),
            &bar.center,
        );
        return cells;
    }

    // The default root tab bar starts with the session name, like tmux's
    // default status line.
    let session_name = state
        .sessions
        .get(&model.session_id)
        .map(|session| session.name.as_str())
        .filter(|_| tabs.is_root);
    if let Some(name) = session_name {
        // At most tmux's default status-left-length.
        let width = tabs.rect.size.width.min(SESSION_LABEL_WIDTH);
        let label = truncate(&format!("[{name}] "), width);
        let label_width = display_width(&label);
        cells.push(BarCells {
            x,
            text: label,
            style: CellStyle::default(),
            target: None,
        });
        x = x.saturating_add(label_width);
    }

    for (index, tab) in tabs.tabs.iter().enumerate() {
        let available = end_x.saturating_sub(x);
        if available == 0 {
            break;
        }

        let label = format_tab_label(tab, available);
        let label_width = display_width(&label);
        cells.push(BarCells {
            x,
            text: label,
            style: tab_style(tab.active),
            target: Some(BarTarget::Tab {
                tabs_node_id: tabs.node_id,
                index,
            }),
        });
        // A blank column follows each label; the row is already cleared.
        x = x.saturating_add(label_width).saturating_add(1);
    }
    cells
}

fn format_tab_label(tab: &crate::presentation::TabItem, width: u16) -> String {
    if width == 0 {
        return String::new();
    }

    let marker = activity_marker(tab.activity);
    let body_width = width.saturating_sub(2);
    let sync = if tab.synchronize_panes { " (sync)" } else { "" };
    let body = truncate(&format!("{marker}{}{sync}", tab.title), body_width);
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

fn layout_bar_segments(cells: &mut Vec<BarCells>, mut x: u16, end_x: u16, segments: &[BarSegment]) {
    for segment in segments {
        let available = end_x.saturating_sub(x);
        if available == 0 {
            break;
        }
        let label = truncate(&segment.text, available);
        let label_width = display_width(&label);
        cells.push(BarCells {
            x,
            text: label,
            style: CellStyle::from(&segment.style),
            target: segment.target.clone(),
        });
        x = x.saturating_add(label_width);
    }
}

/// Fit `text` in `width` columns, ending it with `~` when it is cut. Control
/// characters become spaces, since titles come from programs in panes.
fn truncate(text: &str, width: u16) -> String {
    if width == 0 {
        return String::new();
    }
    let text = &*replace_controls(text);

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

/// Whether the divider cell at `x`, `y` lies along an edge of `pane`: beside
/// it for a vertical divider, above or below it for a horizontal one.
fn borders(pane: Rect, direction: embers_core::SplitDirection, x: i32, y: i32) -> bool {
    let left = pane.origin.x;
    let top = pane.origin.y;
    let right = left + i32::from(pane.size.width);
    let bottom = top + i32::from(pane.size.height);
    match direction {
        embers_core::SplitDirection::Vertical => {
            (x == left - 1 || x == right) && (top..bottom).contains(&y)
        }
        embers_core::SplitDirection::Horizontal => {
            (y == top - 1 || y == bottom) && (left..right).contains(&x)
        }
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

fn render_hints_overlay(
    grid: &mut RenderGrid,
    x: u16,
    y: u16,
    width: u16,
    content_rows: usize,
    top_line: u64,
    hints_state: &crate::state::HintsState,
) {
    // Count the typed prefix once; it's the same for every label character below.
    let typed_len = hints_state.typed.chars().count();
    for hint in &hints_state.matches {
        // A label still consistent with what has been typed so far stays active;
        // labels that no longer match the prefix are dropped from the overlay.
        if !hint.label.starts_with(&hints_state.typed) {
            continue;
        }
        if hint.line < top_line {
            continue;
        }
        let Some(relative_row) = u16::try_from(hint.line - top_line).ok() else {
            continue;
        };
        // Never draw past the pane's content rows onto the row below it.
        if usize::from(relative_row) >= content_rows {
            continue;
        }
        // Dim the matched text so the labels stand out.
        grid.restyle_range(
            x,
            y.saturating_add(relative_row),
            hint.start_col.min(width),
            hint.end_col.min(width),
            |base| CellStyle { dim: true, ..base },
        );
        // Draw the label characters at the start of the match, highlighting the
        // portion already typed.
        let label_row = y.saturating_add(relative_row);
        for (offset, ch) in hint.label.chars().enumerate() {
            let Some(offset) = u16::try_from(offset).ok() else {
                break;
            };
            let column = hint.start_col.saturating_add(offset);
            if column >= width {
                break;
            }
            let typed = usize::from(offset) < typed_len;
            grid.put_char_styled(
                x.saturating_add(column),
                label_row,
                ch,
                hint_label_style(typed),
            );
        }
    }
}

fn hint_label_style(typed: bool) -> CellStyle {
    CellStyle {
        bold: true,
        reverse: !typed,
        underline: if typed {
            UnderlineStyle::Single
        } else {
            UnderlineStyle::None
        },
        ..CellStyle::default()
    }
}

fn search_style() -> CellStyle {
    CellStyle {
        underline: UnderlineStyle::Single,
        italic: true,
        ..CellStyle::default()
    }
}

fn active_search_style() -> CellStyle {
    CellStyle {
        underline: UnderlineStyle::Single,
        reverse: true,
        italic: true,
        ..CellStyle::default()
    }
}
