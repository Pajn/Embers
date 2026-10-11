//! Cell-based split resizing: tmux `resize-pane -L/-R/-U/-D` and dragging a
//! divider with the mouse.
//!
//! Split sizes are stored as relative weights. A resize is planned against the
//! cells each child was drawn with and sent back as those cell counts, which
//! reproduce the new layout exactly at the current viewport and stay
//! proportional at any other size.

use embers_core::{NodeId, Point, SessionId, Size, SplitDirection};
use embers_protocol::NodeRecordKind;

use crate::presentation::{DividerFrame, NavigationDirection, PaneTitles, PresentationModel};
use crate::state::ClientState;

/// New weights for one split, ready for `NodeRequest::Resize`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitResize {
    pub split_node_id: NodeId,
    pub sizes: Vec<u16>,
}

impl PresentationModel {
    /// Plan tmux's `resize-pane -L/-R/-U/-D <cells>` for `leaf_node_id`.
    ///
    /// The nearest visible ancestor split along the direction's axis is
    /// resized by moving the divider after the pane, or the divider before it
    /// when the pane is the split's last child: left and up move it toward the
    /// start, right and down toward the end. Space is taken from the panes on
    /// the shrinking side, nearest first, down to their minimum size. Returns
    /// `None` when nothing would change: there is no such split (including
    /// splits hidden by zoom and the edge of a popup), or the divider is
    /// already at its limit.
    pub fn resize_leaf(
        &self,
        state: &ClientState,
        leaf_node_id: NodeId,
        direction: NavigationDirection,
        cells: u16,
    ) -> Option<SplitResize> {
        let (axis, delta) = match direction {
            NavigationDirection::Left => (SplitDirection::Vertical, -i32::from(cells)),
            NavigationDirection::Right => (SplitDirection::Vertical, i32::from(cells)),
            NavigationDirection::Up => (SplitDirection::Horizontal, -i32::from(cells)),
            NavigationDirection::Down => (SplitDirection::Horizontal, i32::from(cells)),
        };

        let mut child_id = leaf_node_id;
        loop {
            let parent_id = state.nodes.get(&child_id)?.parent_id?;
            if let Some(split) = self
                .splits
                .iter()
                .find(|split| split.node_id == parent_id && split.direction == axis)
            {
                let mut index = split.child_ids.iter().position(|id| *id == child_id)?;
                if index + 1 == split.child_ids.len() {
                    index = index.checked_sub(1)?;
                }
                return move_divider(&split.lengths, &split.min_lengths, index, delta).map(
                    |sizes| SplitResize {
                        split_node_id: split.node_id,
                        sizes,
                    },
                );
            }
            child_id = parent_id;
        }
    }

    /// The divider drawn at `point`, if any. A popup covering the point hides
    /// the dividers beneath it.
    pub fn divider_at(&self, point: Point) -> Option<&DividerFrame> {
        let floating_id = self.floating_at(point).map(|floating| floating.floating_id);
        self.dividers
            .iter()
            .rev()
            .find(|divider| divider.floating_id == floating_id && divider.rect.contains(point))
    }

    /// Plan moving divider `index` of `split_node_id` so it lands under
    /// `point`, clamped so every pane keeps its minimum size.
    pub fn drag_divider(
        &self,
        split_node_id: NodeId,
        index: usize,
        point: Point,
    ) -> Option<SplitResize> {
        let split = self
            .splits
            .iter()
            .find(|split| split.node_id == split_node_id)?;
        if index + 1 >= split.lengths.len() {
            return None;
        }
        let (origin, target) = match split.direction {
            SplitDirection::Horizontal => (split.rect.origin.y, point.y),
            SplitDirection::Vertical => (split.rect.origin.x, point.x),
        };
        let leading: i32 = split.lengths[..=index]
            .iter()
            .map(|length| i32::from(*length))
            .sum();
        let current = origin + leading + i32::try_from(index).ok()?;
        move_divider(&split.lengths, &split.min_lengths, index, target - current).map(|sizes| {
            SplitResize {
                split_node_id,
                sizes,
            }
        })
    }
}

/// Move the divider between `lengths[index]` and `lengths[index + 1]` by
/// `delta` cells. The pane next to the divider on the growing side absorbs
/// the space; panes on the shrinking side give it up nearest first, never
/// below `mins`. Returns `None` if the divider cannot move.
fn move_divider(lengths: &[u16], mins: &[u16], index: usize, delta: i32) -> Option<Vec<u16>> {
    if index + 1 >= lengths.len() || delta == 0 {
        return None;
    }
    let mut lengths = lengths.to_vec();
    let mut needed = u16::try_from(delta.unsigned_abs()).unwrap_or(u16::MAX);
    let (receiver, donors): (usize, Vec<usize>) = if delta > 0 {
        (index, (index + 1..lengths.len()).collect())
    } else {
        (index + 1, (0..=index).rev().collect())
    };

    let mut moved = 0_u16;
    for donor in donors {
        if needed == 0 {
            break;
        }
        let min = mins.get(donor).copied().unwrap_or(1).max(1);
        let spare = lengths[donor].saturating_sub(min);
        let take = spare.min(needed);
        lengths[donor] -= take;
        needed -= take;
        moved += take;
    }
    if moved == 0 {
        return None;
    }
    lengths[receiver] = lengths[receiver].saturating_add(moved);
    // Split weights must be positive; a child squeezed to nothing by a tiny
    // viewport keeps a token weight.
    Some(lengths.into_iter().map(|length| length.max(1)).collect())
}

/// Reconstruct the viewport a session was last drawn at from the PTY sizes
/// the drawing client gave its visible panes: each pane's PTY is its drawn
/// size, minus its title row if the client reported title rows
/// ([`PaneTitles::reported`]). Used where no viewport is known, such as the
/// CLI. Returns `None` if the session or any visible pane is missing.
pub fn drawn_viewport(state: &ClientState, session_id: SessionId) -> Option<Size> {
    let session = state.sessions.get(&session_id)?;
    let root = session
        .zoomed_node_id
        .filter(|node_id| state.nodes.contains_key(node_id))
        .unwrap_or(session.root_node_id);
    let title_rows = u16::from(PaneTitles::reported(session).shown());
    drawn_extent(state, root, title_rows)
}

fn drawn_extent(state: &ClientState, node_id: NodeId, title_rows: u16) -> Option<Size> {
    let node = state.nodes.get(&node_id)?;
    match node.kind {
        NodeRecordKind::BufferView => {
            let buffer = state.buffers.get(&node.buffer_view.as_ref()?.buffer_id)?;
            Some(Size {
                width: buffer.pty_size.cols,
                height: buffer.pty_size.rows.saturating_add(title_rows),
            })
        }
        NodeRecordKind::Tabs => {
            let tabs = node.tabs.as_ref()?;
            let active = tabs.tabs.get(usize::try_from(tabs.active).ok()?)?;
            let child = drawn_extent(state, active.child_id, title_rows)?;
            Some(Size {
                width: child.width,
                height: child.height.saturating_add(1),
            })
        }
        NodeRecordKind::Split => {
            let split = node.split.as_ref()?;
            let children = split
                .child_ids
                .iter()
                .map(|child_id| drawn_extent(state, *child_id, title_rows))
                .collect::<Option<Vec<_>>>()?;
            let dividers = u16::try_from(children.len().checked_sub(1)?).ok()?;
            let sum = |extent: fn(&Size) -> u16| {
                children
                    .iter()
                    .map(extent)
                    .fold(dividers, u16::saturating_add)
            };
            let max = |extent: fn(&Size) -> u16| children.iter().map(extent).max().unwrap_or(0);
            Some(match split.direction {
                SplitDirection::Horizontal => Size {
                    width: max(|size| size.width),
                    height: sum(|size| size.height),
                },
                SplitDirection::Vertical => Size {
                    width: sum(|size| size.width),
                    height: max(|size| size.height),
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::move_divider;

    #[test]
    fn growing_takes_from_the_following_panes_nearest_first() {
        assert_eq!(
            move_divider(&[10, 3, 10], &[1, 1, 1], 0, 5),
            Some(vec![15, 1, 7])
        );
    }

    #[test]
    fn shrinking_takes_from_the_preceding_panes_nearest_first() {
        assert_eq!(
            move_divider(&[10, 3, 10], &[1, 1, 1], 1, -5),
            Some(vec![7, 1, 15])
        );
    }

    #[test]
    fn moves_are_clamped_to_the_available_space() {
        assert_eq!(move_divider(&[4, 4], &[2, 2], 0, 10), Some(vec![6, 2]));
        assert_eq!(move_divider(&[6, 2], &[2, 2], 0, 10), None);
        assert_eq!(move_divider(&[4, 4], &[2, 2], 1, 1), None);
        assert_eq!(move_divider(&[4, 4], &[2, 2], 0, 0), None);
    }
}
