mod support;

use embers_client::{PresentationModel, Renderer};
use embers_core::Size;

use support::{SESSION_ID, demo_state};

#[test]
fn renders_nested_tabs_splits_and_floating_overlay() {
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
    let renderer = Renderer;

    let grid = renderer.render(&state, &presentation);

    assert_eq!(
        grid.render(),
        concat!(
            "  shell  [!workspace]                   \n",
            " + editor    | !build  [ logs-long-tit~]\n",
            "left pane    |>  logs-long-title        \n",
            "line two     |logs visible              \n",
            "line three   |second row                \n",
            "             |+-popup------------+      \n",
            "             ||   popup-top      |      \n",
            "             ||popup top         |      \n",
            "             ||------------------|      \n",
            "             ||   popup-bottom   |      \n",
            "             ||popup bottom      |      \n",
            "             |+------------------+      \n",
            "             |                          \n",
            "             |                          "
        )
    );
}

#[test]
fn truncates_titles_in_narrow_viewports() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 18,
            height: 8,
        },
    )
    .expect("projection succeeds");
    let renderer = Renderer;

    let grid = renderer.render(&state, &presentation);

    assert_eq!(grid.lines()[0], "  shell  [!works~]");
    assert!(grid.lines()[1].contains("!build"));
}
