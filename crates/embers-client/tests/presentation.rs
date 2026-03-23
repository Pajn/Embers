use embers_client::PresentationModel;
use embers_core::{FloatGeometry, Size, SplitDirection};

use crate::support::{
    FLOATING_BOTTOM_LEAF_ID, FLOATING_ID, FLOATING_TOP_LEAF_ID, FOCUSED_LEAF_ID, LEFT_LEAF_ID,
    NESTED_TABS_ID, ROOT_BUFFER_LEAF_ID, ROOT_ONLY_SPLIT_ID, ROOT_SPLIT_LEFT_LEAF_ID,
    ROOT_SPLIT_RIGHT_LEAF_ID, ROOT_TABS_ID, SESSION_ID, demo_state, root_buffer_state,
    root_split_state,
};

#[test]
fn projects_nested_tabs_in_split_and_tracks_focus_path() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let root_tabs = presentation
        .root_tabs
        .as_ref()
        .expect("root tabs are visible");
    assert_eq!(root_tabs.node_id, ROOT_TABS_ID);
    assert_eq!(root_tabs.tabs.len(), 2);
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").tabs_path,
        vec![ROOT_TABS_ID, NESTED_TABS_ID]
    );
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").node_id,
        FOCUSED_LEAF_ID
    );

    let left_leaf = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == LEFT_LEAF_ID)
        .expect("left leaf is visible");
    let right_leaf = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == FOCUSED_LEAF_ID)
        .expect("right leaf is visible");

    assert_eq!(left_leaf.rect.origin.x, 0);
    assert!(right_leaf.rect.origin.x > left_leaf.rect.origin.x);
}

#[test]
fn projects_split_in_floating_window() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let floating = presentation
        .floating
        .iter()
        .find(|window| window.floating_id == FLOATING_ID)
        .expect("floating window exists");
    assert_eq!(floating.rect.size.width, 20);
    assert_eq!(floating.rect.size.height, 7);

    let floating_leaves = presentation
        .leaves
        .iter()
        .filter(|leaf| leaf.floating_id == Some(FLOATING_ID))
        .collect::<Vec<_>>();
    assert_eq!(floating_leaves.len(), 2);
    assert!(
        floating_leaves
            .iter()
            .any(|leaf| leaf.node_id == FLOATING_TOP_LEAF_ID)
    );
    assert!(
        floating_leaves
            .iter()
            .any(|leaf| leaf.node_id == FLOATING_BOTTOM_LEAF_ID)
    );

    let floating_divider = presentation
        .dividers
        .iter()
        .find(|divider| divider.floating_id == Some(FLOATING_ID))
        .expect("floating split divider exists");
    assert_eq!(floating_divider.direction, SplitDirection::Horizontal);
}

#[test]
fn floating_windows_can_start_on_the_top_row() {
    let mut state = demo_state();
    state
        .floating
        .get_mut(&FLOATING_ID)
        .expect("floating window exists")
        .geometry = FloatGeometry::new(0, 0, 20, 7);

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let floating = presentation
        .floating
        .iter()
        .find(|window| window.floating_id == FLOATING_ID)
        .expect("floating window exists");
    assert_eq!(floating.rect.origin.y, 0);
    assert_eq!(floating.rect.size.height, 7);
}

#[test]
fn projects_root_buffer_without_tabs_frame() {
    let state = root_buffer_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert!(presentation.root_tabs.is_none());
    assert!(presentation.tab_bars.is_empty());
    assert_eq!(presentation.leaves.len(), 1);
    assert_eq!(presentation.leaves[0].node_id, ROOT_BUFFER_LEAF_ID);
    assert!(presentation.leaves[0].tabs_path.is_empty());
}

#[test]
fn projects_root_split_without_tabs_frame() {
    let state = root_split_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert!(presentation.root_tabs.is_none());
    assert!(presentation.tab_bars.is_empty());
    assert_eq!(presentation.leaves.len(), 2);
    assert_eq!(presentation.dividers.len(), 1);
    assert_eq!(presentation.dividers[0].direction, SplitDirection::Vertical);
    assert_eq!(presentation.session_id, SESSION_ID);
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").node_id,
        ROOT_SPLIT_RIGHT_LEAF_ID
    );
    assert!(
        presentation
            .leaves
            .iter()
            .any(|leaf| leaf.node_id == ROOT_SPLIT_LEFT_LEAF_ID)
    );
    assert_eq!(
        presentation.focus_target(embers_client::NavigationDirection::Left),
        Some(ROOT_SPLIT_LEFT_LEAF_ID)
    );
    assert_eq!(
        presentation.focused_leaf().expect("focused leaf").tabs_path,
        Vec::<embers_core::NodeId>::new()
    );
    assert_eq!(presentation.dividers[0].floating_id, None);
    assert_ne!(ROOT_ONLY_SPLIT_ID, ROOT_SPLIT_LEFT_LEAF_ID);
}
