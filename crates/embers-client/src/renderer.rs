use std::collections::BTreeMap;

use embers_core::{ActivityState, Point, Rect, Size};

use crate::grid::{BorderStyle, RenderGrid};
use crate::presentation::{DividerFrame, FloatingFrame, LeafFrame, PresentationModel, TabsFrame};
use crate::scripting::{BarSegment, BarSpec};
use crate::state::ClientState;

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
                .map(|segment| clamp_usize_to_u16(segment.text.chars().count()))
                .sum::<u16>();
            let right_x = end_x.saturating_sub(right_width.min(width));
            render_bar_segments(grid, right_x, y, end_x, &bar.right);

            let center_width = bar
                .center
                .iter()
                .map(|segment| clamp_usize_to_u16(segment.text.chars().count()))
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
            grid.put_str(x, y, &label);
            x = x.saturating_add(clamp_usize_to_u16(label.chars().count()));

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
        grid.put_str(x, y, &title);

        if height <= 1 {
            return;
        }

        if let Some(snapshot) = state.snapshots.get(&leaf.buffer_id) {
            for (row, line) in snapshot
                .lines
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
    }

    fn render_divider(&self, grid: &mut RenderGrid, divider: &DividerFrame) {
        if divider.rect.size.width == 0 || divider.rect.size.height == 0 {
            return;
        }

        let x = clamp_i32_to_u16(divider.rect.origin.x);
        let y = clamp_i32_to_u16(divider.rect.origin.y);
        match divider.direction {
            embers_core::SplitDirection::Horizontal => {
                grid.draw_hline(x, y, divider.rect.size.width, '-');
            }
            embers_core::SplitDirection::Vertical => {
                grid.draw_vline(x, y, divider.rect.size.height, '|');
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
        grid.draw_box(floating.rect, border);

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
                grid.put_str(
                    clamp_i32_to_u16(title_rect.origin.x),
                    clamp_i32_to_u16(title_rect.origin.y),
                    &truncate(title, title_rect.size.width),
                );
            }
        }
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
        grid.put_str(x, y, &label);
        x = x.saturating_add(clamp_usize_to_u16(label.chars().count()));
    }
}

fn truncate(text: &str, width: u16) -> String {
    let width = usize::from(width);
    if width == 0 {
        return String::new();
    }

    let len = text.chars().count();
    if len <= width {
        return text.chars().collect();
    }

    if width == 1 {
        return "~".to_owned();
    }

    let mut output = text.chars().take(width - 1).collect::<String>();
    output.push('~');
    output
}

fn clamp_i32_to_u16(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

fn clamp_usize_to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}
