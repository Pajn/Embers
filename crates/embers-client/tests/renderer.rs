use embers_client::{
    HostCapabilities, PaneTitles, PresentationModel, Renderer, SearchMatch, SearchState,
    SelectionKind, SelectionPoint, SelectionState,
};
use embers_core::{
    CellAttrs, CursorPosition, CursorShape, CursorState, Hyperlink, Size, SnapshotLine, StyledRun,
    TermColor,
};

use crate::support::{
    FLOATING_ID, FLOATING_TOP_LEAF_ID, FOCUSED_BUFFER_ID, FOCUSED_LEAF_ID, ROOT_BUFFER_LEAF_ID,
    ROOT_TABS_ID, SESSION_ID, demo_state, root_buffer_state, root_split_state,
};

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
    // On the title row, the scroll indicator stays clear of the line.
    let presentation = PresentationModel::project_with(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
        PaneTitles::Top,
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
            "left pane    | !build  [ logs-long-tit~]\n",
            "line two     |logs visible              \n",
            "line three   |second row                \n",
            "             |third row                 \n",
            "             |╭─popup────────────╮      \n",
            "             |│popup top         │      \n",
            "             |│                  │      \n",
            "             |│------------------│      \n",
            "             |│popup bottom      │      \n",
            "             |│                  │      \n",
            "             |╰──────────────────╯      \n",
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
        ..Renderer::default()
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

/// The demo popup's frame spans columns 14 to 33 and rows 5 to 11, with its
/// title "popup" from column 16 on the top row.
fn render_popup(focused: bool, renderer: Renderer) -> embers_client::RenderGrid {
    let mut state = demo_state();
    state.floating.get_mut(&FLOATING_ID).unwrap().focused = focused;
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");
    renderer.render(&state, &presentation)
}

fn char_at(grid: &embers_client::RenderGrid, x: usize, y: usize) -> char {
    grid.render()
        .lines()
        .nth(y)
        .unwrap()
        .chars()
        .nth(x)
        .unwrap()
}

#[test]
fn popup_frames_draw_the_configured_border_lines() {
    use embers_client::{PopupBorderLines, PopupBorders};

    // Corners, then a horizontal and a vertical cell.
    let cases = [
        (PopupBorderLines::Rounded, "╭╮╰╯─│"),
        (PopupBorderLines::Single, "┌┐└┘─│"),
        (PopupBorderLines::Double, "╔╗╚╝═║"),
        (PopupBorderLines::Heavy, "┏┓┗┛━┃"),
        (PopupBorderLines::Ascii, "++++-|"),
        (PopupBorderLines::None, "      "),
    ];
    for (lines, expected) in cases {
        for focused in [true, false] {
            let renderer = Renderer {
                popup_borders: PopupBorders {
                    lines,
                    ..PopupBorders::default()
                },
                ..Renderer::default()
            };
            let grid = render_popup(focused, renderer);
            let drawn: String = [(14, 5), (33, 5), (14, 11), (33, 11), (20, 11), (14, 8)]
                .into_iter()
                .map(|(x, y)| char_at(&grid, x, y))
                .collect();
            assert_eq!(drawn, expected, "{lines:?} focused={focused}");
            // The title stays on the top border and the content inside it.
            let rendered = grid.render();
            let top = rendered.lines().nth(5).unwrap();
            assert!(top.contains("popup"), "{lines:?}: {top}");
            assert_eq!(char_at(&grid, 15, 6), 'p', "{lines:?}");
        }
    }
}

#[test]
fn popup_frames_are_bold_when_focused_and_dim_when_not_without_reverse_video() {
    let focused = render_popup(true, Renderer::default());
    for (x, y) in [(14, 5), (20, 11), (14, 8), (16, 5)] {
        let style = focused.cell_style(x, y).unwrap();
        assert!(style.bold, "({x}, {y})");
        assert!(!style.reverse, "({x}, {y})");
        assert!(!style.dim, "({x}, {y})");
        assert_eq!(style.fg, None, "({x}, {y})");
    }

    let unfocused = render_popup(false, Renderer::default());
    for (x, y) in [(14, 5), (20, 11), (14, 8), (16, 5)] {
        let style = unfocused.cell_style(x, y).unwrap();
        assert!(style.dim, "({x}, {y})");
        assert!(!style.bold, "({x}, {y})");
        assert!(!style.reverse, "({x}, {y})");
        assert_eq!(style.fg, None, "({x}, {y})");
    }
}

#[test]
fn popup_frames_take_theme_colors() {
    use embers_client::{PopupBorders, TerminalColor};

    let active = TerminalColor::Indexed(5);
    let inactive = TerminalColor::Indexed(8);
    let renderer = Renderer {
        popup_borders: PopupBorders {
            active: Some(active),
            inactive: Some(inactive),
            ..PopupBorders::default()
        },
        ..Renderer::default()
    };

    let focused = render_popup(true, renderer);
    for (x, y) in [(14, 5), (20, 11), (14, 8)] {
        let style = focused.cell_style(x, y).unwrap();
        assert_eq!(style.fg, Some(active), "({x}, {y})");
        assert!(!style.reverse && !style.dim, "({x}, {y})");
    }
    // The title takes the frame's color and stays bold.
    let title = focused.cell_style(16, 5).unwrap();
    assert_eq!(title.fg, Some(active));
    assert!(title.bold);

    let unfocused = render_popup(false, renderer);
    for (x, y) in [(14, 5), (20, 11), (14, 8), (16, 5)] {
        let style = unfocused.cell_style(x, y).unwrap();
        assert_eq!(style.fg, Some(inactive), "({x}, {y})");
        assert!(!style.reverse && !style.dim && !style.bold, "({x}, {y})");
    }
}

#[test]
fn borderless_popups_draw_no_frame_and_their_content_fills_them() {
    use embers_core::PtySize;

    let mut state = demo_state();
    state.floating.get_mut(&FLOATING_ID).unwrap().border = false;
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    // The content gets the whole popup, columns 14 to 33 and rows 5 to 11.
    let floating = &presentation.floating[0];
    assert!(!floating.border);
    assert_eq!(floating.content_rect, floating.rect);
    let top = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == FLOATING_TOP_LEAF_ID)
        .unwrap();
    assert_eq!((top.rect.origin.x, top.rect.origin.y), (14, 5));
    assert_eq!(top.rect.size.width, 20);
    // Without title rows, the default, its PTY is the whole pane.
    assert_eq!(top.title_rect, None);
    assert_eq!(top.pty_size(), Some(PtySize::new(20, top.rect.size.height)));

    let grid = Renderer::default().render(&state, &presentation);
    let rendered = grid.render();
    let lines: Vec<&str> = rendered.lines().collect();
    // No frame, popup title or pane title: the content starts in the
    // popup's corner.
    let top_row: String = lines[5].chars().skip(14).take(20).collect();
    assert_eq!(top_row, "popup top           ");
    assert!(!rendered.contains("popup─") && !rendered.contains('╭'));
    assert!(!rendered.contains("popup-top"));
    assert_eq!(char_at(&grid, 14, 11), ' ');

    // With title rows, the pane's own title row is its only title.
    let presentation = PresentationModel::project_with(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
        PaneTitles::Top,
    )
    .expect("projection succeeds");
    let top = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == FLOATING_TOP_LEAF_ID)
        .unwrap();
    assert_eq!(
        top.pty_size(),
        Some(PtySize::new(20, top.rect.size.height - 1))
    );
    let grid = Renderer::default().render(&state, &presentation);
    let rendered = grid.render();
    let lines: Vec<&str> = rendered.lines().collect();
    let top_row: String = lines[5].chars().skip(14).take(20).collect();
    assert_eq!(top_row, "   popup-top        ");
    assert_eq!(rendered.matches("popup-top").count(), 1);
    assert_eq!(char_at(&grid, 14, 6), 'p');
}

#[test]
fn a_framed_popup_shows_its_title_once_on_its_frame() {
    let mut state = demo_state();
    // A popup titled like its pane, as display-popup creates them (this one
    // holds a split, whose other pane is titled `popup-bottom`).
    let floating = state.floating.get_mut(&FLOATING_ID).unwrap();
    floating.title = Some("picker".to_owned());
    state
        .buffers
        .get_mut(&embers_core::BufferId(5))
        .unwrap()
        .title = "picker".to_owned();
    for pane_titles in [PaneTitles::Off, PaneTitles::Top, PaneTitles::Bottom] {
        let presentation = PresentationModel::project_with(
            &state,
            SESSION_ID,
            Size {
                width: 40,
                height: 14,
            },
            pane_titles,
        )
        .expect("projection succeeds");
        for leaf in presentation
            .leaves
            .iter()
            .filter(|leaf| leaf.floating_id.is_some())
        {
            assert_eq!(leaf.title_rect, None, "{pane_titles:?}");
            assert_eq!(leaf.content_rect, leaf.rect, "{pane_titles:?}");
        }
        let rendered = Renderer::default().render(&state, &presentation).render();
        assert_eq!(
            rendered.matches("picker").count(),
            1,
            "{pane_titles:?}\n{rendered}"
        );
        assert!(!rendered.contains("popup-bottom"), "{rendered}");
        // On the frame's top edge.
        assert!(
            rendered.lines().nth(5).unwrap().contains("─picker─"),
            "{rendered}"
        );
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
            x: focused_leaf.content_rect.origin.x as u16 + 3,
            // +1 for the cursor's row within the pane's content.
            y: focused_leaf.content_rect.origin.y as u16 + 1,
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
            x: leaf.content_rect.origin.x as u16 + 3,
            y: leaf.content_rect.origin.y as u16 + 1,
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

fn render_with_pane_titles(
    state: &embers_client::ClientState,
    viewport: Size,
    pane_titles: PaneTitles,
) -> embers_client::RenderGrid {
    let presentation =
        PresentationModel::project_with(state, SESSION_ID, viewport, pane_titles).unwrap();
    Renderer::default().render(state, &presentation)
}

#[test]
fn panes_without_title_rows_show_content_from_their_first_row() {
    let grid = render_with_pane_titles(
        &root_split_state(),
        Size {
            width: 31,
            height: 3,
        },
        PaneTitles::Off,
    );

    // No title row; the divider still separates the panes.
    assert_eq!(
        grid.render(),
        concat!(
            "left root pane |right root pane\n",
            "               |               \n",
            "               |               "
        )
    );
}

#[test]
fn pane_title_rows_are_drawn_above_or_below_the_content() {
    let state = root_split_state();
    let viewport = Size {
        width: 31,
        height: 3,
    };

    let top = render_with_pane_titles(&state, viewport, PaneTitles::Top);
    assert_eq!(
        top.render(),
        concat!(
            "   root-left   |>+ root-right  \n",
            "left root pane |right root pane\n",
            "               |               "
        )
    );

    let bottom = render_with_pane_titles(&state, viewport, PaneTitles::Bottom);
    assert_eq!(
        bottom.render(),
        concat!(
            "left root pane |right root pane\n",
            "               |               \n",
            "   root-left   |>+ root-right  "
        )
    );
}

#[test]
fn overlays_and_cursor_follow_the_content_without_title_rows() {
    let mut state = root_buffer_state();
    let view = state.view_state_mut(ROOT_BUFFER_LEAF_ID).unwrap();
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
    view.copy_cursor = Some(SelectionPoint {
        line: 13,
        column: 2,
    });
    let viewport = Size {
        width: 20,
        height: 4,
    };

    let grid = render_with_pane_titles(&state, viewport, PaneTitles::Off);
    let lines = grid.lines();
    // The scroll indicator sits in the content's top-right corner, over it.
    assert_eq!(lines[0], "needle here    13/60");
    assert_eq!(lines[1].trim_end(), "plain");
    assert!(grid.cell_style(0, 0).unwrap().reverse);
    let cursor = grid.cursor().expect("copy-mode cursor");
    assert_eq!((cursor.x, cursor.y), (2, 1));

    let titled = render_with_pane_titles(&state, viewport, PaneTitles::Top);
    let lines = titled.lines();
    assert!(lines[0].ends_with("13/60"), "{lines:#?}");
    assert_eq!(lines[1].trim_end(), "needle here");
    assert!(titled.cell_style(0, 1).unwrap().reverse);
    let cursor = titled.cursor().expect("copy-mode cursor");
    assert_eq!((cursor.x, cursor.y), (2, 2));
}
