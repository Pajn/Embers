use embers_client::{
    HostCapabilities, PresentationModel, Renderer, SearchMatch, SearchState, SelectionKind,
    SelectionPoint, SelectionState,
};
use embers_core::{
    CellAttrs, CursorPosition, CursorShape, CursorState, Hyperlink, Size, SnapshotLine, StyledRun,
    TermColor,
};

use crate::support::{FOCUSED_BUFFER_ID, FOCUSED_LEAF_ID, ROOT_TABS_ID, SESSION_ID, demo_state};

/// Build a single-run styled line covering the whole text.
fn styled_line(text: &str, fg: TermColor, attrs: u16) -> SnapshotLine {
    SnapshotLine {
        text: text.to_owned(),
        runs: vec![StyledRun {
            len: text.len() as u32,
            fg,
            bg: TermColor::Default,
            attrs: CellAttrs(attrs),
            underline_color: TermColor::Default,
            link: 0,
        }],
        links: Vec::new(),
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
    Renderer::default().render(&state, &presentation)
}

#[test]
fn renders_hint_labels_over_matches() {
    use embers_client::{HintMatch, HintsState};

    let mut state = demo_state();
    let view = state.view_state_mut(FOCUSED_LEAF_ID).unwrap();
    view.follow_output = false;
    view.scroll_top_line = 0;
    view.visible_lines = vec![SnapshotLine::plain("see https://example.com/x now")];
    view.hints_state = Some(HintsState {
        matches: vec![HintMatch {
            label: "a".to_owned(),
            text: "https://example.com/x".to_owned(),
            line: 0,
            start_col: 4,
            end_col: 25,
        }],
        typed: String::new(),
        on_select: None,
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
    let grid = Renderer::default().render(&state, &presentation);
    let rendered = grid.render();
    // The label 'a' is drawn over the first cell of the match (the URL's 'h'),
    // leaving the rest of the matched text in place.
    assert!(
        rendered.contains("see attps://example.com/x"),
        "expected hint label at match start:\n{rendered}"
    );
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
    let renderer = Renderer::default();

    let grid = renderer.render(&state, &presentation);

    assert_eq!(
        grid.render(),
        concat!(
            "[demo]   shell  [!workspace]            \n",
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
fn pane_borders_take_theme_colors_next_to_the_focused_pane() {
    use embers_client::{PaneBorderColors, TerminalColor};

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
    let active = TerminalColor::Indexed(5);
    let inactive = TerminalColor::Indexed(8);
    let renderer = Renderer {
        pane_borders: PaneBorderColors {
            active: Some(active),
            inactive: Some(inactive),
        },
    };

    let grid = renderer.render(&state, &presentation);

    // The divider is column 13. The focused pane on its right starts at row 2,
    // under its nested tab bar: the divider is active alongside it only.
    assert_eq!(
        grid.render().lines().nth(2).unwrap().chars().nth(13),
        Some('|')
    );
    for row in 2..5 {
        let style = grid.cell_style(13, row).unwrap();
        assert_eq!(style.fg, Some(active), "row {row}");
        assert!(!style.dim, "row {row}");
    }
    assert_eq!(grid.cell_style(13, 1).unwrap().fg, Some(inactive));

    // Without theme colors every divider cell stays dim and uncolored.
    let plain = Renderer::default().render(&state, &presentation);
    for row in 1..5 {
        let style = plain.cell_style(13, row).unwrap();
        assert_eq!(style.fg, None, "row {row}");
        assert!(style.dim, "row {row}");
    }
}

#[test]
fn titles_from_programs_cannot_reach_the_host_as_escape_sequences() {
    use embers_client::{BarSegment, BarSpec, StyleSpec};
    use std::collections::BTreeMap;

    // Programs set pane titles with OSC 2, so a title can carry sequences of
    // its own; so can window titles and what tab bar formatters build.
    let evil = "evil\x1b]2;pwned\x07\x1b[2J";
    let mut state = demo_state();
    for buffer in state.buffers.values_mut() {
        buffer.title = evil.to_owned();
    }
    for tab in &mut state
        .nodes
        .get_mut(&ROOT_TABS_ID)
        .unwrap()
        .tabs
        .as_mut()
        .unwrap()
        .tabs
    {
        tab.title = evil.to_owned();
    }
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 80,
            height: 14,
        },
    )
    .expect("projection succeeds");
    let custom = BTreeMap::from([(
        ROOT_TABS_ID,
        BarSpec {
            left: vec![BarSegment {
                text: evil.to_owned(),
                style: StyleSpec::default(),
                target: None,
            }],
            center: Vec::new(),
            right: Vec::new(),
        },
    )]);

    for grid in [
        Renderer::default().render(&state, &presentation),
        Renderer::default().render_with_tab_bars(&state, &presentation, &custom),
    ] {
        let rendered = grid.render();
        assert!(rendered.contains("evil ]2;pwned  [2J"), "{rendered}");
        for line in grid.ansi_lines() {
            assert!(!line.contains('\x07'), "{line:?}");
            assert!(!line.contains("\x1b]"), "{line:?}");
            assert!(!line.contains("\x1b[2J"), "{line:?}");
        }
    }
}

#[test]
fn default_tab_bar_marks_synchronized_windows() {
    let mut state = demo_state();
    let root = state.nodes.get_mut(&ROOT_TABS_ID).unwrap();
    root.tabs.as_mut().unwrap().tabs[1].synchronize_panes = true;
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    let grid = Renderer::default().render(&state, &presentation);

    assert!(
        grid.render()
            .starts_with("[demo]   shell  [!workspace (sync)]     \n"),
        "{}",
        grid.render()
    );
}

#[test]
fn truncates_titles_in_narrow_viewports() {
    let state = demo_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 25,
            height: 8,
        },
    )
    .expect("projection succeeds");
    let renderer = Renderer::default();

    let grid = renderer.render(&state, &presentation);

    assert_eq!(grid.lines()[0], "[demo]   shell  [!works~]");
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
    let renderer = Renderer::default();
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
    let renderer = Renderer::default();
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
    let renderer = Renderer::default();
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
    let renderer = Renderer::default();
    let grid = renderer.render(&state, &presentation);

    assert!(grid.cursor().is_none());
    assert!(
        grid.ansi_lines()
            .iter()
            .any(|line| line.contains("\x1b[7mlo"))
    );
}

#[test]
fn renderer_draws_the_copy_mode_cursor_instead_of_the_program_cursor() {
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
    view.follow_output = false;
    view.scroll_top_line = 0;
    view.copy_cursor = Some(SelectionPoint { line: 1, column: 3 });

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .unwrap();
    let leaf = presentation.focused_leaf().unwrap().clone();
    let grid = Renderer::default().render(&state, &presentation);

    assert_eq!(
        grid.cursor(),
        Some(embers_client::GridCursor {
            x: leaf.rect.origin.x as u16 + 3,
            y: leaf.rect.origin.y as u16 + 2,
            shape: CursorShape::Block,
        })
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
    let grid = Renderer::default().render(&state, &presentation);
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
fn renders_underline_styles_with_colon_subparameters() {
    let styles = [
        CellAttrs::UNDERLINE,
        CellAttrs::DOUBLE_UNDERLINE,
        CellAttrs::CURLY_UNDERLINE,
        CellAttrs::DOTTED_UNDERLINE,
        CellAttrs::DASHED_UNDERLINE,
    ];
    let line = SnapshotLine {
        text: "abcde".to_owned(),
        runs: styles
            .iter()
            .map(|&attrs| StyledRun {
                len: 1,
                attrs: CellAttrs(attrs),
                ..StyledRun::default()
            })
            .collect(),
        links: Vec::new(),
    };
    let ansi = render_focused_with_lines(vec![line])
        .ansi_lines()
        .join("\n");
    // A single underline keeps the universally understood `4`; the other
    // styles use the `4:n` sub-parameter form.
    for expected in [
        "\x1b[4ma",
        "\x1b[4:2mb",
        "\x1b[4:3mc",
        "\x1b[4:4md",
        "\x1b[4:5me",
    ] {
        assert!(ansi.contains(expected), "missing {expected:?} in {ansi:?}");
    }
}

#[test]
fn renders_underline_color() {
    let mut rgb = styled_line("diag", TermColor::Default, CellAttrs::CURLY_UNDERLINE);
    rgb.runs[0].underline_color = TermColor::Rgb {
        r: 255,
        g: 16,
        b: 0,
    };
    let mut indexed = styled_line("warn", TermColor::Default, CellAttrs::UNDERLINE);
    indexed.runs[0].underline_color = TermColor::Indexed(3);
    let grid = render_focused_with_lines(vec![rgb, indexed]);
    let ansi = grid.ansi_lines().join("\n");
    assert!(
        ansi.contains("\x1b[58:2::255:16:0m"),
        "expected rgb underline color in {ansi:?}"
    );
    assert!(
        ansi.contains("\x1b[58:5:3m"),
        "expected indexed underline color in {ansi:?}"
    );
}

fn linked_line(text: &str, linked_len: u32, link: Hyperlink) -> SnapshotLine {
    SnapshotLine {
        text: text.to_owned(),
        runs: vec![
            StyledRun {
                len: linked_len,
                link: 1,
                ..StyledRun::default()
            },
            StyledRun {
                len: text.len() as u32 - linked_len,
                ..StyledRun::default()
            },
        ],
        links: vec![link],
    }
}

#[test]
fn renders_osc8_hyperlinks_and_closes_them() {
    let grid = render_focused_with_lines(vec![linked_line(
        "docs here",
        4,
        Hyperlink {
            id: Some("d1".to_owned()),
            uri: "https://example.com/docs".to_owned(),
        },
    )]);
    let ansi = grid.ansi_lines().join("\n");
    assert!(
        ansi.contains("\x1b]8;id=d1;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\ here"),
        "expected an OSC 8 span around the linked text in {ansi:?}"
    );
    // Every opened link is closed on its own row.
    for line in grid.ansi_lines() {
        assert_eq!(
            line.matches("\x1b]8;id=").count() + line.matches("\x1b]8;;http").count(),
            line.matches("\x1b]8;;\x1b\\").count(),
            "unbalanced OSC 8 in {line:?}"
        );
    }
}

#[test]
fn hosts_without_support_get_plain_underlines_and_no_links() {
    let mut curly = styled_line("diag", TermColor::Default, CellAttrs::CURLY_UNDERLINE);
    curly.runs[0].underline_color = TermColor::Indexed(3);
    let linked = linked_line(
        "docs here",
        4,
        Hyperlink {
            id: None,
            uri: "https://example.com/docs".to_owned(),
        },
    );
    let grid = render_focused_with_lines(vec![curly, linked]);

    let ansi = grid.ansi_lines_for(HostCapabilities::NONE).join("\n");
    assert!(
        ansi.contains("\x1b[4mdiag"),
        "expected a plain underline in {ansi:?}"
    );
    assert!(
        !ansi.contains("\x1b[4:"),
        "underline style leaked: {ansi:?}"
    );
    assert!(
        !ansi.contains("\x1b[58"),
        "underline color leaked: {ansi:?}"
    );
    assert!(!ansi.contains("\x1b]8;"), "hyperlink leaked: {ansi:?}");
    assert!(ansi.contains("docs here"));

    // Each capability is independent: styles without colors.
    let ansi = grid
        .ansi_lines_for(HostCapabilities {
            underline_styles: true,
            ..HostCapabilities::NONE
        })
        .join("\n");
    assert!(
        ansi.contains("\x1b[4:3mdiag"),
        "expected a curly underline in {ansi:?}"
    );
    assert!(
        !ansi.contains("\x1b[58"),
        "underline color leaked: {ansi:?}"
    );

    // Full support is what `ansi_lines` renders.
    assert_eq!(
        grid.ansi_lines_for(HostCapabilities::ALL),
        grid.ansi_lines()
    );
}

#[test]
fn osc8_emission_strips_control_characters() {
    // A URI cannot smuggle a terminator or a new escape sequence to the host.
    let grid = render_focused_with_lines(vec![linked_line(
        "x",
        1,
        Hyperlink {
            id: Some("a;b:c".to_owned()),
            uri: "https://e.com/\x07\x1b]0;pwned\x1b\\".to_owned(),
        },
    )]);
    let ansi = grid.ansi_lines().join("\n");
    assert!(
        ansi.contains("\x1b]8;id=abc;https://e.com/]0;pwned\\\x1b\\x"),
        "expected sanitized OSC 8 in {ansi:?}"
    );
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
    let grid = Renderer::default().render(&state, &presentation);
    assert!(
        grid.ansi_lines()
            .iter()
            .any(|line| line.contains("\x1b[38;2;10;20;30m"))
    );
}
