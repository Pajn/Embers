use std::collections::BTreeMap;

use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, MuxError, NodeId, Point, Rect, Result,
    SessionId, Size, SplitDirection,
};
use embers_protocol::{NodeRecordKind, SessionRecord};

use crate::state::ClientState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabItem {
    pub title: String,
    pub child_id: NodeId,
    pub active: bool,
    pub activity: ActivityState,
    pub buffer_count: usize,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DividerFrame {
    pub direction: SplitDirection,
    pub rect: Rect,
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
    pub floating: Vec<FloatingFrame>,
}

impl PresentationModel {
    pub fn project(state: &ClientState, session_id: SessionId, viewport: Size) -> Result<Self> {
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
        };
        if let Some(zoomed_node_id) = session.zoomed_node_id
            && state.nodes.contains_key(&zoomed_node_id)
        {
            projector.project_node(zoomed_node_id, root_bounds, None, false, Vec::new())?;
        } else {
            projector.project_node(session.root_node_id, root_bounds, None, true, Vec::new())?;

            let overlay_bounds = root_bounds;
            for floating_id in &session.floating_ids {
                let Some(window) = state.floating.get(floating_id) else {
                    continue;
                };
                if !window.visible {
                    continue;
                }

                let rect = clip_rect(geometry_rect(window.geometry), overlay_bounds);
                if rect.size.width == 0 || rect.size.height == 0 {
                    continue;
                }

                let content_rect = inset_border(rect);
                projector.projection.floating.push(FloatingFrame {
                    floating_id: window.id,
                    rect,
                    content_rect,
                    title: window.title.clone(),
                    focused: window.focused,
                });

                projector.project_node(
                    window.root_node_id,
                    content_rect,
                    Some(window.id),
                    false,
                    Vec::new(),
                )?;
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

#[derive(Default)]
struct Projection {
    tab_bars: Vec<TabsFrame>,
    leaves: Vec<LeafFrame>,
    dividers: Vec<DividerFrame>,
    floating: Vec<FloatingFrame>,
}

struct Projector<'a> {
    state: &'a ClientState,
    session: &'a SessionRecord,
    projection: &'a mut Projection,
    activity_by_node: BTreeMap<NodeId, ActivityState>,
    buffer_count_by_node: BTreeMap<NodeId, usize>,
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

                let bar_rect = Rect {
                    origin: rect.origin,
                    size: Size {
                        width: rect.size.width,
                        height: 1,
                    },
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
                    inset_top(rect, 1),
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
    use embers_core::{ActivityState, BufferId, NodeId, PtySize, SessionId, Size};
    use embers_protocol::{
        BufferRecord, BufferRecordKind, BufferRecordState, BufferViewRecord, NodeRecord,
        NodeRecordKind, SessionRecord, SplitRecord,
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
                    env: Default::default(),
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
