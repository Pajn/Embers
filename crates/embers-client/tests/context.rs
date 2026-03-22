mod support;

use embers_client::{Context, PresentationModel, TabBarContext};
use embers_core::Size;

use support::{NESTED_TABS_ID, ROOT_SPLIT_ID, SESSION_ID, demo_state};

const TEST_SIZE: Size = Size {
    width: 40,
    height: 14,
};

#[test]
fn visible_nodes_include_layout_ancestors_without_direct_geometry() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");
    let context = Context::from_state(&state, Some(&presentation));

    assert!(
        context
            .find_node(ROOT_SPLIT_ID)
            .expect("split exists")
            .visible
    );
}

#[test]
fn tab_bar_context_reports_recursive_buffer_counts() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let root_tabs = presentation
        .root_tabs
        .as_ref()
        .expect("root tabs are visible");
    let root_context = TabBarContext::from_frame(root_tabs, "normal", TEST_SIZE.width);
    assert_eq!(
        root_context
            .tabs
            .iter()
            .map(|tab| tab.buffer_count)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );

    let nested_tabs = presentation
        .tab_bars
        .iter()
        .find(|tabs| tabs.node_id == NESTED_TABS_ID)
        .expect("nested tabs are visible");
    let nested_context = TabBarContext::from_frame(nested_tabs, "normal", TEST_SIZE.width);
    assert_eq!(
        nested_context
            .tabs
            .iter()
            .map(|tab| tab.buffer_count)
            .collect::<Vec<_>>(),
        vec![1, 1]
    );
}
