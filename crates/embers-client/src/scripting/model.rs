use std::collections::BTreeMap;

use embers_core::{BufferId, FloatingId, NodeId, SplitDirection};

use crate::input::KeySequence;
use crate::presentation::NavigationDirection;
use crate::state::SelectionKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Noop,
    Chain(Vec<Action>),
    EnterMode {
        mode: String,
    },
    LeaveMode,
    ToggleMode {
        mode: String,
    },
    ClearPendingKeys,
    FocusDirection {
        direction: NavigationDirection,
    },
    ResizeDirection {
        direction: NavigationDirection,
        amount: u16,
    },
    SelectTab {
        tabs_node_id: Option<NodeId>,
        index: usize,
    },
    NextTab {
        tabs_node_id: Option<NodeId>,
    },
    PrevTab {
        tabs_node_id: Option<NodeId>,
    },
    FocusBuffer {
        buffer_id: BufferId,
    },
    RevealBuffer {
        buffer_id: BufferId,
    },
    SplitCurrent {
        direction: SplitDirection,
        new_child: TreeSpec,
    },
    ReplaceNode {
        node_id: Option<NodeId>,
        tree: TreeSpec,
    },
    WrapNodeInSplit {
        node_id: Option<NodeId>,
        direction: SplitDirection,
        sibling: TreeSpec,
    },
    WrapNodeInTabs {
        node_id: Option<NodeId>,
        tabs: TabsSpec,
    },
    InsertTabAfter {
        tabs_node_id: Option<NodeId>,
        title: Option<String>,
        child: TreeSpec,
    },
    InsertTabBefore {
        tabs_node_id: Option<NodeId>,
        title: Option<String>,
        child: TreeSpec,
    },
    OpenFloating {
        spec: FloatingSpec,
    },
    ReplaceFloatingRoot {
        floating_id: Option<FloatingId>,
        tree: TreeSpec,
    },
    CloseFloating {
        floating_id: Option<FloatingId>,
    },
    CloseView {
        node_id: Option<NodeId>,
    },
    KillBuffer {
        buffer_id: Option<BufferId>,
    },
    DetachBuffer {
        buffer_id: Option<BufferId>,
    },
    MoveBufferToNode {
        buffer_id: BufferId,
        node_id: NodeId,
    },
    MoveBufferToFloating {
        buffer_id: BufferId,
        geometry: FloatingGeometrySpec,
        title: Option<String>,
        focus: bool,
    },
    SendKeys {
        buffer_id: Option<BufferId>,
        keys: KeySequence,
    },
    SendBytes {
        buffer_id: Option<BufferId>,
        bytes: Vec<u8>,
    },
    ScrollLineUp,
    ScrollLineDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    FollowOutput,
    EnterSearchMode,
    SearchNext,
    SearchPrev,
    CancelSearch,
    EnterSelect {
        kind: SelectionKind,
    },
    SelectMove {
        direction: NavigationDirection,
    },
    CopySelection,
    CancelSelection,
    Notify {
        level: NotifyLevel,
        message: String,
    },
    RunNamedAction {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferSpawnSpec {
    pub title: Option<String>,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingSpec {
    pub tree: TreeSpec,
    pub geometry: FloatingGeometrySpec,
    pub title: Option<String>,
    pub focus: bool,
    pub close_on_empty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingGeometrySpec {
    pub width: FloatingSize,
    pub height: FloatingSize,
    pub anchor: FloatingAnchor,
    pub offset_x: i16,
    pub offset_y: i16,
}

impl Default for FloatingGeometrySpec {
    fn default() -> Self {
        Self {
            width: FloatingSize::Percent(50),
            height: FloatingSize::Percent(50),
            anchor: FloatingAnchor::Center,
            offset_x: 0,
            offset_y: 0,
        }
    }
}

/// Floating sizes expressed as percentages are resolved in the inclusive
/// `1..=100` range when converted into concrete geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatingSize {
    Cells(u16),
    Percent(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatingAnchor {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabSpec {
    pub title: String,
    pub tree: Box<TreeSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsSpec {
    pub tabs: Vec<TabSpec>,
    pub active: usize,
}

impl TabsSpec {
    pub fn try_new(tabs: Vec<TabSpec>, active: usize) -> Result<Self, String> {
        if tabs.is_empty() {
            return Err("tabs cannot be empty".to_owned());
        }
        if active >= tabs.len() {
            return Err("active tab index is out of bounds".to_owned());
        }
        Ok(Self { tabs, active })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeSpec {
    BufferCurrent,
    BufferAttach {
        buffer_id: BufferId,
    },
    BufferSpawn(BufferSpawnSpec),
    BufferEmpty,
    CurrentNode,
    Split {
        direction: SplitDirection,
        children: Vec<TreeSpec>,
        sizes: Vec<u16>,
    },
    Tabs(TabsSpec),
}
