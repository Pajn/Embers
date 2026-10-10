use std::collections::BTreeMap;

use embers_core::{BufferId, FloatingId, NodeId, SessionId, SplitDirection};
use embers_protocol::{
    BufferHistoryPlacement, BufferHistoryScope, NodeBreakDestination, NodeJoinPlacement,
    SynchronizePanesMode,
};

use crate::chooser::ChooserScope;
use crate::copy_mode::{CopyMotion, JumpKind};
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
    SelectTab {
        tabs_node_id: Option<NodeId>,
        index: usize,
    },
    /// tmux `select-window -t :N`: select the root tab numbered `number`,
    /// counting from the configured base index.
    SelectWindow {
        number: u64,
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
    OpenBufferHistory {
        buffer_id: BufferId,
        scope: BufferHistoryScope,
        placement: BufferHistoryPlacement,
    },
    SplitCurrent {
        direction: SplitDirection,
        new_child: TreeSpec,
    },
    ReplaceNode {
        node_id: Option<NodeId>,
        tree: TreeSpec,
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
    /// Add a window (a root tab) to the current session, after the last one.
    NewWindow {
        title: Option<String>,
        child: TreeSpec,
    },
    OpenFloating {
        spec: FloatingSpec,
    },
    CloseFloating {
        floating_id: Option<FloatingId>,
    },
    CloseView {
        node_id: Option<NodeId>,
    },
    /// Close a floating window and end the programs of the buffers it shows.
    KillFloating {
        floating_id: Option<FloatingId>,
    },
    /// Close a view and end the programs of the buffers it shows.
    KillView {
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
        close_on_empty: bool,
    },
    ZoomNode {
        node_id: Option<NodeId>,
    },
    UnzoomNode {
        session_id: Option<SessionId>,
    },
    ToggleZoomNode {
        node_id: Option<NodeId>,
    },
    /// tmux `resize-pane -L/-R/-U/-D <cells>` on the focused pane.
    ResizePane {
        direction: NavigationDirection,
        cells: u16,
    },
    /// tmux `synchronize-panes` for the active window.
    SetSynchronizePanes {
        mode: SynchronizePanesMode,
    },
    SwapSiblingNodes {
        first_node_id: Option<NodeId>,
        second_node_id: NodeId,
    },
    BreakNode {
        node_id: Option<NodeId>,
        destination: NodeBreakDestination,
    },
    JoinBufferAtNode {
        node_id: Option<NodeId>,
        buffer_id: BufferId,
        placement: NodeJoinPlacement,
    },
    MoveNodeBefore {
        node_id: Option<NodeId>,
        sibling_node_id: NodeId,
    },
    MoveNodeAfter {
        node_id: Option<NodeId>,
        sibling_node_id: NodeId,
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
    EnterSearchBackward,
    SearchNext,
    SearchPrev,
    CommitSearch,
    CancelSearch,
    EnterCopyMode,
    ExitCopyMode,
    CopyMotion {
        motion: CopyMotion,
    },
    /// Wait for a character, then jump to it on the cursor line.
    CopyJump {
        kind: JumpKind,
    },
    CopyJumpAgain,
    CopyJumpReverse,
    EnterSelect {
        kind: SelectionKind,
    },
    SelectMove {
        direction: NavigationDirection,
    },
    SelectOtherEnd,
    CopySelection,
    CancelSelection,
    Notify {
        level: NotifyLevel,
        message: String,
    },
    RunNamedAction {
        name: String,
    },
    SwitchSession {
        name: String,
    },
    LastSession,
    NextSession,
    PrevSession,
    RunShell {
        command: Vec<String>,
    },
    /// tmux `set-environment [-g] [-u]`: set (or unset, when `value` is
    /// `None`) a variable for panes created later in the current session, or
    /// in every session when `global`.
    SetEnvironment {
        global: bool,
        name: String,
        value: Option<String>,
    },
    EnterHints {
        action: Option<String>,
    },
    /// tmux `command-prompt`: open the prompt labelled `prompt` (`:` when
    /// `None`) with `initial` text, both expanded as formats. On Enter the
    /// input runs as a command line, or `template` does with `%%` replaced by
    /// the quoted input.
    CommandPrompt {
        prompt: Option<String>,
        initial: String,
        template: Option<String>,
    },
    /// tmux `choose-tree`: open the session/window/pane picker.
    ChooseTree {
        scope: ChooserScope,
    },
    /// Run a command line as if typed at the command prompt.
    RunCommand {
        command: String,
    },
    /// tmux `display-message`: expand `message` as a format for the focused
    /// pane and show it in the status line for `duration_ms` (the default
    /// display time when `None`).
    DisplayMessage {
        message: String,
        duration_ms: Option<u32>,
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
