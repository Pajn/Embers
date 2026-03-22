mod support;

use embers_client::{Context, PresentationModel};
use embers_core::Size;

use support::{ROOT_SPLIT_ID, SESSION_ID, demo_state};

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
