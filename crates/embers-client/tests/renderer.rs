use embers_client::{
    PresentationModel, Renderer, SearchMatch, SearchState, SelectionKind, SelectionPoint,
    SelectionState,
};
use embers_core::{
    CellAttrs, CursorPosition, CursorShape, CursorState, Size, SnapshotLine, StyledRun, TermColor,
};

use crate::support::{FOCUSED_BUFFER_ID, FOCUSED_LEAF_ID, SESSION_ID, demo_state};

/// Build a single-run styled line covering the whole text.
fn styled_line(text: &str, fg: TermColor, attrs: u16) -> SnapshotLine {
    SnapshotLine {
        text: text.to_owned(),
        runs: vec![StyledRun {
            len: text.len() as u32,
            fg,
            bg: TermColor::Default,
            attrs: CellAttrs(attrs),
        }],
    }
}

/// Render `demo_state` after installing styled visible lines on the focused leaf.
fn render_focused_with_lines(lines: Vec<SnapshotLine>) -> embers_client::RenderGrid {
    let mut state = demo_state();
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.follow_output = false;
    view.scroll_top_line = 0;
    view.visible_lines = lines;
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");
    Renderer.render(&state, &presentation)
}

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
        .get_mut(&FOCUSED_BUFFER_ID)
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
            // +1 for the tab/header row above the leaf and +1 for the cursor's row within it.
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
    view.visible_lines = vec![
        SnapshotLine::plain("needle here"),
        SnapshotLine::plain("plain"),
    ];
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
fn renderer_uses_snapshot_lines_for_overlays_when_visible_lines_are_empty() {
    let mut state = demo_state();
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.follow_output = false;
    view.scroll_top_line = 0;
    view.visible_lines.clear();
    view.search_state = Some(SearchState {
        query: "left".to_owned(),
        matches: vec![SearchMatch {
            line: 0,
            start_column: 0,
            end_column: 4,
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

    assert!(
        ansi.iter().any(|line| line.contains("\x1b[4m\x1b[7m")),
        "{ansi:?}"
    );
}

#[test]
fn renderer_draws_selection_overlay_and_hides_program_cursor_when_selecting() {
    let mut state = demo_state();
    state
        .snapshots
        .get_mut(&FOCUSED_BUFFER_ID)
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

#[test]
fn renders_indexed_foreground_run_in_pane() {
    let grid = render_focused_with_lines(vec![styled_line("red", TermColor::Indexed(1), 0)]);
    let ansi = grid.ansi_lines();
    assert!(
        ansi.iter().any(|line| line.contains("\x1b[38;5;1m")),
        "{ansi:?}"
    );
    // Plain projection still shows the text.
    assert!(grid.lines().iter().any(|line| line.contains("red")));
}

#[test]
fn selection_composes_with_content_color() {
    let mut state = demo_state();
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.follow_output = false;
    view.scroll_top_line = 0;
    view.visible_lines = vec![styled_line("red", TermColor::Indexed(1), 0)];
    view.selection_state = Some(SelectionState {
        kind: SelectionKind::Character,
        anchor: SelectionPoint { line: 0, column: 0 },
        cursor: SelectionPoint { line: 0, column: 2 },
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
    let grid = Renderer.render(&state, &presentation);
    let ansi = grid.ansi_lines();

    // The selected span keeps its red foreground and gains reverse — the two
    // compose rather than the selection erasing the color.
    let styled = ansi
        .iter()
        .find(|line| line.contains("\x1b[38;5;1m"))
        .expect("red foreground present");
    assert!(
        styled.contains("\x1b[7m"),
        "expected reverse too: {styled:?}"
    );
}

#[test]
fn hidden_run_blanks_the_glyph() {
    let grid = render_focused_with_lines(vec![styled_line(
        "secret",
        TermColor::Default,
        CellAttrs::HIDDEN,
    )]);
    // The hidden text must not appear as glyphs in the pane.
    assert!(
        grid.lines().iter().all(|line| !line.contains("secret")),
        "{:?}",
        grid.lines()
    );
}

#[test]
fn renders_styled_wide_char() {
    let grid = render_focused_with_lines(vec![styled_line("界x", TermColor::Indexed(2), 0)]);
    let ansi = grid.ansi_lines();
    assert!(
        ansi.iter().any(|line| line.contains("\x1b[38;5;2m")),
        "{ansi:?}"
    );
    assert!(grid.lines().iter().any(|line| line.contains("界")));
}

#[test]
fn scrolled_styled_view_keeps_color() {
    let mut state = demo_state();
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.follow_output = false;
    view.scroll_top_line = 5;
    view.total_line_count = 60;
    view.visible_lines = vec![styled_line(
        "scrolled",
        TermColor::Rgb {
            r: 10,
            g: 20,
            b: 30,
        },
        0,
    )];
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .unwrap();
    let grid = Renderer.render(&state, &presentation);
    assert!(
        grid.ansi_lines()
            .iter()
            .any(|line| line.contains("\x1b[38;2;10;20;30m"))
    );
}
