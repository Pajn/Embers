use embers_client::{NavigationDirection, PresentationModel, SplitResize, drawn_viewport};
use embers_core::{BufferId, FloatGeometry, NodeId, Point, PtySize, Size};

use crate::support::{
    FLOATING_ID, FLOATING_SPLIT_ID, FLOATING_TOP_LEAF_ID, FOCUSED_LEAF_ID, LEFT_LEAF_ID,
    ROOT_SPLIT_ID, SESSION_ID, demo_state,
};

// demo_state at 80x20: the root tab bar takes row 0, and the root split is
// [LEFT_LEAF_ID (weight 1), nested tabs holding FOCUSED_LEAF_ID (weight 2)]
// across 79 columns plus one divider, so the children are 26 and 53 wide and
// the divider sits in column 26.
const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

fn project(state: &embers_client::ClientState) -> PresentationModel {
    PresentationModel::project(state, SESSION_ID, VIEWPORT).expect("projection succeeds")
}

fn root_split(sizes: Vec<u16>) -> Option<SplitResize> {
    Some(SplitResize {
        split_node_id: ROOT_SPLIT_ID,
        sizes,
    })
}

#[test]
fn split_frames_report_drawn_lengths_and_minimums() {
    let state = demo_state();
    let presentation = project(&state);

    let split = presentation
        .splits
        .iter()
        .find(|split| split.node_id == ROOT_SPLIT_ID)
        .expect("root split is projected");
    assert_eq!(split.child_ids.len(), 2);
    assert_eq!(split.lengths, vec![26, 53]);
    assert_eq!(split.min_lengths, vec![1, 1]);

    let floating = presentation
        .splits
        .iter()
        .find(|split| split.node_id == FLOATING_SPLIT_ID)
        .expect("floating split is projected");
    // Two stacked panes each need a title row and one content row.
    assert_eq!(floating.min_lengths, vec![2, 2]);
}

#[test]
fn resizing_the_last_child_moves_the_divider_before_it() {
    let state = demo_state();
    let presentation = project(&state);

    // Like tmux, the last pane in a split moves its leading border: -L grows
    // it and -R shrinks it.
    assert_eq!(
        presentation.resize_leaf(&state, FOCUSED_LEAF_ID, NavigationDirection::Left, 3),
        root_split(vec![23, 56])
    );
    assert_eq!(
        presentation.resize_leaf(&state, FOCUSED_LEAF_ID, NavigationDirection::Right, 3),
        root_split(vec![29, 50])
    );
}

#[test]
fn resizing_a_leading_child_moves_its_trailing_divider() {
    let state = demo_state();
    let presentation = project(&state);

    assert_eq!(
        presentation.resize_leaf(&state, LEFT_LEAF_ID, NavigationDirection::Left, 3),
        root_split(vec![23, 56])
    );
    assert_eq!(
        presentation.resize_leaf(&state, LEFT_LEAF_ID, NavigationDirection::Right, 3),
        root_split(vec![29, 50])
    );
}

#[test]
fn resizing_clamps_at_the_minimum_and_is_a_no_op_at_the_edge() {
    let mut state = demo_state();
    let presentation = project(&state);

    let clamped = presentation.resize_leaf(&state, LEFT_LEAF_ID, NavigationDirection::Left, 500);
    assert_eq!(clamped, root_split(vec![1, 78]));

    let split = state.nodes.get_mut(&ROOT_SPLIT_ID).unwrap();
    split.split.as_mut().unwrap().sizes = vec![1, 78];
    let presentation = project(&state);
    assert_eq!(
        presentation.resize_leaf(&state, LEFT_LEAF_ID, NavigationDirection::Left, 3),
        None
    );
}

#[test]
fn resizing_without_a_split_on_that_axis_is_a_no_op() {
    let state = demo_state();
    let presentation = project(&state);

    assert_eq!(
        presentation.resize_leaf(&state, FOCUSED_LEAF_ID, NavigationDirection::Up, 3),
        None
    );
    assert_eq!(
        presentation.resize_leaf(&state, FOCUSED_LEAF_ID, NavigationDirection::Down, 3),
        None
    );
}

#[test]
fn resizing_inside_a_popup_stays_inside_the_popup() {
    let mut state = demo_state();
    state.floating.get_mut(&FLOATING_ID).unwrap().geometry = FloatGeometry::new(10, 2, 30, 14);
    let presentation = project(&state);
    let floating = presentation
        .splits
        .iter()
        .find(|split| split.node_id == FLOATING_SPLIT_ID)
        .expect("floating split is projected")
        .clone();
    let total: u16 = floating.lengths.iter().sum();

    let resize = presentation
        .resize_leaf(&state, FLOATING_TOP_LEAF_ID, NavigationDirection::Down, 1)
        .expect("popup split can grow its top pane");
    assert_eq!(resize.split_node_id, FLOATING_SPLIT_ID);
    assert_eq!(resize.sizes[0], floating.lengths[0] + 1);
    assert_eq!(resize.sizes.iter().sum::<u16>(), total);

    // A popup's root has no parent, so there is no outer split to resize.
    assert_eq!(
        presentation.resize_leaf(&state, FLOATING_TOP_LEAF_ID, NavigationDirection::Left, 1),
        None
    );
}

#[test]
fn zoomed_panes_have_no_visible_split_to_resize() {
    let mut state = demo_state();
    state.sessions.get_mut(&SESSION_ID).unwrap().zoomed_node_id = Some(FOCUSED_LEAF_ID);
    let presentation = project(&state);

    assert_eq!(
        presentation.resize_leaf(&state, FOCUSED_LEAF_ID, NavigationDirection::Left, 3),
        None
    );
}

#[test]
fn divider_hit_testing_finds_the_split_and_respects_popups() {
    let state = demo_state();
    let presentation = project(&state);

    let divider = presentation
        .divider_at(Point { x: 26, y: 2 })
        .expect("root divider is hit");
    assert_eq!(divider.split_node_id, ROOT_SPLIT_ID);
    assert_eq!(divider.index, 0);

    assert!(presentation.divider_at(Point { x: 25, y: 2 }).is_none());
    // The popup covers this part of the root divider.
    assert!(presentation.divider_at(Point { x: 26, y: 6 }).is_none());

    let popup_divider = presentation
        .dividers
        .iter()
        .find(|divider| divider.split_node_id == FLOATING_SPLIT_ID)
        .expect("popup divider exists")
        .clone();
    assert_eq!(
        presentation.divider_at(popup_divider.rect.origin),
        Some(&popup_divider)
    );
}

#[test]
fn dragging_a_divider_moves_it_to_the_pointer() {
    let state = demo_state();
    let presentation = project(&state);

    assert_eq!(
        presentation.drag_divider(ROOT_SPLIT_ID, 0, Point { x: 40, y: 2 }),
        root_split(vec![40, 39])
    );
    assert_eq!(
        presentation.drag_divider(ROOT_SPLIT_ID, 0, Point { x: 10, y: 15 }),
        root_split(vec![10, 69])
    );
    assert_eq!(
        presentation.drag_divider(ROOT_SPLIT_ID, 0, Point { x: -5, y: 2 }),
        root_split(vec![1, 78])
    );
    assert_eq!(
        presentation.drag_divider(ROOT_SPLIT_ID, 0, Point { x: 26, y: 2 }),
        None
    );
    assert_eq!(
        presentation.drag_divider(NodeId(999), 0, Point { x: 40, y: 2 }),
        None
    );
}

#[test]
fn drawn_viewport_is_reconstructed_from_pane_pty_sizes() {
    let mut state = demo_state();
    // The sizes a client drawing the session at 80x20 gives the visible panes.
    state.buffers.get_mut(&BufferId(2)).unwrap().pty_size = PtySize::new(26, 18);
    state.buffers.get_mut(&BufferId(4)).unwrap().pty_size = PtySize::new(53, 17);

    assert_eq!(drawn_viewport(&state, SESSION_ID), Some(VIEWPORT));
}
