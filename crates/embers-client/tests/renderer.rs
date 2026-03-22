mod support;

use embers_client::{
    PresentationModel, Renderer, SearchMatch, SearchState, SelectionKind, SelectionPoint,
    SelectionState,
};
use embers_core::{BufferId, CursorPosition, CursorShape, CursorState, Size};

use support::{FOCUSED_LEAF_ID, SESSION_ID, demo_state};

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

#[test]
fn renderer_emits_styles_and_tracks_cursor_position() {
    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&BufferId(4))
        .expect("focused pane snapshot")
        .cursor = Some(CursorState {
        position: CursorPosition { row: 1, col: 3 },
        shape: CursorShape::Beam,
    });
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
    let focused_leaf = presentation.focused_leaf().expect("focused leaf");

    let grid = renderer.render(&state, &presentation);

    assert_eq!(
        grid.cursor(),
        Some(embers_client::GridCursor {
            x: focused_leaf.rect.origin.x as u16 + 3,
            y: focused_leaf.rect.origin.y as u16 + 1 + 1,
            shape: CursorShape::Beam,
        })
    );
    assert!(grid.ansi_lines()[0].contains("\x1b[7m"));
}

#[test]
fn renderer_shows_scroll_indicator_and_search_highlights() {
    let mut state = demo_state();
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.follow_output = false;
    view.scroll_top_line = 12;
    view.total_line_count = 60;
    view.visible_lines = vec!["needle here".to_owned(), "plain".to_owned()];
    view.search_state = Some(SearchState {
        query: "needle".to_owned(),
        matches: vec![SearchMatch {
            line: 12,
            start_column: 0,
            end_column: 6,
        }],
        active_match_index: Some(0),
    });

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .unwrap();
    let renderer = Renderer;
    let grid = renderer.render(&state, &presentation);
    let ansi = grid.ansi_lines();

    assert!(ansi.iter().any(|line| line.contains("13/60")));
    assert!(ansi.iter().any(|line| line.contains("\x1b[4m\x1b[7m")));
}

#[test]
fn renderer_draws_selection_overlay_and_hides_program_cursor_when_selecting() {
    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&BufferId(4))
        .expect("focused pane snapshot")
        .cursor = Some(CursorState {
        position: CursorPosition { row: 0, col: 0 },
        shape: CursorShape::Beam,
    });
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.selection_state = Some(SelectionState {
        kind: SelectionKind::Character,
        anchor: SelectionPoint { line: 0, column: 0 },
        cursor: SelectionPoint { line: 0, column: 1 },
    });

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .unwrap();
    let renderer = Renderer;
    let grid = renderer.render(&state, &presentation);

    assert!(grid.cursor().is_none());
    assert!(
        grid.ansi_lines()
            .iter()
            .any(|line| line.contains("\x1b[7mlo"))
    );
}
