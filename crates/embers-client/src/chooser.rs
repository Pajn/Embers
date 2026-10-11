//! The session, window and pane picker (tmux `choose-tree`).
//!
//! The picker is a collapsible tree of every session, its windows (root tabs)
//! and their panes, built from the client's state each time it is drawn so it
//! follows changes while open. Rows are identified by [`ChooserItem`], so the
//! selection stays on the same session, window or pane as the tree changes.

use std::collections::BTreeSet;

use embers_core::{NodeId, Point, Rect, SessionId, Size};
use embers_protocol::{NodeRecordKind, TabsRecord};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::grid::{BorderStyle, CellStyle, RenderGrid};
use crate::numbering::window_number;
use crate::state::ClientState;

/// What the picker starts showing, like tmux `choose-tree`'s `-s` and `-w`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooserScope {
    /// Sessions, windows and panes, all expanded.
    Tree,
    /// Sessions expanded to their windows (tmux `choose-tree -w`).
    Windows,
    /// Sessions only, collapsed (tmux `choose-tree -s`).
    Sessions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChooserItem {
    Session(SessionId),
    /// A window by its root tab's child node (the session's root node when
    /// the root is not tabs). Its position is looked up only when it is
    /// acted on, as windows before it may close while the picker is open.
    Window(SessionId, NodeId),
    Pane(SessionId, NodeId),
}

impl ChooserItem {
    pub fn session_id(self) -> SessionId {
        match self {
            Self::Session(session_id) | Self::Window(session_id, _) | Self::Pane(session_id, _) => {
                session_id
            }
        }
    }

    /// What the item is, for messages: "session", "window" or "pane".
    pub fn kind(self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Window(..) => "window",
            Self::Pane(..) => "pane",
        }
    }

    /// Whether the item is still in `state`.
    pub fn exists(self, state: &ClientState) -> bool {
        match self {
            Self::Session(session_id) => state.sessions.contains_key(&session_id),
            Self::Window(session_id, window_id) => {
                window_position(state, session_id, window_id).is_some()
            }
            Self::Pane(session_id, node_id) => state.nodes.get(&node_id).is_some_and(|node| {
                node.session_id == session_id && node.kind == NodeRecordKind::BufferView
            }),
        }
    }
}

/// The current position of the window `window_id` (a [`ChooserItem::Window`]'s
/// node) among `session_id`'s windows, or `None` when it has closed.
pub fn window_position(
    state: &ClientState,
    session_id: SessionId,
    window_id: NodeId,
) -> Option<usize> {
    let session = state.sessions.get(&session_id)?;
    match root_tabs(state, session.root_node_id) {
        Some(tabs) => tabs.tabs.iter().position(|tab| tab.child_id == window_id),
        None => (session.root_node_id == window_id).then_some(0),
    }
}

/// The session's windows: its root node's tabs, when the root is tabs.
fn root_tabs(state: &ClientState, root_id: NodeId) -> Option<&TabsRecord> {
    state.nodes.get(&root_id).and_then(|root| {
        root.tabs
            .as_ref()
            .filter(|_| root.kind == NodeRecordKind::Tabs)
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChooserRow {
    pub item: ChooserItem,
    pub depth: usize,
    pub label: String,
    pub has_children: bool,
    pub expanded: bool,
    /// The client's current session, window or pane.
    pub current: bool,
}

/// An open picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chooser {
    pub scope: ChooserScope,
    expanded: BTreeSet<ChooserItem>,
    selected: Option<ChooserItem>,
    /// Text rows must contain (case-insensitively), with their ancestors.
    pub filter: String,
    /// Keys edit the filter (after `/`) instead of driving the picker.
    pub editing_filter: bool,
    /// The item `x` asked to kill, waiting for `y`.
    pub confirm_kill: Option<ChooserItem>,
    /// First row shown, so the selection stays in view.
    scroll: usize,
    /// Mode to return to when the picker closes.
    pub return_mode: String,
}

/// One node of the full tree, before collapsing and filtering.
struct TreeNode {
    row: ChooserRow,
    children: Vec<TreeNode>,
}

impl Chooser {
    /// Open the picker with `scope`'s rows expanded and the client's current
    /// item (`current_session`'s window or focused pane) selected.
    pub fn open(
        scope: ChooserScope,
        state: &ClientState,
        current_session: Option<SessionId>,
        base_index: Option<u32>,
        return_mode: impl Into<String>,
    ) -> Self {
        let mut chooser = Self {
            scope,
            expanded: BTreeSet::new(),
            selected: None,
            filter: String::new(),
            editing_filter: false,
            confirm_kill: None,
            scroll: 0,
            return_mode: return_mode.into(),
        };
        let tree = build_tree(state, current_session, base_index);
        for session in &tree {
            match scope {
                ChooserScope::Sessions => {}
                ChooserScope::Windows => {
                    chooser.expanded.insert(session.row.item);
                }
                ChooserScope::Tree => {
                    chooser.expanded.insert(session.row.item);
                    for window in &session.children {
                        chooser.expanded.insert(window.row.item);
                    }
                }
            }
        }
        // Select the deepest current row that is shown.
        chooser.selected = chooser
            .rows(state, current_session, base_index)
            .iter()
            .filter(|row| row.current)
            .max_by_key(|row| row.depth)
            .map(|row| row.item);
        chooser
    }

    /// The rows to show, in order: expanded rows' children follow them; with
    /// a filter, only matching rows and their ancestors, all expanded.
    pub fn rows(
        &self,
        state: &ClientState,
        current_session: Option<SessionId>,
        base_index: Option<u32>,
    ) -> Vec<ChooserRow> {
        let tree = build_tree(state, current_session, base_index);
        let filter = self.filter.to_lowercase();
        let mut rows = Vec::new();
        for node in &tree {
            if filter.is_empty() {
                self.flatten(node, &mut rows);
            } else {
                flatten_matching(node, &filter, &mut rows);
            }
        }
        rows
    }

    fn flatten(&self, node: &TreeNode, rows: &mut Vec<ChooserRow>) {
        let expanded = self.expanded.contains(&node.row.item);
        let mut row = node.row.clone();
        row.expanded = expanded && row.has_children;
        rows.push(row);
        if expanded {
            for child in &node.children {
                self.flatten(child, rows);
            }
        }
    }

    /// The index of the selected row in `rows`, falling back to the first row
    /// when the selected item is gone or filtered out.
    pub fn selected_index(&self, rows: &[ChooserRow]) -> Option<usize> {
        if rows.is_empty() {
            return None;
        }
        Some(
            self.selected
                .and_then(|item| rows.iter().position(|row| row.item == item))
                .unwrap_or(0),
        )
    }

    pub fn selected(&self, rows: &[ChooserRow]) -> Option<ChooserItem> {
        self.selected_index(rows).map(|index| rows[index].item)
    }

    /// The item Enter and `x` act on. A selected item that has left `state`
    /// is returned as it is, so the action finds it gone instead of acting on
    /// the row highlighted in its place; one that is only filtered out gives
    /// way to the highlighted row.
    pub fn target(&self, state: &ClientState, rows: &[ChooserRow]) -> Option<ChooserItem> {
        match self.selected {
            Some(item) if !item.exists(state) => Some(item),
            _ => self.selected(rows),
        }
    }

    /// Move the selection by `delta` rows, stopping at the ends.
    pub fn move_by(&mut self, rows: &[ChooserRow], delta: isize) {
        let Some(index) = self.selected_index(rows) else {
            return;
        };
        let last = rows.len() as isize - 1;
        let next = (index as isize + delta).clamp(0, last) as usize;
        self.selected = Some(rows[next].item);
    }

    pub fn select_first(&mut self, rows: &[ChooserRow]) {
        self.selected = rows.first().map(|row| row.item);
    }

    pub fn select_last(&mut self, rows: &[ChooserRow]) {
        self.selected = rows.last().map(|row| row.item);
    }

    /// Expand the selected row, or move into its first child when it is
    /// already expanded.
    pub fn expand(&mut self, rows: &[ChooserRow]) {
        let Some(index) = self.selected_index(rows) else {
            return;
        };
        let row = &rows[index];
        if !row.has_children {
            return;
        }
        if row.expanded {
            self.move_by(rows, 1);
        } else {
            self.expanded.insert(row.item);
        }
    }

    /// Collapse the selected row, or move to its parent when it is collapsed.
    pub fn collapse(&mut self, rows: &[ChooserRow]) {
        let Some(index) = self.selected_index(rows) else {
            return;
        };
        let row = &rows[index];
        if row.expanded {
            self.expanded.remove(&row.item);
            return;
        }
        if let Some(parent) = rows[..index]
            .iter()
            .rev()
            .find(|candidate| candidate.depth < row.depth)
        {
            self.selected = Some(parent.item);
        }
    }

    /// Draw the picker over `area` of `grid`: a box with one line per row,
    /// the selection highlighted, and a footer for the filter, a kill
    /// confirmation or key hints.
    pub fn render(&mut self, grid: &mut RenderGrid, area: Rect, rows: &[ChooserRow]) {
        if area.size.width < 8 || area.size.height < 4 {
            return;
        }
        let x = area.origin.x;
        let y = area.origin.y;
        let width = area.size.width;
        let height = area.size.height;
        grid.fill_rect(area, ' ', CellStyle::default());
        grid.draw_box_styled(area, BorderStyle::ASCII, CellStyle::default().with_bold());
        let title = match self.scope {
            ChooserScope::Tree => " choose-tree ",
            ChooserScope::Windows => " choose-window ",
            ChooserScope::Sessions => " choose-session ",
        };
        grid.put_str_styled(
            clamp(x + 2),
            clamp(y),
            &fit(title, width.saturating_sub(4)),
            CellStyle::default().with_bold(),
        );

        let inner_width = width.saturating_sub(2);
        let list_height = usize::from(height.saturating_sub(3));
        let selected = self.selected_index(rows);
        if let Some(selected) = selected {
            if selected < self.scroll {
                self.scroll = selected;
            } else if selected >= self.scroll + list_height {
                self.scroll = selected + 1 - list_height;
            }
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(list_height));

        for (line, (index, row)) in rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_height)
            .enumerate()
        {
            let marker = match (row.has_children, row.expanded) {
                (true, true) => "- ",
                (true, false) => "+ ",
                (false, _) => "  ",
            };
            let current = if row.current { " *" } else { "" };
            let text = format!(" {}{marker}{}{current}", "  ".repeat(row.depth), row.label);
            let style = if Some(index) == selected {
                CellStyle::default().with_reverse()
            } else {
                CellStyle::default()
            };
            let text = pad(&fit(&text, inner_width), inner_width);
            grid.put_str_styled(clamp(x + 1), clamp(y + 1 + line as i32), &text, style);
        }
        if rows.is_empty() {
            grid.put_str(
                clamp(x + 1),
                clamp(y + 1),
                &fit(" (no matches)", inner_width),
            );
        }

        let footer = if let Some(item) = self.confirm_kill {
            let label = rows
                .iter()
                .find(|row| row.item == item)
                .map(|row| row.label.as_str())
                .unwrap_or_default();
            format!(" kill {} {label}? (y/n)", item.kind())
        } else if self.editing_filter || !self.filter.is_empty() {
            format!(" filter: {}", self.filter)
        } else {
            " enter select  x kill  / filter  h/l fold  q quit".to_owned()
        };
        let footer_style = if self.confirm_kill.is_some() {
            CellStyle::default().with_bold()
        } else {
            CellStyle::default()
        };
        grid.put_str_styled(
            clamp(x + 1),
            clamp(y + i32::from(height) - 2),
            &pad(&fit(&footer, inner_width), inner_width),
            footer_style,
        );
    }
}

/// The area the picker is drawn in within `viewport`: most of it, centered.
pub fn chooser_area(viewport: Size) -> Rect {
    let width = (viewport.width.saturating_mul(4) / 5)
        .max(viewport.width.min(40))
        .min(viewport.width);
    let height = (viewport.height.saturating_mul(4) / 5)
        .max(viewport.height.min(10))
        .min(viewport.height);
    Rect {
        origin: Point {
            x: i32::from((viewport.width - width) / 2),
            y: i32::from((viewport.height - height) / 2),
        },
        size: Size { width, height },
    }
}

fn flatten_matching(node: &TreeNode, filter: &str, rows: &mut Vec<ChooserRow>) -> bool {
    let mut children = Vec::new();
    let mut child_matches = false;
    for child in &node.children {
        child_matches |= flatten_matching(child, filter, &mut children);
    }
    let matches = node.row.label.to_lowercase().contains(filter);
    if !matches && !child_matches {
        return false;
    }
    let mut row = node.row.clone();
    row.expanded = child_matches;
    rows.push(row);
    rows.extend(children);
    true
}

fn build_tree(
    state: &ClientState,
    current_session: Option<SessionId>,
    base_index: Option<u32>,
) -> Vec<TreeNode> {
    state
        .sessions
        .values()
        .map(|session| {
            let is_current_session = current_session == Some(session.id);
            let windows: Vec<TreeNode> = match root_tabs(state, session.root_node_id) {
                Some(tabs) => tabs
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(position, tab)| {
                        let is_current = is_current_session && position == tabs.active as usize;
                        window_node(
                            state,
                            session.id,
                            position,
                            &tab.title,
                            tab.child_id,
                            is_current,
                            session.focused_leaf_id,
                            base_index,
                        )
                    })
                    .collect(),
                // A session whose root is not tabs is one window, named
                // after its pane as `list-windows` names it.
                None => vec![window_node(
                    state,
                    session.id,
                    0,
                    &untabbed_window_title(state, session.root_node_id, session.focused_leaf_id),
                    session.root_node_id,
                    is_current_session,
                    session.focused_leaf_id,
                    base_index,
                )],
            };
            let count = windows.len();
            TreeNode {
                row: ChooserRow {
                    item: ChooserItem::Session(session.id),
                    depth: 0,
                    label: format!(
                        "{}: {count} window{}",
                        session.name,
                        if count == 1 { "" } else { "s" }
                    ),
                    has_children: count > 0,
                    expanded: false,
                    current: is_current_session,
                },
                children: windows,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn window_node(
    state: &ClientState,
    session_id: SessionId,
    position: usize,
    title: &str,
    child_id: NodeId,
    is_current: bool,
    focused_leaf: Option<NodeId>,
    base_index: Option<u32>,
) -> TreeNode {
    let mut leaves = Vec::new();
    collect_leaves(state, child_id, &mut leaves);
    let panes: Vec<TreeNode> = leaves
        .into_iter()
        .map(|leaf_id| TreeNode {
            row: ChooserRow {
                item: ChooserItem::Pane(session_id, leaf_id),
                depth: 2,
                label: pane_label(state, leaf_id),
                has_children: false,
                expanded: false,
                current: is_current && focused_leaf == Some(leaf_id),
            },
            children: Vec::new(),
        })
        .collect();
    let count = panes.len();
    TreeNode {
        row: ChooserRow {
            item: ChooserItem::Window(session_id, child_id),
            depth: 1,
            label: format!(
                "{}: {title} ({count} pane{})",
                window_number(position, base_index),
                if count == 1 { "" } else { "s" }
            ),
            has_children: count > 0,
            expanded: false,
            current: is_current,
        },
        children: panes,
    }
}

fn untabbed_window_title(
    state: &ClientState,
    root_id: NodeId,
    focused_leaf: Option<NodeId>,
) -> String {
    let mut leaves = Vec::new();
    collect_leaves(state, root_id, &mut leaves);
    focused_leaf
        .filter(|leaf| leaves.contains(leaf))
        .or_else(|| leaves.first().copied())
        .and_then(|leaf| state.nodes.get(&leaf))
        .and_then(|node| node.buffer_view.as_ref())
        .and_then(|view| state.buffers.get(&view.buffer_id))
        .map(|buffer| buffer.title.clone())
        .unwrap_or_default()
}

/// The panes under `node_id`, in layout order, including those in hidden
/// nested tabs.
fn collect_leaves(state: &ClientState, node_id: NodeId, leaves: &mut Vec<NodeId>) {
    let Some(node) = state.nodes.get(&node_id) else {
        return;
    };
    match node.kind {
        NodeRecordKind::BufferView => leaves.push(node_id),
        NodeRecordKind::Split => {
            for child in node.split.iter().flat_map(|split| &split.child_ids) {
                collect_leaves(state, *child, leaves);
            }
        }
        NodeRecordKind::Tabs => {
            for tab in node.tabs.iter().flat_map(|tabs| &tabs.tabs) {
                collect_leaves(state, tab.child_id, leaves);
            }
        }
    }
}

fn pane_label(state: &ClientState, leaf_id: NodeId) -> String {
    let buffer = state
        .nodes
        .get(&leaf_id)
        .and_then(|node| node.buffer_view.as_ref())
        .and_then(|view| state.buffers.get(&view.buffer_id));
    match buffer {
        Some(buffer) => {
            let command = buffer.process_name().unwrap_or_default();
            format!("{leaf_id}: {} [{command}]", buffer.title)
        }
        None => leaf_id.to_string(),
    }
}

fn clamp(value: i32) -> u16 {
    value.clamp(0, i32::from(u16::MAX)) as u16
}

/// Cut `text` to `width` display columns.
fn fit(text: &str, width: u16) -> String {
    let width = usize::from(width);
    let mut used = 0;
    let mut output = String::new();
    for ch in text.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        used += ch_width;
        output.push(ch);
    }
    output
}

/// Pad `text` with spaces to `width` display columns.
fn pad(text: &str, width: u16) -> String {
    let used = UnicodeWidthStr::width(text);
    format!(
        "{text}{}",
        " ".repeat(usize::from(width).saturating_sub(used))
    )
}
