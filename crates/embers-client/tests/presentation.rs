use embers_client::{
    LayoutOptions, PaneTitles, PresentationModel, RootTabBar, StatusLineMode, TabBarPosition,
};
use embers_core::{
    ActivityState, BufferId, FloatGeometry, NodeId, Point, PtySize, SessionId, Size, SplitDirection,
};
use embers_protocol::{
    BufferRecord, BufferRecordKind, BufferRecordState, BufferViewRecord, NodeRecord, NodeRecordKind,
};

use crate::support::{
    FLOATING_BOTTOM_LEAF_ID, FLOATING_ID, FLOATING_SPLIT_ID, FLOATING_TOP_LEAF_ID, FOCUSED_LEAF_ID,
    LEFT_LEAF_ID, NESTED_TABS_ID, ROOT_BUFFER_LEAF_ID, ROOT_ONLY_SPLIT_ID, ROOT_SPLIT_LEFT_LEAF_ID,
    ROOT_SPLIT_RIGHT_LEAF_ID, ROOT_TABS_ID, SESSION_ID, demo_state, floating_focused_state,
    root_buffer_state, root_split_state,
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
fn root_tab_bar_can_sit_at_the_bottom() {
    let state = demo_state();
    let viewport = Size {
        width: 40,
        height: 14,
    };
    let top = PresentationModel::project(&state, SESSION_ID, viewport).unwrap();
    let bottom =
        PresentationModel::project_with(&state, SESSION_ID, viewport, TabBarPosition::Bottom)
            .unwrap();

    assert_eq!(top.root_tabs.as_ref().unwrap().rect.origin.y, 0);
    assert_eq!(bottom.root_tabs.as_ref().unwrap().rect.origin.y, 13);
    // Nested tab bars stay at the top of their own area.
    let nested = |model: &PresentationModel| {
        model
            .tab_bars
            .iter()
            .find(|bar| bar.node_id == NESTED_TABS_ID)
            .unwrap()
            .rect
    };
    assert_eq!(nested(&top).origin.y, 1);
    assert_eq!(nested(&bottom).origin.y, 0);

    // The window's panes and dividers move up a row and keep their sizes, so
    // PTY sizes do not depend on where the bar is. Popups keep their place.
    assert_eq!(top.leaves.len(), bottom.leaves.len());
    for (top_leaf, bottom_leaf) in top.leaves.iter().zip(&bottom.leaves) {
        assert_eq!(top_leaf.node_id, bottom_leaf.node_id);
        let shift = if top_leaf.floating_id.is_some() { 0 } else { 1 };
        assert_eq!(bottom_leaf.rect.origin.y, top_leaf.rect.origin.y - shift);
        assert_eq!(bottom_leaf.rect.size, top_leaf.rect.size);
        assert_eq!(bottom_leaf.pty_size(), top_leaf.pty_size());
    }
    for (top_divider, bottom_divider) in top.dividers.iter().zip(&bottom.dividers) {
        let shift = if top_divider.floating_id.is_some() {
            0
        } else {
            1
        };
        assert_eq!(
            bottom_divider.rect.origin.y,
            top_divider.rect.origin.y - shift
        );
    }
    assert_eq!(top.floating, bottom.floating);

    // Status content is drawn over the root tab bar, wherever it is.
    assert_eq!(top.status_row(), Some(0));
    assert_eq!(bottom.status_row(), Some(13));
}

#[test]
fn status_content_takes_the_last_row_without_a_root_tab_bar() {
    let state = root_buffer_state();
    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .unwrap();
    assert!(presentation.root_tabs.is_none());
    assert_eq!(presentation.status_row(), Some(13));
}

#[test]
fn screen_layouts_give_the_window_every_row_the_bars_do_not_take() {
    let state = demo_state();
    let terminal = Size {
        width: 40,
        height: 14,
    };
    let pty_sizes = |model: &PresentationModel| {
        model
            .leaves
            .iter()
            .map(|leaf| (leaf.node_id, leaf.pty_size()))
            .collect::<Vec<_>>()
    };
    for status_line in [StatusLineMode::Overlay, StatusLineMode::Separate] {
        for position in [TabBarPosition::Top, TabBarPosition::Bottom] {
            for visible in [true, false] {
                let layout = format!("{status_line:?} {position:?} visible={visible}");
                let viewport = status_line.content_viewport(terminal);
                let separate_rows = u16::from(status_line == StatusLineMode::Separate);
                assert_eq!(viewport.height, terminal.height - separate_rows, "{layout}");

                let model = PresentationModel::project_with(
                    &state,
                    SESSION_ID,
                    viewport,
                    RootTabBar { position, visible },
                )
                .unwrap();
                let window_top = i32::from(visible && position == TabBarPosition::Top);
                let window_rows = viewport.height - u16::from(visible);
                let tiled = model
                    .leaves
                    .iter()
                    .filter(|leaf| leaf.floating_id.is_none())
                    .collect::<Vec<_>>();
                let top = tiled.iter().map(|leaf| leaf.rect.origin.y).min().unwrap();
                let bottom = tiled
                    .iter()
                    .map(|leaf| leaf.rect.origin.y + i32::from(leaf.rect.size.height))
                    .max()
                    .unwrap();
                assert_eq!(top, window_top, "{layout}");
                assert_eq!(bottom, window_top + i32::from(window_rows), "{layout}");

                // A hidden bar draws nothing and takes no clicks.
                let root_bar = model.root_tabs.as_ref().unwrap();
                assert_eq!(root_bar.rect.size.height, u16::from(visible), "{layout}");
                // Status content goes over the bar, or the last row without it.
                let status_row = match (visible, position) {
                    (true, TabBarPosition::Top) => 0,
                    _ => viewport.height - 1,
                };
                assert_eq!(model.status_row(), Some(status_row), "{layout}");

                // The reported viewport counts a hidden bar's row, so the
                // CLI's projection, with the bar shown, gives the same sizes.
                let layout_viewport = model.layout_viewport();
                assert_eq!(
                    layout_viewport.height,
                    viewport.height + u16::from(!visible),
                    "{layout}"
                );
                let cli = PresentationModel::project(&state, SESSION_ID, layout_viewport).unwrap();
                assert_eq!(pty_sizes(&model), pty_sizes(&cli), "{layout}");
            }
        }
    }
}

#[test]
fn hiding_the_root_tab_bar_keeps_nested_tab_bars() {
    let state = demo_state();
    let viewport = Size {
        width: 40,
        height: 14,
    };
    let hidden = PresentationModel::project_with(
        &state,
        SESSION_ID,
        viewport,
        RootTabBar {
            position: TabBarPosition::Top,
            visible: false,
        },
    )
    .unwrap();
    let nested = hidden
        .tab_bars
        .iter()
        .find(|bar| bar.node_id == NESTED_TABS_ID)
        .unwrap();
    assert_eq!(nested.rect.origin.y, 0);
    assert_eq!(nested.rect.size.height, 1);
}

#[test]
fn tabs_report_the_pane_they_show() {
    let mut state = demo_state();
    let shell = state.buffers.get_mut(&BufferId(1)).unwrap();
    shell.title = "shell title".to_owned();
    shell.command = vec!["-zsh".to_owned()];
    shell.cwd = Some("/srv/app".to_owned());
    let editor = state.buffers.get_mut(&BufferId(4)).unwrap();
    editor.foreground_command = Some(vec!["nvim".to_owned(), "notes.txt".to_owned()]);
    let project = |state: &embers_client::ClientState| {
        PresentationModel::project(
            state,
            SESSION_ID,
            Size {
                width: 80,
                height: 24,
            },
        )
        .expect("projection succeeds")
    };
    let pane = |presentation: &PresentationModel, tabs: NodeId, index: usize| {
        let frame = presentation
            .tab_bars
            .iter()
            .find(|frame| frame.node_id == tabs)
            .expect("tabs are projected");
        frame.tabs[index].pane.clone().expect("tab shows a pane")
    };

    let presentation = project(&state);
    // A hidden window shows its only pane.
    let hidden = pane(&presentation, ROOT_TABS_ID, 0);
    assert_eq!(hidden.title, "shell title");
    assert_eq!(hidden.process_name.as_deref(), Some("zsh"));
    assert_eq!(hidden.cwd.as_deref(), Some("/srv/app"));
    // The window holding the focus shows the focused pane, also through its
    // nested tabs.
    let focused = pane(&presentation, ROOT_TABS_ID, 1);
    assert_eq!(focused.process_name.as_deref(), Some("nvim"));
    assert_eq!(focused.title, "logs-long-title");
    assert_eq!(
        pane(&presentation, NESTED_TABS_ID, 0).title,
        "build",
        "a hidden nested tab shows its own pane"
    );

    state.sessions.get_mut(&SESSION_ID).unwrap().focused_leaf_id = Some(LEFT_LEAF_ID);
    let presentation = project(&state);
    assert_eq!(pane(&presentation, ROOT_TABS_ID, 1).title, "editor");
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
fn zoomed_floating_windows_keep_floating_context() {
    let mut state = floating_focused_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session exists")
        .zoomed_node_id = Some(FLOATING_SPLIT_ID);

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
        .expect("zoomed floating frame exists");
    let floating_leaves = presentation
        .leaves
        .iter()
        .filter(|leaf| leaf.floating_id == Some(FLOATING_ID))
        .collect::<Vec<_>>();
    let floating_dividers = presentation
        .dividers
        .iter()
        .filter(|divider| divider.floating_id == Some(FLOATING_ID))
        .collect::<Vec<_>>();
    let floating_leaf_ids = floating_leaves
        .iter()
        .map(|leaf| leaf.node_id)
        .collect::<Vec<_>>();
    assert_eq!(floating.rect.size.width, 20);
    assert_eq!(floating.rect.size.height, 7);
    assert_eq!(floating_leaf_ids.len(), 2);
    assert!(floating_leaf_ids.contains(&FLOATING_TOP_LEAF_ID));
    assert!(floating_leaf_ids.contains(&FLOATING_BOTTOM_LEAF_ID));
    assert_eq!(presentation.leaves.len(), 2);
    assert_eq!(floating_dividers.len(), 1);
    assert_eq!(presentation.dividers.len(), 1);
    assert_eq!(floating_dividers[0].direction, SplitDirection::Horizontal);
}

#[test]
fn zoomed_floating_descendants_keep_floating_context() {
    let mut state = floating_focused_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session exists")
        .zoomed_node_id = Some(FLOATING_TOP_LEAF_ID);

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert_eq!(presentation.focused_floating_id(), Some(FLOATING_ID));
    assert_eq!(presentation.floating.len(), 1);
    assert_eq!(presentation.floating[0].floating_id, FLOATING_ID);
    assert_eq!(presentation.leaves.len(), 1);
    assert_eq!(presentation.leaves[0].node_id, FLOATING_TOP_LEAF_ID);
    assert_eq!(presentation.leaves[0].floating_id, Some(FLOATING_ID));
}

#[test]
fn stale_zoomed_floating_nodes_outside_session_are_ignored() {
    let mut state = demo_state();
    let session = state.sessions.get_mut(&SESSION_ID).expect("session exists");
    session.zoomed_node_id = Some(FLOATING_TOP_LEAF_ID);
    session.floating_ids.clear();

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert_eq!(
        presentation.root_tabs.as_ref().map(|tabs| tabs.node_id),
        Some(ROOT_TABS_ID)
    );
    assert_eq!(
        presentation.focused_leaf().map(|leaf| leaf.node_id),
        Some(FOCUSED_LEAF_ID)
    );
    assert!(presentation.floating.is_empty());
    assert!(
        presentation
            .leaves
            .iter()
            .all(|leaf| leaf.floating_id.is_none())
    );
}

#[test]
fn foreign_session_zoom_targets_are_ignored() {
    const FOREIGN_SESSION_ID: SessionId = SessionId(99);
    const FOREIGN_NODE_ID: NodeId = NodeId(999);
    const FOREIGN_BUFFER_ID: BufferId = BufferId(999);

    let mut state = demo_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session exists")
        .zoomed_node_id = Some(FOREIGN_NODE_ID);
    state.nodes.insert(
        FOREIGN_NODE_ID,
        NodeRecord {
            id: FOREIGN_NODE_ID,
            session_id: FOREIGN_SESSION_ID,
            // Intentional: point the foreign node at the local root tabs so session checks win.
            parent_id: Some(ROOT_TABS_ID),
            kind: NodeRecordKind::BufferView,
            buffer_view: Some(BufferViewRecord {
                buffer_id: FOREIGN_BUFFER_ID,
                focused: false,
                zoomed: false,
                follow_output: true,
                last_render_size: PtySize::new(80, 24),
            }),
            split: None,
            tabs: None,
            focus_leaf_id: None,
        },
    );
    state.buffers.insert(
        FOREIGN_BUFFER_ID,
        BufferRecord {
            id: FOREIGN_BUFFER_ID,
            title: "foreign".to_owned(),
            command: vec!["/bin/sh".to_owned()],
            cwd: Some("/tmp".to_owned()),
            kind: BufferRecordKind::Pty,
            pid: None,
            env: Default::default(),
            state: BufferRecordState::Running,
            attachment_node_id: Some(FOREIGN_NODE_ID),
            read_only: false,
            helper_source_buffer_id: None,
            helper_scope: None,
            pty_size: PtySize::new(80, 24),
            activity: ActivityState::Idle,
            last_snapshot_seq: 1,
            exit_code: None,
            pipe: None,
            user_options: Default::default(),
            foreground_command: None,
            bell_sequence: 0,
            host: None,
        },
    );

    let presentation = PresentationModel::project(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
    )
    .expect("projection succeeds");

    assert_eq!(
        presentation.root_tabs.as_ref().map(|tabs| tabs.node_id),
        Some(ROOT_TABS_ID)
    );
    assert_eq!(
        presentation.focused_leaf().map(|leaf| leaf.node_id),
        Some(FOCUSED_LEAF_ID)
    );
    assert!(
        presentation
            .leaves
            .iter()
            .all(|leaf| leaf.node_id != FOREIGN_NODE_ID)
    );
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

#[test]
fn panes_without_title_rows_draw_content_in_their_whole_rect() {
    let state = root_buffer_state();
    let viewport = Size {
        width: 40,
        height: 14,
    };
    let presentation =
        PresentationModel::project_with(&state, SESSION_ID, viewport, TabBarPosition::Top).unwrap();

    let leaf = &presentation.leaves[0];
    assert_eq!(leaf.title_rect, None);
    assert_eq!(leaf.content_rect, leaf.rect);
    assert_eq!(leaf.content_rect.size, viewport);
    assert_eq!(leaf.pty_size(), Some(PtySize::new(40, 14)));
}

#[test]
fn pane_title_rows_take_the_top_or_bottom_row_of_each_pane() {
    let state = root_buffer_state();
    let viewport = Size {
        width: 40,
        height: 14,
    };
    let project = |pane_titles| {
        PresentationModel::project_with(&state, SESSION_ID, viewport, pane_titles)
            .unwrap()
            .leaves[0]
            .clone()
    };

    let top = project(PaneTitles::Top);
    let title = top.title_rect.expect("title row");
    assert_eq!(title.origin, Point { x: 0, y: 0 });
    assert_eq!(title.size.height, 1);
    assert_eq!(top.content_rect.origin, Point { x: 0, y: 1 });
    assert_eq!(top.content_rect.size.height, 13);
    assert_eq!(top.pty_size(), Some(PtySize::new(40, 13)));

    let bottom = project(PaneTitles::Bottom);
    assert_eq!(
        bottom.title_rect.expect("title row").origin,
        Point { x: 0, y: 13 }
    );
    assert_eq!(bottom.content_rect.origin, Point { x: 0, y: 0 });
    assert_eq!(bottom.content_rect.size.height, 13);
    assert_eq!(bottom.pty_size(), Some(PtySize::new(40, 13)));
}

#[test]
fn split_panes_without_title_rows_keep_their_divider() {
    let state = root_split_state();
    let presentation = PresentationModel::project_with(
        &state,
        SESSION_ID,
        Size {
            width: 40,
            height: 14,
        },
        TabBarPosition::Top,
    )
    .unwrap();

    assert_eq!(presentation.dividers.len(), 1);
    let divider = presentation.dividers[0].rect;
    assert_eq!(
        divider.size,
        Size {
            width: 1,
            height: 14
        }
    );
    let mut widths = 0;
    for leaf in &presentation.leaves {
        assert_eq!(leaf.title_rect, None);
        assert!(!leaf.rect.contains(divider.origin));
        let size = leaf.pty_size().expect("pane has content");
        assert_eq!(size.rows, 14);
        widths += size.cols;
    }
    assert_eq!(widths + 1, 40);
}

#[test]
fn framed_popups_have_no_title_rows_and_borderless_ones_follow_the_setting() {
    let viewport = Size {
        width: 40,
        height: 14,
    };
    for border in [true, false] {
        let mut state = demo_state();
        state.floating.get_mut(&FLOATING_ID).unwrap().border = border;
        for pane_titles in [PaneTitles::Off, PaneTitles::Top, PaneTitles::Bottom] {
            let presentation =
                PresentationModel::project_with(&state, SESSION_ID, viewport, pane_titles).unwrap();
            let popup = &presentation.floating[0];
            let popup_leaves = presentation
                .leaves
                .iter()
                .filter(|leaf| leaf.floating_id.is_some())
                .collect::<Vec<_>>();
            assert_eq!(popup_leaves.len(), 2);
            for leaf in popup_leaves {
                let context = format!("border={border} {pane_titles:?} {leaf:?}");
                // The frame carries a framed popup's title, so its panes
                // have no title row; a borderless popup's panes have one
                // where tiled panes do.
                let title_row = !border && pane_titles.shown();
                assert_eq!(leaf.title_rect.is_some(), title_row, "{context}");
                if let Some(title) = leaf.title_rect {
                    let y = match pane_titles {
                        PaneTitles::Top => leaf.rect.origin.y,
                        _ => leaf.rect.origin.y + i32::from(leaf.rect.size.height) - 1,
                    };
                    assert_eq!(title.origin.y, y, "{context}");
                }
                // Everything stays inside the popup's content area.
                assert!(popup.content_rect.contains(leaf.rect.origin), "{context}");
            }
            if !border && !pane_titles.shown() {
                // The content fills the whole popup, as wisp's picker needs.
                assert_eq!(popup.content_rect, popup.rect);
                let top = presentation
                    .leaves
                    .iter()
                    .find(|leaf| leaf.node_id == FLOATING_TOP_LEAF_ID)
                    .unwrap();
                assert_eq!(top.content_rect.origin, popup.rect.origin);
            }
        }
    }
}

/// Every root tab bar and status line layout, with every title row setting.
fn every_layout() -> Vec<(StatusLineMode, LayoutOptions)> {
    let mut layouts = Vec::new();
    for status_line in [StatusLineMode::Overlay, StatusLineMode::Separate] {
        for position in [TabBarPosition::Top, TabBarPosition::Bottom] {
            for visible in [true, false] {
                for pane_titles in [PaneTitles::Off, PaneTitles::Top, PaneTitles::Bottom] {
                    layouts.push((
                        status_line,
                        LayoutOptions::new(RootTabBar { position, visible }, pane_titles),
                    ));
                }
            }
        }
    }
    layouts
}

#[test]
fn title_rows_and_root_tab_bar_layouts_hold_together() {
    let state = demo_state();
    let terminal = Size {
        width: 40,
        height: 14,
    };
    for (status_line, options) in every_layout() {
        let layout = format!("{status_line:?} {options:?}");
        let viewport = status_line.content_viewport(terminal);
        let model = PresentationModel::project_with(&state, SESSION_ID, viewport, options).unwrap();
        let bar = model.root_tabs.as_ref().unwrap().rect;

        for leaf in model
            .leaves
            .iter()
            .filter(|leaf| leaf.floating_id.is_none())
        {
            // Title rows take a row of the pane and nothing of the bar's.
            let title_rows = u16::from(options.pane_titles.shown());
            assert_eq!(leaf.title_rect.is_some(), title_rows == 1, "{layout}");
            assert_eq!(
                leaf.content_rect.size.height + title_rows,
                leaf.rect.size.height,
                "{layout}"
            );
            assert_eq!(leaf.content_rect.size.width, leaf.rect.size.width);
            let rows = |rect: embers_core::Rect| {
                rect.origin.y..rect.origin.y + i32::from(rect.size.height)
            };
            for y in rows(leaf.rect) {
                assert!(!rows(bar).contains(&y), "{layout}: pane over the bar");
            }
            if let Some(title) = leaf.title_rect {
                let expected_y = match options.pane_titles {
                    PaneTitles::Top => leaf.rect.origin.y,
                    _ => leaf.rect.origin.y + i32::from(leaf.rect.size.height) - 1,
                };
                assert_eq!(title.origin.y, expected_y, "{layout}");
                assert!(!rows(leaf.content_rect).contains(&title.origin.y));
            }
            assert_eq!(
                leaf.pty_size().map(|size| size.rows),
                Some(leaf.content_rect.size.height),
                "{layout}"
            );
        }

        // The status row is the bar's, or the last row without one shown,
        // whatever the title rows are.
        let status_row = if options.tab_bar.visible {
            u16::try_from(bar.origin.y).unwrap()
        } else {
            viewport.height - 1
        };
        assert_eq!(model.status_row(), Some(status_row), "{layout}");

        // The client reports the layout viewport and whether title rows are
        // shown. The CLI, projecting that with the bar shown at the top and
        // the reported title rows, sizes every pane as the client does.
        let layout_viewport = model.layout_viewport();
        assert_eq!(
            layout_viewport.height,
            viewport.height + u16::from(!options.tab_bar.visible),
            "{layout}"
        );
        let reported = if options.pane_titles.shown() {
            PaneTitles::Top
        } else {
            PaneTitles::Off
        };
        let cli =
            PresentationModel::project_with(&state, SESSION_ID, layout_viewport, reported).unwrap();
        let pty_sizes = |model: &PresentationModel| {
            model
                .leaves
                .iter()
                .map(|leaf| (leaf.node_id, leaf.pty_size()))
                .collect::<Vec<_>>()
        };
        assert_eq!(pty_sizes(&model), pty_sizes(&cli), "{layout}");
    }
}

#[test]
fn without_title_rows_a_hidden_root_tab_bar_gives_panes_every_row() {
    let state = demo_state();
    let viewport = Size {
        width: 40,
        height: 14,
    };
    let model = PresentationModel::project_with(
        &state,
        SESSION_ID,
        viewport,
        LayoutOptions::new(
            RootTabBar {
                position: TabBarPosition::Bottom,
                visible: false,
            },
            PaneTitles::Off,
        ),
    )
    .unwrap();

    let left = model
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == LEFT_LEAF_ID)
        .unwrap();
    assert_eq!(left.title_rect, None);
    assert_eq!(left.content_rect, left.rect);
    assert_eq!(left.rect.origin.y, 0);
    assert_eq!(left.rect.size.height, 14);
    assert_eq!(left.pty_size().map(|size| size.rows), Some(14));
    // Status content goes over the last pane row, and the client reports a
    // viewport with the hidden bar's row added.
    assert_eq!(model.status_row(), Some(13));
    assert_eq!(
        model.layout_viewport(),
        Size {
            width: 40,
            height: 15
        }
    );
}

#[test]
fn top_title_rows_sit_under_the_window_with_the_root_tab_bar_at_the_bottom() {
    let state = demo_state();
    let viewport = Size {
        width: 40,
        height: 14,
    };
    let model = PresentationModel::project_with(
        &state,
        SESSION_ID,
        viewport,
        LayoutOptions::new(TabBarPosition::Bottom, PaneTitles::Top),
    )
    .unwrap();

    assert_eq!(model.root_tabs.as_ref().unwrap().rect.origin.y, 13);
    let left = model
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == LEFT_LEAF_ID)
        .unwrap();
    assert_eq!(left.title_rect.unwrap().origin, Point { x: 0, y: 0 });
    assert_eq!(left.content_rect.origin, Point { x: 0, y: 1 });
    assert_eq!(left.content_rect.size.height, 12);
    assert_eq!(left.pty_size().map(|size| size.rows), Some(12));
    assert_eq!(model.status_row(), Some(13));
    assert_eq!(model.layout_viewport(), viewport);
}
