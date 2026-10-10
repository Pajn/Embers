use std::collections::BTreeMap;

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, MuxError, NodeId, Point, PtySize, Rect,
    Result, SessionId, Size, SplitDirection,
};
use embers_protocol::{FloatingRecord, NodeRecordKind, SessionRecord};

use crate::state::ClientState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Where the session's root tab bar (its window list) is drawn, like tmux's
/// `status-position`. Nested tab bars always sit at the top of their area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabBarPosition {
    #[default]
    Top,
    Bottom,
}

impl TabBarPosition {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabItem {
    pub title: String,
    pub child_id: NodeId,
    pub active: bool,
    pub activity: ActivityState,
    pub buffer_count: usize,
    /// tmux `synchronize-panes` is on for the window this tab holds.
    pub synchronize_panes: bool,
    /// The pane the tab shows, as tmux's window formats see its active pane.
    pub pane: Option<TabPane>,
}

/// What a tab bar shows of a tab's pane: the focused pane when the tab holds
/// the focus, otherwise the pane its active nested tabs lead to (the first
/// pane of a split).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabPane {
    pub title: String,
    /// The program in the pane's foreground, like tmux `pane_current_command`.
    pub process_name: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsFrame {
    pub node_id: NodeId,
    pub rect: Rect,
    pub tabs: Vec<TabItem>,
    pub active: usize,
    pub is_root: bool,
    pub floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafFrame {
    pub node_id: NodeId,
    pub buffer_id: BufferId,
    pub rect: Rect,
    pub title: String,
    pub activity: ActivityState,
    pub focused: bool,
    pub floating_id: Option<FloatingId>,
    pub tabs_path: Vec<NodeId>,
}

impl LeafFrame {
    /// The PTY size matching the terminal content the renderer draws for this
    /// leaf: its full width, minus the title row it draws on top. `None` when
    /// the leaf is too small to show any terminal rows.
    pub fn pty_size(&self) -> Option<PtySize> {
        let rows = self.rect.size.height.saturating_sub(1);
        (self.rect.size.width > 0 && rows > 0).then(|| PtySize::new(self.rect.size.width, rows))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DividerFrame {
    pub direction: SplitDirection,
    pub rect: Rect,
    pub floating_id: Option<FloatingId>,
    /// The split this divider belongs to.
    pub split_node_id: NodeId,
    /// The divider sits between child `index` and child `index + 1`.
    pub index: usize,
}

/// A projected split: the cells each child was drawn with along the split
/// axis, and how far each child can shrink before its content disappears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitFrame {
    pub node_id: NodeId,
    pub direction: SplitDirection,
    pub rect: Rect,
    pub child_ids: Vec<NodeId>,
    pub lengths: Vec<u16>,
    pub min_lengths: Vec<u16>,
    pub floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingFrame {
    pub floating_id: FloatingId,
    pub rect: Rect,
    pub content_rect: Rect,
    pub title: Option<String>,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentationModel {
    pub session_id: SessionId,
    pub viewport: Size,
    pub root_tabs: Option<TabsFrame>,
    pub tab_bars: Vec<TabsFrame>,
    pub leaves: Vec<LeafFrame>,
    pub dividers: Vec<DividerFrame>,
    pub splits: Vec<SplitFrame>,
    pub floating: Vec<FloatingFrame>,
}

impl PresentationModel {
    /// Project with the root tab bar at the top.
    pub fn project(state: &ClientState, session_id: SessionId, viewport: Size) -> Result<Self> {
        Self::project_with(state, session_id, viewport, TabBarPosition::Top)
    }

    /// Project with the root tab bar at `tab_bar`. The bar takes one row
    /// either way, so panes have the same sizes wherever it is; only their
    /// positions differ.
    pub fn project_with(
        state: &ClientState,
        session_id: SessionId,
        viewport: Size,
        tab_bar: TabBarPosition,
    ) -> Result<Self> {
        let session = state
            .sessions
            .get(&session_id)
            .ok_or_else(|| MuxError::not_found(format!("session {session_id} is not cached")))?;
        let root_bounds = Rect {
            origin: Point { x: 0, y: 0 },
            size: viewport,
        };
        let mut projection = Projection::default();
        let mut projector = Projector {
            state,
            session,
            projection: &mut projection,
            activity_by_node: BTreeMap::new(),
            buffer_count_by_node: BTreeMap::new(),
            tab_bar,
        };
        if let Some(zoomed_node_id) = session
            .zoomed_node_id
            .filter(|node_id| node_belongs_to_session(state, session, *node_id))
        {
            if let Some(window) = floating_ancestor_for_node(state, session, zoomed_node_id) {
                project_floating_window(&mut projector, window, root_bounds, zoomed_node_id)?;
            } else {
                let is_root = zoomed_node_id == session.root_node_id;
                projector.project_node(zoomed_node_id, root_bounds, None, is_root, Vec::new())?;
            }
        } else {
            projector.project_node(session.root_node_id, root_bounds, None, true, Vec::new())?;

            for floating_id in &session.floating_ids {
                let Some(window) = state.floating.get(floating_id) else {
                    continue;
                };
                if !node_belongs_to_session(state, session, window.root_node_id) {
                    continue;
                }
                project_floating_window(&mut projector, window, root_bounds, window.root_node_id)?;
            }
        }

        let root_tabs = projection.tab_bars.iter().find(|bar| bar.is_root).cloned();

        Ok(Self {
            session_id,
            viewport,
            root_tabs,
            tab_bars: projection.tab_bars,
            leaves: projection.leaves,
            dividers: projection.dividers,
            splits: projection.splits,
            floating: projection.floating,
        })
    }

    pub fn focused_leaf(&self) -> Option<&LeafFrame> {
        self.leaves.iter().find(|leaf| leaf.focused)
    }

    pub fn focused_buffer_id(&self) -> Option<BufferId> {
        self.focused_leaf().map(|leaf| leaf.buffer_id)
    }

    pub fn focused_floating_id(&self) -> Option<FloatingId> {
        self.focused_leaf()
            .and_then(|leaf| leaf.floating_id)
            .or_else(|| {
                self.floating
                    .iter()
                    .find(|floating| floating.focused)
                    .map(|floating| floating.floating_id)
            })
    }

    pub fn focused_tabs(&self) -> Option<&TabsFrame> {
        let focused_leaf = self.focused_leaf()?;
        let tabs_node_id = focused_leaf.tabs_path.last().copied()?;
        self.tab_bars.iter().find(|bar| bar.node_id == tabs_node_id)
    }

    pub fn focus_target(&self, direction: NavigationDirection) -> Option<NodeId> {
        let focused = self.focused_leaf()?;
        let focused_context = focused.floating_id;
        let focused_center = rect_center(focused.rect);

        self.leaves
            .iter()
            .filter(|candidate| {
                candidate.node_id != focused.node_id && candidate.floating_id == focused_context
            })
            .filter_map(|candidate| {
                direction_score(focused.rect, candidate.rect, focused_center, direction)
                    .map(|score| (score, candidate.node_id))
            })
            .min_by(|left, right| left.0.cmp(&right.0))
            .map(|(_, node_id)| node_id)
    }

    pub fn leaf_at(&self, point: Point) -> Option<&LeafFrame> {
        self.leaves
            .iter()
            .rev()
            .find(|leaf| leaf.rect.contains(point))
    }

    pub fn floating_at(&self, point: Point) -> Option<&FloatingFrame> {
        self.floating
            .iter()
            .rev()
            .find(|floating| floating.rect.contains(point))
    }
}

fn node_belongs_to_session(state: &ClientState, session: &SessionRecord, node_id: NodeId) -> bool {
    let mut current = Some(node_id);
    while let Some(candidate) = current {
        let Some(node) = state.nodes.get(&candidate) else {
            return false;
        };
        if node.session_id != session.id {
            return false;
        }
        if candidate == session.root_node_id {
            return true;
        }
        if floating_window_for_root(state, session, candidate).is_some() {
            return true;
        }
        current = node.parent_id;
    }
    false
}

fn floating_window_for_root<'a>(
    state: &'a ClientState,
    session: &SessionRecord,
    root_node_id: NodeId,
) -> Option<&'a FloatingRecord> {
    session
        .floating_ids
        .iter()
        .filter_map(|floating_id| state.floating.get(floating_id))
        .find(|window| window.root_node_id == root_node_id)
}

fn floating_ancestor_for_node<'a>(
    state: &'a ClientState,
    session: &SessionRecord,
    node_id: NodeId,
) -> Option<&'a FloatingRecord> {
    let mut current = Some(node_id);
    while let Some(candidate) = current {
        if let Some(window) = floating_window_for_root(state, session, candidate) {
            return Some(window);
        }
        current = state.nodes.get(&candidate).and_then(|node| node.parent_id);
    }
    None
}

#[derive(Default)]
struct Projection {
    tab_bars: Vec<TabsFrame>,
    leaves: Vec<LeafFrame>,
    dividers: Vec<DividerFrame>,
    splits: Vec<SplitFrame>,
    floating: Vec<FloatingFrame>,
}

struct Projector<'a> {
    state: &'a ClientState,
    session: &'a SessionRecord,
    projection: &'a mut Projection,
    activity_by_node: BTreeMap<NodeId, ActivityState>,
    buffer_count_by_node: BTreeMap<NodeId, usize>,
    /// Where the root tab bar goes.
    tab_bar: TabBarPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FocusScore {
    primary: u32,
    secondary: u32,
    tertiary: u32,
}

impl Projector<'_> {
    fn project_node(
        &mut self,
        node_id: NodeId,
        rect: Rect,
        floating_id: Option<FloatingId>,
        is_root: bool,
        tabs_path: Vec<NodeId>,
    ) -> Result<()> {
        if rect.size.width == 0 || rect.size.height == 0 {
            return Ok(());
        }

        let node = self
            .state
            .nodes
            .get(&node_id)
            .ok_or_else(|| MuxError::not_found(format!("node {node_id} is not cached")))?;

        match node.kind {
            NodeRecordKind::BufferView => {
                let buffer_view = node.buffer_view.as_ref().ok_or_else(|| {
                    MuxError::protocol(format!("buffer-view node {} is missing payload", node.id))
                })?;
                let buffer = self
                    .state
                    .buffers
                    .get(&buffer_view.buffer_id)
                    .ok_or_else(|| {
                        MuxError::not_found(format!(
                            "buffer {} is not cached",
                            buffer_view.buffer_id
                        ))
                    })?;
                self.projection.leaves.push(LeafFrame {
                    node_id: node.id,
                    buffer_id: buffer.id,
                    rect,
                    title: buffer.title.clone(),
                    activity: buffer.activity,
                    focused: self.session.focused_leaf_id == Some(node.id),
                    floating_id,
                    tabs_path,
                });
                Ok(())
            }
            NodeRecordKind::Tabs => {
                let tabs = node.tabs.as_ref().ok_or_else(|| {
                    MuxError::protocol(format!("tabs node {} is missing payload", node.id))
                })?;
                let active_index = usize::try_from(tabs.active).map_err(|_| {
                    MuxError::protocol(format!(
                        "tabs node {} has active index {} that exceeds platform limits",
                        node.id, tabs.active
                    ))
                })?;
                let active_child = tabs.tabs.get(active_index).ok_or_else(|| {
                    MuxError::protocol(format!(
                        "tabs node {} has invalid active index {}",
                        node.id, tabs.active
                    ))
                })?;

                let at_bottom =
                    is_root && floating_id.is_none() && self.tab_bar == TabBarPosition::Bottom;
                let (bar_rect, content_rect) = if at_bottom {
                    let content_rect = inset_bottom(rect, 1);
                    let bar_rect = Rect {
                        origin: Point {
                            x: rect.origin.x,
                            y: rect.origin.y + i32::from(content_rect.size.height),
                        },
                        size: Size {
                            width: rect.size.width,
                            height: 1,
                        },
                    };
                    (bar_rect, content_rect)
                } else {
                    let bar_rect = Rect {
                        origin: rect.origin,
                        size: Size {
                            width: rect.size.width,
                            height: 1,
                        },
                    };
                    (bar_rect, inset_top(rect, 1))
                };
                let mut tab_items = Vec::with_capacity(tabs.tabs.len());
                for (index, tab) in tabs.tabs.iter().enumerate() {
                    tab_items.push(TabItem {
                        title: tab.title.clone(),
                        child_id: tab.child_id,
                        active: u32::try_from(index).ok() == Some(tabs.active),
                        activity: subtree_activity_cached(
                            self.state,
                            tab.child_id,
                            &mut self.activity_by_node,
                        ),
                        buffer_count: subtree_buffer_count_cached(
                            self.state,
                            tab.child_id,
                            &mut self.buffer_count_by_node,
                        ),
                        synchronize_panes: tab.synchronize_panes,
                        pane: tab_pane(self.state, tab.child_id, self.session.focused_leaf_id),
                    });
                }

                self.projection.tab_bars.push(TabsFrame {
                    node_id: node.id,
                    rect: bar_rect,
                    tabs: tab_items,
                    active: active_index,
                    is_root,
                    floating_id,
                });

                let mut child_tabs_path = tabs_path;
                child_tabs_path.push(node.id);
                self.project_node(
                    active_child.child_id,
                    content_rect,
                    floating_id,
                    false,
                    child_tabs_path,
                )
            }
            NodeRecordKind::Split => {
                let split = node.split.as_ref().ok_or_else(|| {
                    MuxError::protocol(format!("split node {} is missing payload", node.id))
                })?;
                if split.child_ids.is_empty() {
                    return Ok(());
                }

                let child_rects =
                    split_rects(rect, split.direction, &split.sizes, split.child_ids.len());
                self.projection.splits.push(SplitFrame {
                    node_id: node.id,
                    direction: split.direction,
                    rect,
                    child_ids: split.child_ids.clone(),
                    lengths: child_rects
                        .iter()
                        .map(|child| axis_length(*child, split.direction))
                        .collect(),
                    min_lengths: split
                        .child_ids
                        .iter()
                        .map(|child_id| min_extent(self.state, *child_id, split.direction))
                        .collect(),
                    floating_id,
                });
                for (index, child_id) in split.child_ids.iter().enumerate() {
                    self.project_node(
                        *child_id,
                        child_rects[index],
                        floating_id,
                        false,
                        tabs_path.clone(),
                    )?;

                    if let Some(divider_rect) = divider_rect_for(
                        split.direction,
                        child_rects[index],
                        index,
                        split.child_ids.len(),
                    ) {
                        self.projection.dividers.push(DividerFrame {
                            direction: split.direction,
                            rect: divider_rect,
                            floating_id,
                            split_node_id: node.id,
                            index,
                        });
                    }
                }

                Ok(())
            }
        }
    }
}

#[allow(dead_code)]
fn subtree_activity(state: &ClientState, node_id: NodeId) -> ActivityState {
    subtree_activity_cached(state, node_id, &mut BTreeMap::new())
}

fn subtree_activity_cached(
    state: &ClientState,
    node_id: NodeId,
    cache: &mut BTreeMap<NodeId, ActivityState>,
) -> ActivityState {
    if let Some(activity) = cache.get(&node_id).copied() {
        return activity;
    }

    let Some(node) = state.nodes.get(&node_id) else {
        return ActivityState::Idle;
    };

    let activity = match node.kind {
        NodeRecordKind::BufferView => node
            .buffer_view
            .as_ref()
            .and_then(|view| state.buffers.get(&view.buffer_id))
            .map_or(ActivityState::Idle, |buffer| buffer.activity),
        NodeRecordKind::Tabs => node
            .tabs
            .as_ref()
            .map(|tabs| {
                tabs.tabs.iter().fold(ActivityState::Idle, |activity, tab| {
                    max_activity(
                        activity,
                        subtree_activity_cached(state, tab.child_id, cache),
                    )
                })
            })
            .unwrap_or(ActivityState::Idle),
        NodeRecordKind::Split => node
            .split
            .as_ref()
            .map(|split| {
                split
                    .child_ids
                    .iter()
                    .fold(ActivityState::Idle, |activity, child_id| {
                        max_activity(activity, subtree_activity_cached(state, *child_id, cache))
                    })
            })
            .unwrap_or(ActivityState::Idle),
    };

    cache.insert(node_id, activity);
    activity
}

/// The pane a tab whose content is `child_id` shows; see [`TabPane`].
fn tab_pane(
    state: &ClientState,
    child_id: NodeId,
    focused_leaf_id: Option<NodeId>,
) -> Option<TabPane> {
    let holds_focus = |leaf_id: NodeId| {
        let mut current = leaf_id;
        loop {
            if current == child_id {
                return true;
            }
            match state.nodes.get(&current).and_then(|node| node.parent_id) {
                Some(parent_id) => current = parent_id,
                None => return false,
            }
        }
    };
    let mut leaf_id = match focused_leaf_id {
        Some(leaf_id) if holds_focus(leaf_id) => leaf_id,
        _ => child_id,
    };
    // Bounded by the node count, so a malformed cycle cannot spin forever.
    for _ in 0..=state.nodes.len() {
        let node = state.nodes.get(&leaf_id)?;
        leaf_id = match node.kind {
            NodeRecordKind::BufferView => {
                let buffer = state.buffers.get(&node.buffer_view.as_ref()?.buffer_id)?;
                return Some(TabPane {
                    title: buffer.title.clone(),
                    process_name: buffer.process_name(),
                    cwd: buffer.cwd.clone(),
                });
            }
            NodeRecordKind::Tabs => {
                let tabs = node.tabs.as_ref()?;
                tabs.tabs.get(usize::try_from(tabs.active).ok()?)?.child_id
            }
            NodeRecordKind::Split => *node.split.as_ref()?.child_ids.first()?,
        };
    }
    None
}

pub(crate) fn subtree_buffer_count(state: &ClientState, node_id: NodeId) -> usize {
    subtree_buffer_count_cached(state, node_id, &mut BTreeMap::new())
}

fn subtree_buffer_count_cached(
    state: &ClientState,
    node_id: NodeId,
    cache: &mut BTreeMap<NodeId, usize>,
) -> usize {
    if let Some(count) = cache.get(&node_id).copied() {
        return count;
    }

    let Some(node) = state.nodes.get(&node_id) else {
        return 0;
    };

    let count = match node.kind {
        NodeRecordKind::BufferView => usize::from(node.buffer_view.is_some()),
        NodeRecordKind::Tabs => node
            .tabs
            .as_ref()
            .map(|tabs| {
                tabs.tabs
                    .iter()
                    .map(|tab| subtree_buffer_count_cached(state, tab.child_id, cache))
                    .sum()
            })
            .unwrap_or(0),
        NodeRecordKind::Split => node
            .split
            .as_ref()
            .map(|split| {
                split
                    .child_ids
                    .iter()
                    .map(|child_id| subtree_buffer_count_cached(state, *child_id, cache))
                    .sum()
            })
            .unwrap_or(0),
    };

    cache.insert(node_id, count);
    count
}

fn max_activity(left: ActivityState, right: ActivityState) -> ActivityState {
    if activity_rank(right) > activity_rank(left) {
        right
    } else {
        left
    }
}

fn activity_rank(activity: ActivityState) -> u8 {
    match activity {
        ActivityState::Idle => 0,
        ActivityState::Activity => 1,
        ActivityState::Bell => 2,
    }
}

fn axis_length(rect: Rect, direction: SplitDirection) -> u16 {
    match direction {
        SplitDirection::Horizontal => rect.size.height,
        SplitDirection::Vertical => rect.size.width,
    }
}

/// The smallest extent `node_id` can be drawn at along `direction`'s axis
/// while every visible pane keeps at least one cell of content: a pane needs
/// its title row plus one row, a tab bar adds a row, and nested splits along
/// the same axis add their dividers.
pub(crate) fn min_extent(state: &ClientState, node_id: NodeId, direction: SplitDirection) -> u16 {
    let Some(node) = state.nodes.get(&node_id) else {
        return 1;
    };
    match node.kind {
        NodeRecordKind::BufferView => match direction {
            SplitDirection::Horizontal => 2,
            SplitDirection::Vertical => 1,
        },
        NodeRecordKind::Tabs => {
            let bar = match direction {
                SplitDirection::Horizontal => 1,
                SplitDirection::Vertical => 0,
            };
            let child = node.tabs.as_ref().and_then(|tabs| {
                let active = usize::try_from(tabs.active).ok()?;
                tabs.tabs.get(active).map(|tab| tab.child_id)
            });
            bar + child.map_or(1, |child_id| min_extent(state, child_id, direction))
        }
        NodeRecordKind::Split => {
            let Some(split) = node
                .split
                .as_ref()
                .filter(|split| !split.child_ids.is_empty())
            else {
                return 1;
            };
            let mins = split
                .child_ids
                .iter()
                .map(|child_id| min_extent(state, *child_id, direction));
            if split.direction == direction {
                let dividers = u16::try_from(split.child_ids.len() - 1).unwrap_or(u16::MAX);
                mins.fold(dividers, u16::saturating_add)
            } else {
                mins.max().unwrap_or(1)
            }
        }
    }
}

fn split_rects(
    rect: Rect,
    direction: SplitDirection,
    sizes: &[u16],
    child_count: usize,
) -> Vec<Rect> {
    if child_count == 0 {
        return Vec::new();
    }

    let divider_count = u16::try_from(child_count.saturating_sub(1)).unwrap_or(u16::MAX);
    let available = match direction {
        SplitDirection::Horizontal => rect.size.height.saturating_sub(divider_count),
        SplitDirection::Vertical => rect.size.width.saturating_sub(divider_count),
    };
    let lengths = proportional_lengths(available, sizes, child_count);

    let mut rects = Vec::with_capacity(child_count);
    let mut x = rect.origin.x;
    let mut y = rect.origin.y;
    for length in lengths {
        let child_rect = match direction {
            SplitDirection::Horizontal => Rect {
                origin: Point { x, y },
                size: Size {
                    width: rect.size.width,
                    height: length,
                },
            },
            SplitDirection::Vertical => Rect {
                origin: Point { x, y },
                size: Size {
                    width: length,
                    height: rect.size.height,
                },
            },
        };
        rects.push(child_rect);

        match direction {
            SplitDirection::Horizontal => {
                y += i32::from(length) + 1;
            }
            SplitDirection::Vertical => {
                x += i32::from(length) + 1;
            }
        }
    }

    rects
}

fn proportional_lengths(total: u16, sizes: &[u16], child_count: usize) -> Vec<u16> {
    if child_count == 0 {
        return Vec::new();
    }

    if total == 0 {
        return vec![0; child_count];
    }

    let weights = if sizes.len() == child_count && sizes.iter().any(|weight| *weight > 0) {
        sizes.to_vec()
    } else {
        vec![1; child_count]
    };
    let weight_sum = weights
        .iter()
        .map(|weight| u32::from(*weight))
        .sum::<u32>()
        .max(1);
    let total_u32 = u32::from(total);

    let mut lengths = vec![0_u16; child_count];
    let mut used = 0_u16;
    for (index, weight) in weights.iter().enumerate() {
        if index + 1 == child_count {
            lengths[index] = total.saturating_sub(used);
            break;
        }

        let length = ((total_u32 * u32::from(*weight)) / weight_sum) as u16;
        lengths[index] = length;
        used = used.saturating_add(length);
    }

    let mut remainder = total.saturating_sub(lengths.iter().sum::<u16>());
    let mut index = 0;
    while remainder > 0 {
        lengths[index % child_count] = lengths[index % child_count].saturating_add(1);
        remainder -= 1;
        index += 1;
    }

    lengths
}

fn divider_rect_for(
    direction: SplitDirection,
    rect: Rect,
    index: usize,
    child_count: usize,
) -> Option<Rect> {
    if index + 1 == child_count {
        return None;
    }

    match direction {
        SplitDirection::Horizontal => Some(Rect {
            origin: Point {
                x: rect.origin.x,
                y: rect.origin.y + i32::from(rect.size.height),
            },
            size: Size {
                width: rect.size.width,
                height: 1,
            },
        }),
        SplitDirection::Vertical => Some(Rect {
            origin: Point {
                x: rect.origin.x + i32::from(rect.size.width),
                y: rect.origin.y,
            },
            size: Size {
                width: 1,
                height: rect.size.height,
            },
        }),
    }
}

fn inset_bottom(rect: Rect, amount: u16) -> Rect {
    Rect {
        origin: rect.origin,
        size: Size {
            width: rect.size.width,
            height: rect.size.height.saturating_sub(amount),
        },
    }
}

fn inset_top(rect: Rect, amount: u16) -> Rect {
    let consumed = amount.min(rect.size.height);
    Rect {
        origin: Point {
            x: rect.origin.x,
            y: rect.origin.y + i32::from(consumed),
        },
        size: Size {
            width: rect.size.width,
            height: rect.size.height.saturating_sub(consumed),
        },
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::PresentationModel;
    use crate::state::ClientState;
    use embers_core::{
        ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, PtySize, SessionId, Size,
    };
    use embers_protocol::{
        BufferRecord, BufferRecordKind, BufferRecordState, BufferViewRecord, FloatingRecord,
        NodeRecord, NodeRecordKind, SessionRecord, SplitRecord, TabRecord, TabsRecord,
    };

    #[test]
    fn zoomed_node_projects_only_the_zoomed_subtree() {
        let mut state = ClientState::default();
        state.sessions.insert(
            SessionId(1),
            SessionRecord {
                id: SessionId(1),
                name: "main".to_owned(),
                root_node_id: NodeId(1),
                floating_ids: Vec::new(),
                focused_leaf_id: Some(NodeId(3)),
                focused_floating_id: None,
                zoomed_node_id: Some(NodeId(3)),
                viewport: None,
            },
        );
        state.nodes.insert(
            NodeId(1),
            NodeRecord {
                id: NodeId(1),
                session_id: SessionId(1),
                parent_id: None,
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: embers_core::SplitDirection::Vertical,
                    child_ids: vec![NodeId(2), NodeId(3)],
                    sizes: vec![1, 1],
                }),
                tabs: None,
            },
        );
        for (node_id, buffer_id, focused) in [
            (NodeId(2), BufferId(10), false),
            (NodeId(3), BufferId(11), true),
        ] {
            state.nodes.insert(
                node_id,
                NodeRecord {
                    id: node_id,
                    session_id: SessionId(1),
                    parent_id: Some(NodeId(1)),
                    kind: NodeRecordKind::BufferView,
                    buffer_view: Some(BufferViewRecord {
                        buffer_id,
                        focused,
                        zoomed: node_id == NodeId(3),
                        follow_output: true,
                        last_render_size: PtySize::new(80, 24),
                    }),
                    split: None,
                    tabs: None,
                },
            );
            state.buffers.insert(
                buffer_id,
                BufferRecord {
                    id: buffer_id,
                    title: format!("buffer-{buffer_id}"),
                    command: vec!["sh".to_owned()],
                    cwd: None,
                    kind: BufferRecordKind::Pty,
                    state: BufferRecordState::Running,
                    pid: Some(1),
                    attachment_node_id: Some(node_id),
                    read_only: false,
                    helper_source_buffer_id: None,
                    helper_scope: None,
                    pty_size: PtySize::new(80, 24),
                    activity: ActivityState::Idle,
                    last_snapshot_seq: 0,
                    exit_code: None,
                    pipe: None,
                    env: Default::default(),
                    user_options: Default::default(),
                    foreground_command: None,
                },
            );
        }

        let presentation = PresentationModel::project(
            &state,
            SessionId(1),
            Size {
                width: 80,
                height: 24,
            },
        )
        .expect("project zoomed session");
        assert_eq!(presentation.leaves.len(), 1);
        assert_eq!(presentation.leaves[0].node_id, NodeId(3));
    }

    #[test]
    fn missing_zoomed_node_falls_back_to_normal_projection() {
        let mut state = ClientState::default();
        state.sessions.insert(
            SessionId(1),
            SessionRecord {
                id: SessionId(1),
                name: "main".to_owned(),
                root_node_id: NodeId(1),
                floating_ids: Vec::new(),
                focused_leaf_id: Some(NodeId(3)),
                focused_floating_id: None,
                zoomed_node_id: Some(NodeId(99)),
                viewport: None,
            },
        );
        state.nodes.insert(
            NodeId(1),
            NodeRecord {
                id: NodeId(1),
                session_id: SessionId(1),
                parent_id: None,
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: embers_core::SplitDirection::Vertical,
                    child_ids: vec![NodeId(2), NodeId(3)],
                    sizes: vec![1, 1],
                }),
                tabs: None,
            },
        );
        for (node_id, buffer_id, focused) in [
            (NodeId(2), BufferId(10), false),
            (NodeId(3), BufferId(11), true),
        ] {
            state.nodes.insert(
                node_id,
                NodeRecord {
                    id: node_id,
                    session_id: SessionId(1),
                    parent_id: Some(NodeId(1)),
                    kind: NodeRecordKind::BufferView,
                    buffer_view: Some(BufferViewRecord {
                        buffer_id,
                        focused,
                        zoomed: false,
                        follow_output: true,
                        last_render_size: PtySize::new(80, 24),
                    }),
                    split: None,
                    tabs: None,
                },
            );
            state.buffers.insert(
                buffer_id,
                BufferRecord {
                    id: buffer_id,
                    title: format!("buffer-{buffer_id}"),
                    command: vec!["sh".to_owned()],
                    cwd: None,
                    kind: BufferRecordKind::Pty,
                    state: BufferRecordState::Running,
                    pid: Some(1),
                    attachment_node_id: Some(node_id),
                    read_only: false,
                    helper_source_buffer_id: None,
                    helper_scope: None,
                    pty_size: PtySize::new(80, 24),
                    activity: ActivityState::Idle,
                    last_snapshot_seq: 0,
                    exit_code: None,
                    pipe: None,
                    env: Default::default(),
                    user_options: Default::default(),
                    foreground_command: None,
                },
            );
        }

        let presentation = PresentationModel::project(
            &state,
            SessionId(1),
            Size {
                width: 80,
                height: 24,
            },
        )
        .expect("project with missing zoom node");

        assert_eq!(presentation.leaves.len(), 2);
        assert!(presentation.root_tabs.is_none());
        assert!(presentation.floating.is_empty());
    }

    #[test]
    fn hidden_zoomed_floating_is_not_projected() {
        let mut state = ClientState::default();
        state.sessions.insert(
            SessionId(1),
            SessionRecord {
                id: SessionId(1),
                name: "main".to_owned(),
                root_node_id: NodeId(1),
                floating_ids: vec![FloatingId(30)],
                focused_leaf_id: Some(NodeId(3)),
                focused_floating_id: Some(FloatingId(30)),
                zoomed_node_id: Some(NodeId(3)),
                viewport: None,
            },
        );
        for (node_id, buffer_id, focused, zoomed) in [
            (NodeId(1), BufferId(10), false, false),
            (NodeId(3), BufferId(11), true, true),
        ] {
            state.nodes.insert(
                node_id,
                NodeRecord {
                    id: node_id,
                    session_id: SessionId(1),
                    parent_id: None,
                    kind: NodeRecordKind::BufferView,
                    buffer_view: Some(BufferViewRecord {
                        buffer_id,
                        focused,
                        zoomed,
                        follow_output: true,
                        last_render_size: PtySize::new(80, 24),
                    }),
                    split: None,
                    tabs: None,
                },
            );
            state.buffers.insert(
                buffer_id,
                BufferRecord {
                    id: buffer_id,
                    title: format!("buffer-{buffer_id}"),
                    command: vec!["sh".to_owned()],
                    cwd: None,
                    kind: BufferRecordKind::Pty,
                    state: BufferRecordState::Running,
                    pid: Some(1),
                    attachment_node_id: Some(node_id),
                    read_only: false,
                    helper_source_buffer_id: None,
                    helper_scope: None,
                    pty_size: PtySize::new(80, 24),
                    activity: ActivityState::Idle,
                    last_snapshot_seq: 0,
                    exit_code: None,
                    pipe: None,
                    env: Default::default(),
                    user_options: Default::default(),
                    foreground_command: None,
                },
            );
        }
        state.floating.insert(
            FloatingId(30),
            FloatingRecord {
                id: FloatingId(30),
                session_id: SessionId(1),
                root_node_id: NodeId(3),
                title: Some("hidden".to_owned()),
                geometry: FloatGeometry::new(4, 4, 20, 10),
                focused: true,
                visible: false,
                close_on_empty: true,
            },
        );

        let presentation = PresentationModel::project(
            &state,
            SessionId(1),
            Size {
                width: 80,
                height: 24,
            },
        )
        .expect("project hidden zoomed floating");
        assert!(presentation.floating.is_empty());
        assert!(presentation.leaves.is_empty());
        assert!(presentation.root_tabs.is_none());
        assert_eq!(presentation.focused_floating_id(), None);
    }

    #[test]
    fn zoomed_descendant_of_floating_root_keeps_floating_projection() {
        let mut state = ClientState::default();
        state.sessions.insert(
            SessionId(1),
            SessionRecord {
                id: SessionId(1),
                name: "main".to_owned(),
                root_node_id: NodeId(1),
                floating_ids: vec![FloatingId(30)],
                focused_leaf_id: Some(NodeId(12)),
                focused_floating_id: Some(FloatingId(30)),
                zoomed_node_id: Some(NodeId(12)),
                viewport: None,
            },
        );
        state.nodes.insert(
            NodeId(1),
            NodeRecord {
                id: NodeId(1),
                session_id: SessionId(1),
                parent_id: None,
                kind: NodeRecordKind::BufferView,
                buffer_view: Some(BufferViewRecord {
                    buffer_id: BufferId(10),
                    focused: false,
                    zoomed: false,
                    follow_output: true,
                    last_render_size: PtySize::new(80, 24),
                }),
                split: None,
                tabs: None,
            },
        );
        state.nodes.insert(
            NodeId(10),
            NodeRecord {
                id: NodeId(10),
                session_id: SessionId(1),
                parent_id: None,
                kind: NodeRecordKind::Split,
                buffer_view: None,
                split: Some(SplitRecord {
                    direction: embers_core::SplitDirection::Vertical,
                    child_ids: vec![NodeId(11), NodeId(12)],
                    sizes: vec![1, 1],
                }),
                tabs: None,
            },
        );
        for (node_id, buffer_id, focused, zoomed) in [
            (NodeId(11), BufferId(11), false, false),
            (NodeId(12), BufferId(12), true, true),
        ] {
            state.nodes.insert(
                node_id,
                NodeRecord {
                    id: node_id,
                    session_id: SessionId(1),
                    parent_id: Some(NodeId(10)),
                    kind: NodeRecordKind::BufferView,
                    buffer_view: Some(BufferViewRecord {
                        buffer_id,
                        focused,
                        zoomed,
                        follow_output: true,
                        last_render_size: PtySize::new(80, 24),
                    }),
                    split: None,
                    tabs: None,
                },
            );
            state.buffers.insert(
                buffer_id,
                BufferRecord {
                    id: buffer_id,
                    title: format!("buffer-{buffer_id}"),
                    command: vec!["sh".to_owned()],
                    cwd: None,
                    kind: BufferRecordKind::Pty,
                    state: BufferRecordState::Running,
                    pid: Some(1),
                    attachment_node_id: Some(node_id),
                    read_only: false,
                    helper_source_buffer_id: None,
                    helper_scope: None,
                    pty_size: PtySize::new(80, 24),
                    activity: ActivityState::Idle,
                    last_snapshot_seq: 0,
                    exit_code: None,
                    pipe: None,
                    env: Default::default(),
                    user_options: Default::default(),
                    foreground_command: None,
                },
            );
        }
        state.buffers.insert(
            BufferId(10),
            BufferRecord {
                id: BufferId(10),
                title: "buffer-10".to_owned(),
                command: vec!["sh".to_owned()],
                cwd: None,
                kind: BufferRecordKind::Pty,
                state: BufferRecordState::Running,
                pid: Some(1),
                attachment_node_id: Some(NodeId(1)),
                read_only: false,
                helper_source_buffer_id: None,
                helper_scope: None,
                pty_size: PtySize::new(80, 24),
                activity: ActivityState::Idle,
                last_snapshot_seq: 0,
                exit_code: None,
                pipe: None,
                env: Default::default(),
                user_options: Default::default(),
                foreground_command: None,
            },
        );
        state.floating.insert(
            FloatingId(30),
            FloatingRecord {
                id: FloatingId(30),
                session_id: SessionId(1),
                root_node_id: NodeId(10),
                title: Some("popup".to_owned()),
                geometry: FloatGeometry::new(4, 4, 20, 10),
                focused: true,
                visible: true,
                close_on_empty: true,
            },
        );

        let presentation = PresentationModel::project(
            &state,
            SessionId(1),
            Size {
                width: 80,
                height: 24,
            },
        )
        .expect("project zoomed floating descendant");

        assert_eq!(presentation.floating.len(), 1);
        assert_eq!(presentation.floating[0].floating_id, FloatingId(30));
        assert_eq!(presentation.leaves.len(), 1);
        assert_eq!(presentation.leaves[0].node_id, NodeId(12));
        assert_eq!(presentation.leaves[0].floating_id, Some(FloatingId(30)));
        assert_eq!(presentation.focused_floating_id(), Some(FloatingId(30)));
    }

    #[test]
    fn zoomed_root_tabs_preserve_root_tabs_frame() {
        let mut state = ClientState::default();
        state.sessions.insert(
            SessionId(1),
            SessionRecord {
                id: SessionId(1),
                name: "main".to_owned(),
                root_node_id: NodeId(1),
                floating_ids: Vec::new(),
                focused_leaf_id: Some(NodeId(2)),
                focused_floating_id: None,
                zoomed_node_id: Some(NodeId(1)),
                viewport: None,
            },
        );
        state.nodes.insert(
            NodeId(1),
            NodeRecord {
                id: NodeId(1),
                session_id: SessionId(1),
                parent_id: None,
                kind: NodeRecordKind::Tabs,
                buffer_view: None,
                split: None,
                tabs: Some(TabsRecord {
                    active: 0,
                    tabs: vec![TabRecord {
                        title: "main".to_owned(),
                        child_id: NodeId(2),
                        synchronize_panes: false,
                    }],
                }),
            },
        );
        state.nodes.insert(
            NodeId(2),
            NodeRecord {
                id: NodeId(2),
                session_id: SessionId(1),
                parent_id: Some(NodeId(1)),
                kind: NodeRecordKind::BufferView,
                buffer_view: Some(BufferViewRecord {
                    buffer_id: BufferId(10),
                    focused: true,
                    zoomed: false,
                    follow_output: true,
                    last_render_size: PtySize::new(80, 24),
                }),
                split: None,
                tabs: None,
            },
        );
        state.buffers.insert(
            BufferId(10),
            BufferRecord {
                id: BufferId(10),
                title: "buffer-10".to_owned(),
                command: vec!["sh".to_owned()],
                cwd: None,
                kind: BufferRecordKind::Pty,
                state: BufferRecordState::Running,
                pid: Some(1),
                attachment_node_id: Some(NodeId(2)),
                read_only: false,
                helper_source_buffer_id: None,
                helper_scope: None,
                pty_size: PtySize::new(80, 24),
                activity: ActivityState::Idle,
                last_snapshot_seq: 0,
                exit_code: None,
                pipe: None,
                env: Default::default(),
                user_options: Default::default(),
                foreground_command: None,
            },
        );

        let presentation = PresentationModel::project(
            &state,
            SessionId(1),
            Size {
                width: 80,
                height: 24,
            },
        )
        .expect("project zoomed root tabs");

        let root_tabs = presentation
            .root_tabs
            .as_ref()
            .expect("root tabs frame exists");
        assert_eq!(root_tabs.node_id, NodeId(1));
        assert!(root_tabs.is_root);
    }
}

fn project_floating_window(
    projector: &mut Projector<'_>,
    window: &FloatingRecord,
    bounds: Rect,
    node_id: NodeId,
) -> Result<()> {
    if !window.visible {
        return Ok(());
    }

    let rect = clip_rect(geometry_rect(window.geometry), bounds);
    if rect.size.width == 0 || rect.size.height == 0 {
        return Ok(());
    }

    let content_rect = inset_border(rect);
    projector.projection.floating.push(FloatingFrame {
        floating_id: window.id,
        rect,
        content_rect,
        title: window.title.clone(),
        focused: window.focused,
    });
    projector.project_node(node_id, content_rect, Some(window.id), false, Vec::new())
}

fn inset_border(rect: Rect) -> Rect {
    Rect {
        origin: Point {
            x: rect.origin.x + 1,
            y: rect.origin.y + 1,
        },
        size: Size {
            width: rect.size.width.saturating_sub(2),
            height: rect.size.height.saturating_sub(2),
        },
    }
}

fn geometry_rect(geometry: FloatGeometry) -> Rect {
    Rect {
        origin: Point {
            x: i32::from(geometry.x),
            y: i32::from(geometry.y),
        },
        size: Size {
            width: geometry.width,
            height: geometry.height,
        },
    }
}

fn clip_rect(rect: Rect, bounds: Rect) -> Rect {
    let left = rect.origin.x.max(bounds.origin.x);
    let top = rect.origin.y.max(bounds.origin.y);
    let right = (rect.origin.x + i32::from(rect.size.width))
        .min(bounds.origin.x + i32::from(bounds.size.width));
    let bottom = (rect.origin.y + i32::from(rect.size.height))
        .min(bounds.origin.y + i32::from(bounds.size.height));

    if right <= left || bottom <= top {
        return Rect {
            origin: Point { x: left, y: top },
            size: Size {
                width: 0,
                height: 0,
            },
        };
    }

    Rect {
        origin: Point { x: left, y: top },
        size: Size {
            width: u16::try_from(right - left).unwrap_or(0),
            height: u16::try_from(bottom - top).unwrap_or(0),
        },
    }
}

fn rect_center(rect: Rect) -> Point {
    Point {
        x: rect.origin.x + i32::from(rect.size.width / 2),
        y: rect.origin.y + i32::from(rect.size.height / 2),
    }
}

fn direction_score(
    focused: Rect,
    candidate: Rect,
    focused_center: Point,
    direction: NavigationDirection,
) -> Option<FocusScore> {
    let candidate_center = rect_center(candidate);

    let (primary, secondary, tertiary) = match direction {
        NavigationDirection::Left => {
            let candidate_right = candidate.origin.x + i32::from(candidate.size.width);
            if candidate_right > focused.origin.x {
                return None;
            }
            (
                (focused.origin.x - candidate_right) as u32,
                (focused_center.y - candidate_center.y).unsigned_abs(),
                (focused_center.x - candidate_center.x).unsigned_abs(),
            )
        }
        NavigationDirection::Right => {
            let focused_right = focused.origin.x + i32::from(focused.size.width);
            if candidate.origin.x < focused_right {
                return None;
            }
            (
                (candidate.origin.x - focused_right) as u32,
                (focused_center.y - candidate_center.y).unsigned_abs(),
                (focused_center.x - candidate_center.x).unsigned_abs(),
            )
        }
        NavigationDirection::Up => {
            let candidate_bottom = candidate.origin.y + i32::from(candidate.size.height);
            if candidate_bottom > focused.origin.y {
                return None;
            }
            (
                (focused.origin.y - candidate_bottom) as u32,
                (focused_center.x - candidate_center.x).unsigned_abs(),
                (focused_center.y - candidate_center.y).unsigned_abs(),
            )
        }
        NavigationDirection::Down => {
            let focused_bottom = focused.origin.y + i32::from(focused.size.height);
            if candidate.origin.y < focused_bottom {
                return None;
            }
            (
                (candidate.origin.y - focused_bottom) as u32,
                (focused_center.x - candidate_center.x).unsigned_abs(),
                (focused_center.y - candidate_center.y).unsigned_abs(),
            )
        }
    };

    Some(FocusScore {
        primary,
        secondary,
        tertiary,
    })
}

#[cfg(test)]
mod tests {
    use embers_core::{Point, Rect, Size};

    use super::{NavigationDirection, direction_score};

    #[test]
    fn direction_score_rejects_candidates_outside_requested_axis() {
        let focused = Rect {
            origin: Point { x: 10, y: 5 },
            size: Size {
                width: 4,
                height: 3,
            },
        };
        let overlapping_left = Rect {
            origin: Point { x: 8, y: 5 },
            size: Size {
                width: 3,
                height: 3,
            },
        };

        assert_eq!(
            direction_score(
                focused,
                overlapping_left,
                Point { x: 12, y: 6 },
                NavigationDirection::Left
            ),
            None
        );
    }
}
