use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, PtySize, SessionId, Size,
    SplitDirection,
};
use embers_protocol::{
    BufferRecord, BufferRecordKind, BufferRecordState, BufferViewRecord, FloatingRecord,
    NodeRecord, NodeRecordKind, SplitRecord, TabRecord, TabsRecord,
};

use crate::presentation::{LayoutOptions, PaneTitles, PresentationModel, RootTabBar};
use crate::state::ClientState;

/// Where a pane about to be created will be placed, mirroring the server
/// request that places it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanePlacement {
    /// `NodeRequest::Split` of `leaf_id`, which puts the new pane second in an
    /// even split.
    Split {
        leaf_id: NodeId,
        direction: SplitDirection,
    },
    /// `SessionRequest::AddRootTab`: a new active window.
    RootTab,
    /// `NodeRequest::AddTab` on `tabs_node_id`, which activates the new tab.
    Tab { tabs_node_id: NodeId },
    /// `FloatingRequest::Create` with `geometry`.
    Floating { geometry: FloatGeometry },
    /// `NodeRequest::MoveBufferToNode` into `leaf_id`, which keeps its place.
    Replace { leaf_id: NodeId },
}

/// The PTY size a pane created at `placement` in `session_id` will be drawn
/// at in `viewport`, so its buffer can be spawned at that size rather than
/// resized once a client draws it.
///
/// The placement is applied to a copy of the session's layout and projected
/// like a frame, so the result includes all chrome the client draws. Only the
/// structural change is mirrored: focus and zoom stay as they are, so a pane
/// hidden behind a zoom is not drawn. `None` when the session or target is
/// unknown or the new pane would not be drawn, in which case the server
/// default applies and the client resizes the pane when it shows it.
///
/// The root tab bar is taken to be shown. It takes one row at the top or the
/// bottom alike ([`crate::TabBarPosition`]), so sizes do not depend on where
/// the client draws it. A client that hides it reports a viewport with its
/// row added ([`PresentationModel::layout_viewport`]), so callers without the
/// client's config, given that viewport, get the same answer as
/// [`spawn_pty_size_with`] gives the client. Title rows do change sizes, so
/// `pane_titles` is the client's; callers without its config take what it
/// reported ([`PaneTitles::reported`]).
pub fn spawn_pty_size(
    state: &ClientState,
    session_id: SessionId,
    viewport: Size,
    pane_titles: PaneTitles,
    placement: PanePlacement,
) -> Option<PtySize> {
    spawn_pty_size_with(
        state,
        session_id,
        viewport,
        LayoutOptions::new(RootTabBar::default(), pane_titles),
        placement,
    )
}

/// [`spawn_pty_size`] for a client that lays the session out as `options`
/// says, in the viewport it draws the session in.
pub fn spawn_pty_size_with(
    state: &ClientState,
    session_id: SessionId,
    viewport: Size,
    options: impl Into<LayoutOptions>,
    placement: PanePlacement,
) -> Option<PtySize> {
    let options = options.into();
    let mut layout = session_layout(state, session_id)?;
    add_new_leaf(&mut layout, session_id);
    let drawn_leaf = match placement {
        PanePlacement::Split { leaf_id, direction } => {
            split_leaf(&mut layout, session_id, leaf_id, direction)?;
            NEW_LEAF
        }
        PanePlacement::RootTab => {
            let tabs_node_id = root_tabs(&mut layout, session_id)?;
            add_active_tab(&mut layout, tabs_node_id)?;
            NEW_LEAF
        }
        PanePlacement::Tab { tabs_node_id } => {
            if layout.nodes.get(&tabs_node_id)?.session_id != session_id {
                return None;
            }
            add_active_tab(&mut layout, tabs_node_id)?;
            NEW_LEAF
        }
        PanePlacement::Floating { geometry } => {
            add_floating(&mut layout, session_id, geometry)?;
            NEW_LEAF
        }
        PanePlacement::Replace { leaf_id } => leaf_id,
    };

    PresentationModel::project_with(&layout, session_id, viewport, options)
        .ok()?
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == drawn_leaf)?
        .pty_size()
}

// Ids for the records the simulated placement adds. Real ids count up from 1,
// so these never collide with an existing record.
const NEW_LEAF: NodeId = NodeId(u64::MAX);
const NEW_CONTAINER: NodeId = NodeId(u64::MAX - 1);
const NEW_BUFFER: BufferId = BufferId(u64::MAX);
const NEW_FLOATING: FloatingId = FloatingId(u64::MAX);

/// The records needed to project `session_id`, without snapshots or view
/// state.
fn session_layout(state: &ClientState, session_id: SessionId) -> Option<ClientState> {
    let session = state.sessions.get(&session_id)?.clone();
    let mut layout = ClientState::default();
    for (node_id, node) in &state.nodes {
        if node.session_id != session_id {
            continue;
        }
        if let Some(view) = &node.buffer_view
            && let Some(buffer) = state.buffers.get(&view.buffer_id)
        {
            layout.buffers.insert(buffer.id, buffer.clone());
        }
        layout.nodes.insert(*node_id, node.clone());
    }
    for floating_id in &session.floating_ids {
        if let Some(window) = state.floating.get(floating_id) {
            layout.floating.insert(*floating_id, window.clone());
        }
    }
    layout.sessions.insert(session_id, session);
    Some(layout)
}

/// Adds the new pane's leaf and buffer, not yet attached anywhere.
fn add_new_leaf(layout: &mut ClientState, session_id: SessionId) {
    layout.buffers.insert(
        NEW_BUFFER,
        BufferRecord {
            id: NEW_BUFFER,
            title: String::new(),
            command: Vec::new(),
            cwd: None,
            pipe: None,
            kind: BufferRecordKind::Pty,
            state: BufferRecordState::Created,
            pid: None,
            attachment_node_id: Some(NEW_LEAF),
            read_only: false,
            helper_source_buffer_id: None,
            helper_scope: None,
            pty_size: PtySize::default(),
            activity: ActivityState::Idle,
            last_snapshot_seq: 0,
            exit_code: None,
            env: Default::default(),
            user_options: Default::default(),
            foreground_command: None,
        },
    );
    layout.nodes.insert(
        NEW_LEAF,
        NodeRecord {
            id: NEW_LEAF,
            session_id,
            parent_id: None,
            kind: NodeRecordKind::BufferView,
            buffer_view: Some(BufferViewRecord {
                buffer_id: NEW_BUFFER,
                focused: false,
                zoomed: false,
                follow_output: true,
                last_render_size: PtySize::default(),
            }),
            split: None,
            tabs: None,
            focus_leaf_id: None,
        },
    );
}

/// Wraps `leaf_id` in an even split with the new leaf second.
fn split_leaf(
    layout: &mut ClientState,
    session_id: SessionId,
    leaf_id: NodeId,
    direction: SplitDirection,
) -> Option<()> {
    let leaf = layout.nodes.get(&leaf_id)?;
    if leaf.session_id != session_id || leaf.kind != NodeRecordKind::BufferView {
        return None;
    }
    let parent_id = leaf.parent_id;
    replace_child(layout, session_id, parent_id, leaf_id, NEW_CONTAINER)?;
    layout.nodes.insert(
        NEW_CONTAINER,
        NodeRecord {
            id: NEW_CONTAINER,
            session_id,
            parent_id,
            kind: NodeRecordKind::Split,
            buffer_view: None,
            split: Some(SplitRecord {
                direction,
                child_ids: vec![leaf_id, NEW_LEAF],
                sizes: vec![1, 1],
            }),
            tabs: None,
            focus_leaf_id: None,
        },
    );
    set_parent(layout, leaf_id, NEW_CONTAINER);
    set_parent(layout, NEW_LEAF, NEW_CONTAINER);
    Some(())
}

/// The session's root tabs container, wrapping the root in one first when it
/// is not, as the server does before adding a root tab.
fn root_tabs(layout: &mut ClientState, session_id: SessionId) -> Option<NodeId> {
    let root_id = layout.sessions.get(&session_id)?.root_node_id;
    if layout.nodes.get(&root_id)?.kind == NodeRecordKind::Tabs {
        return Some(root_id);
    }
    layout.nodes.insert(
        NEW_CONTAINER,
        NodeRecord {
            id: NEW_CONTAINER,
            session_id,
            parent_id: None,
            kind: NodeRecordKind::Tabs,
            buffer_view: None,
            split: None,
            tabs: Some(TabsRecord {
                active: 0,
                tabs: vec![TabRecord {
                    title: String::new(),
                    child_id: root_id,
                    synchronize_panes: false,
                }],
            }),
            focus_leaf_id: None,
        },
    );
    set_parent(layout, root_id, NEW_CONTAINER);
    layout.sessions.get_mut(&session_id)?.root_node_id = NEW_CONTAINER;
    Some(NEW_CONTAINER)
}

/// Adds the new leaf as the active tab of `tabs_node_id`. Where the tab goes
/// does not change its size.
fn add_active_tab(layout: &mut ClientState, tabs_node_id: NodeId) -> Option<()> {
    let tabs = layout.nodes.get_mut(&tabs_node_id)?.tabs.as_mut()?;
    tabs.tabs.push(TabRecord {
        title: String::new(),
        child_id: NEW_LEAF,
        synchronize_panes: false,
    });
    tabs.active = u32::try_from(tabs.tabs.len() - 1).ok()?;
    set_parent(layout, NEW_LEAF, tabs_node_id);
    Some(())
}

/// Opens the new leaf in a floating window on top of the others.
fn add_floating(
    layout: &mut ClientState,
    session_id: SessionId,
    geometry: FloatGeometry,
) -> Option<()> {
    layout.floating.insert(
        NEW_FLOATING,
        FloatingRecord {
            id: NEW_FLOATING,
            session_id,
            root_node_id: NEW_LEAF,
            title: None,
            geometry,
            focused: true,
            visible: true,
            close_on_empty: true,
        },
    );
    layout
        .sessions
        .get_mut(&session_id)?
        .floating_ids
        .push(NEW_FLOATING);
    Some(())
}

fn set_parent(layout: &mut ClientState, node_id: NodeId, parent_id: NodeId) {
    if let Some(node) = layout.nodes.get_mut(&node_id) {
        node.parent_id = Some(parent_id);
    }
}

/// Points whatever holds `old` (a split, a tab, the session root or a
/// floating window) at `new` instead.
fn replace_child(
    layout: &mut ClientState,
    session_id: SessionId,
    parent_id: Option<NodeId>,
    old: NodeId,
    new: NodeId,
) -> Option<()> {
    let Some(parent_id) = parent_id else {
        let session = layout.sessions.get_mut(&session_id)?;
        if session.root_node_id == old {
            session.root_node_id = new;
        } else {
            layout
                .floating
                .values_mut()
                .find(|window| window.root_node_id == old)?
                .root_node_id = new;
        }
        return Some(());
    };
    let parent = layout.nodes.get_mut(&parent_id)?;
    if let Some(split) = parent.split.as_mut() {
        *split.child_ids.iter_mut().find(|child| **child == old)? = new;
    } else {
        parent
            .tabs
            .as_mut()?
            .tabs
            .iter_mut()
            .find(|tab| tab.child_id == old)?
            .child_id = new;
    }
    Some(())
}
