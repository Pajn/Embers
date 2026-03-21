use embers_core::{BufferId, FloatGeometry, NodeId, SplitDirection};

use crate::presentation::NavigationDirection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    EnterMode { mode: String },
    Focus { direction: NavigationDirection },
    Resize { direction: NavigationDirection, amount: u16 },
    SelectTab { index: usize },
    Split { direction: SplitDirection, tree: TreeSpec },
    ReplaceCurrentWith { tree: TreeSpec },
    ReplaceNode { target: NodeTarget, tree: TreeSpec },
    WrapCurrentInSplit { direction: SplitDirection, tree: TreeSpec },
    WrapCurrentInTabs { tabs: Vec<TabSpec>, active: usize },
    InsertTabAfterCurrent { title: String, tree: TreeSpec },
    OpenFloating { tree: TreeSpec, options: FloatingOptions },
    DetachBuffer { target: BufferTarget },
    KillBuffer { target: BufferTarget, force: bool },
    SendBytes { target: BufferTarget, bytes: Vec<u8> },
    Notify { message: String },
    ReloadConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferTarget {
    Current,
    Buffer(BufferId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeTarget {
    Current,
    Node(NodeId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferSpawnSpec {
    pub title: Option<String>,
    pub command: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingOptions {
    pub geometry: FloatGeometry,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightedTreeSpec {
    pub weight: u16,
    pub tree: Box<TreeSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabSpec {
    pub title: String,
    pub tree: Box<TreeSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeSpec {
    BufferSpawn(BufferSpawnSpec),
    BufferAttach { buffer_id: BufferId },
    BufferCurrent,
    CurrentNode,
    BufferEmpty,
    Split {
        direction: SplitDirection,
        children: Vec<WeightedTreeSpec>,
    },
    Tabs {
        tabs: Vec<TabSpec>,
        active: usize,
    },
}
