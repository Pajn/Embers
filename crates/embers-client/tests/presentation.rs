mod support;

use embers_client::PresentationModel;
use embers_core::{Size, SplitDirection};

use support::{
    FLOATING_BOTTOM_LEAF_ID, FLOATING_ID, FLOATING_TOP_LEAF_ID, FOCUSED_LEAF_ID, LEFT_LEAF_ID,
    NESTED_TABS_ID, ROOT_TABS_ID, SESSION_ID, demo_state,
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

    assert_eq!(presentation.root_tabs.node_id, ROOT_TABS_ID);
    assert_eq!(presentation.root_tabs.tabs.len(), 2);
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
