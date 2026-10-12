use embers_client::{
    LayoutOptions, PanePlacement, PaneTitles, PresentationModel, RootTabBar, TabBarPosition,
    spawn_pty_size, spawn_pty_size_with,
};
use embers_core::{BufferId, FloatGeometry, NodeId, PtySize, Size, SplitDirection};

use crate::support::{
    FLOATING_TOP_LEAF_ID, FOCUSED_BUFFER_ID, FOCUSED_LEAF_ID, NESTED_TABS_ID,
    ROOT_SPLIT_LEFT_LEAF_ID, ROOT_SPLIT_RIGHT_LEAF_ID, ROOT_TABS_ID, SESSION_ID, demo_state,
    root_buffer_state, root_split_state,
};

const VIEWPORT: Size = Size {
    width: 41,
    height: 21,
};

#[test]
fn split_gives_the_new_pane_the_second_half_of_the_leaf() {
    let state = root_split_state();
    // The right pane is 20 columns of the 41 (one is the divider) by 21 rows.
    // Stacked, it keeps 20 rows besides the new divider: 10 for the new pane,
    // minus its title row.
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Split {
                leaf_id: ROOT_SPLIT_RIGHT_LEAF_ID,
                direction: SplitDirection::Horizontal,
            },
        ),
        Some(PtySize::new(20, 9))
    );
    // Side by side, the 20 columns lose one to the new divider.
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Split {
                leaf_id: ROOT_SPLIT_RIGHT_LEAF_ID,
                direction: SplitDirection::Vertical,
            },
        ),
        Some(PtySize::new(10, 20))
    );
}

#[test]
fn split_inside_a_floating_window_stays_inside_its_border() {
    let state = demo_state();
    let presentation =
        PresentationModel::project_with(&state, SESSION_ID, VIEWPORT, PaneTitles::Top)
            .expect("projection succeeds");
    let top = presentation
        .leaves
        .iter()
        .find(|leaf| leaf.node_id == FLOATING_TOP_LEAF_ID)
        .expect("floating leaf is drawn");
    let size = spawn_pty_size(
        &state,
        SESSION_ID,
        VIEWPORT,
        PaneTitles::Top,
        PanePlacement::Split {
            leaf_id: FLOATING_TOP_LEAF_ID,
            direction: SplitDirection::Vertical,
        },
    )
    .expect("new pane is drawn");
    // The divider takes a column and the second pane any odd one left over.
    let shared = top.rect.size.width - 1;
    assert_eq!(size.cols, shared - shared / 2);
    assert_eq!(size.rows, top.rect.size.height - 1);
}

#[test]
fn new_window_takes_the_area_below_the_root_tab_bar() {
    assert_eq!(
        spawn_pty_size(
            &demo_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::RootTab,
        ),
        Some(PtySize::new(41, 19))
    );
    // The server only adds windows to a root tabs container.
    assert_eq!(
        spawn_pty_size(
            &root_buffer_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::RootTab,
        ),
        None
    );
    assert_eq!(
        spawn_pty_size(
            &demo_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::RootTab,
        ),
        Some(PtySize::new(41, 19))
    );
}

#[test]
fn a_hidden_root_tab_bar_leaves_its_row_to_new_windows() {
    let state = demo_state();
    let hidden = RootTabBar {
        position: TabBarPosition::Bottom,
        visible: false,
    };
    let size = spawn_pty_size_with(&state, SESSION_ID, VIEWPORT, hidden, PanePlacement::RootTab);
    // Without title rows, the default, the window has every row.
    assert_eq!(size, Some(PtySize::new(41, 21)));

    // The CLI, given the viewport the client reports, agrees.
    let reported = PresentationModel::project_with(&state, SESSION_ID, VIEWPORT, hidden)
        .unwrap()
        .layout_viewport();
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            reported,
            PaneTitles::Off,
            PanePlacement::RootTab
        ),
        size
    );
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            reported,
            PaneTitles::Off,
            PanePlacement::Split {
                leaf_id: FOCUSED_LEAF_ID,
                direction: SplitDirection::Horizontal,
            },
        ),
        spawn_pty_size_with(
            &state,
            SESSION_ID,
            VIEWPORT,
            hidden,
            PanePlacement::Split {
                leaf_id: FOCUSED_LEAF_ID,
                direction: SplitDirection::Horizontal,
            },
        )
    );
}

#[test]
fn new_tab_takes_the_area_of_its_tabs_container() {
    let state = demo_state();
    let presentation =
        PresentationModel::project_with(&state, SESSION_ID, VIEWPORT, PaneTitles::Top)
            .expect("projection succeeds");
    let bar = presentation
        .tab_bars
        .iter()
        .find(|bar| bar.node_id == NESTED_TABS_ID)
        .expect("nested tab bar is drawn");
    let size = spawn_pty_size(
        &state,
        SESSION_ID,
        VIEWPORT,
        PaneTitles::Top,
        PanePlacement::Tab {
            tabs_node_id: NESTED_TABS_ID,
        },
    )
    .expect("new tab is drawn");
    assert_eq!(size.cols, bar.rect.size.width);
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Tab {
                tabs_node_id: ROOT_TABS_ID,
            },
        ),
        Some(PtySize::new(41, 19))
    );
}

#[test]
fn popup_loses_its_border_and_title_row() {
    assert_eq!(
        spawn_pty_size(
            &root_buffer_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Floating {
                geometry: FloatGeometry::new(2, 2, 30, 10),
            },
        ),
        Some(PtySize::new(28, 7))
    );
    // A popup reaching past the viewport is clipped to it.
    assert_eq!(
        spawn_pty_size(
            &root_buffer_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Floating {
                geometry: FloatGeometry::new(30, 2, 30, 10),
            },
        ),
        Some(PtySize::new(9, 7))
    );
}

#[test]
fn replaced_pane_keeps_the_size_of_the_leaf() {
    assert_eq!(
        spawn_pty_size(
            &root_split_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Replace {
                leaf_id: ROOT_SPLIT_LEFT_LEAF_ID,
            },
        ),
        Some(PtySize::new(20, 20))
    );
}

#[test]
fn hidden_or_unknown_placements_have_no_size() {
    // A zoom on another pane hides the new one.
    let mut state = root_split_state();
    state
        .sessions
        .get_mut(&SESSION_ID)
        .expect("session")
        .zoomed_node_id = Some(ROOT_SPLIT_LEFT_LEAF_ID);
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Split {
                leaf_id: ROOT_SPLIT_RIGHT_LEAF_ID,
                direction: SplitDirection::Vertical,
            },
        ),
        None
    );
    assert_eq!(
        spawn_pty_size(
            &root_buffer_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Split {
                leaf_id: NodeId(999),
                direction: SplitDirection::Vertical,
            },
        ),
        None
    );
    // Splitting something other than a leaf is not a placement the server
    // accepts.
    assert_eq!(
        spawn_pty_size(
            &demo_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::Split {
                leaf_id: NESTED_TABS_ID,
                direction: SplitDirection::Vertical,
            },
        ),
        None
    );
    assert_eq!(
        spawn_pty_size(
            &root_buffer_state(),
            embers_core::SessionId(99),
            VIEWPORT,
            PaneTitles::Top,
            PanePlacement::RootTab,
        ),
        None
    );
}

#[test]
fn panes_without_title_rows_spawn_at_their_whole_area() {
    // The same placements as above, with no title rows to lose.
    assert_eq!(
        spawn_pty_size(
            &root_split_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Off,
            PanePlacement::Split {
                leaf_id: ROOT_SPLIT_RIGHT_LEAF_ID,
                direction: SplitDirection::Horizontal,
            },
        ),
        Some(PtySize::new(20, 10))
    );
    assert_eq!(
        spawn_pty_size(
            &demo_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Off,
            PanePlacement::RootTab,
        ),
        Some(PtySize::new(41, 20))
    );
    // Popup panes keep their title row.
    assert_eq!(
        spawn_pty_size(
            &root_buffer_state(),
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Off,
            PanePlacement::Floating {
                geometry: FloatGeometry::new(2, 2, 30, 10),
            },
        ),
        Some(PtySize::new(28, 7))
    );
}

#[test]
fn the_cli_spawns_panes_at_the_client_size_for_every_layout() {
    let state = demo_state();
    let placements = [
        PanePlacement::RootTab,
        PanePlacement::Tab {
            tabs_node_id: NESTED_TABS_ID,
        },
        PanePlacement::Split {
            leaf_id: FOCUSED_LEAF_ID,
            direction: SplitDirection::Horizontal,
        },
        PanePlacement::Split {
            leaf_id: FOCUSED_LEAF_ID,
            direction: SplitDirection::Vertical,
        },
    ];
    for position in [TabBarPosition::Top, TabBarPosition::Bottom] {
        for visible in [true, false] {
            for pane_titles in [PaneTitles::Off, PaneTitles::Top, PaneTitles::Bottom] {
                let options = LayoutOptions::new(RootTabBar { position, visible }, pane_titles);
                // What the client reports: the layout viewport, and whether
                // panes have title rows.
                let reported_viewport =
                    PresentationModel::project_with(&state, SESSION_ID, VIEWPORT, options)
                        .unwrap()
                        .layout_viewport();
                let reported_titles = if pane_titles.shown() {
                    PaneTitles::Top
                } else {
                    PaneTitles::Off
                };
                for placement in placements {
                    let client =
                        spawn_pty_size_with(&state, SESSION_ID, VIEWPORT, options, placement);
                    assert!(client.is_some(), "{options:?} {placement:?}");
                    assert_eq!(
                        spawn_pty_size(
                            &state,
                            SESSION_ID,
                            reported_viewport,
                            reported_titles,
                            placement
                        ),
                        client,
                        "{options:?} {placement:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn top_title_rows_with_the_root_tab_bar_at_the_bottom_take_a_row_from_new_windows() {
    let state = demo_state();
    // One row for the bar, one for the new pane's title.
    assert_eq!(
        spawn_pty_size_with(
            &state,
            SESSION_ID,
            VIEWPORT,
            LayoutOptions::new(TabBarPosition::Bottom, PaneTitles::Top),
            PanePlacement::RootTab,
        ),
        Some(PtySize::new(41, 19))
    );
}

#[test]
fn new_panes_take_the_cell_size_a_client_gave_the_sessions_panes() {
    let mut state = demo_state();
    // No pane has a pixel size yet: neither has the new one.
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Off,
            PanePlacement::RootTab
        ),
        Some(PtySize::new(41, 20))
    );

    // A hidden pane kept the cell size of an older client: it is used only
    // while no other pane has one.
    state
        .buffers
        .get_mut(&BufferId(1))
        .expect("hidden buffer")
        .pty_size = PtySize {
        cols: 80,
        rows: 24,
        pixel_width: 800,
        pixel_height: 528,
    };
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Off,
            PanePlacement::RootTab
        ),
        Some(PtySize {
            cols: 41,
            rows: 20,
            pixel_width: 410,
            pixel_height: 440,
        })
    );

    // The focused pane's cell size wins over it.
    state
        .buffers
        .get_mut(&FOCUSED_BUFFER_ID)
        .expect("focused buffer")
        .pty_size = PtySize {
        cols: 80,
        rows: 24,
        pixel_width: 720,
        pixel_height: 480,
    };
    assert_eq!(
        spawn_pty_size(
            &state,
            SESSION_ID,
            VIEWPORT,
            PaneTitles::Off,
            PanePlacement::RootTab
        ),
        Some(PtySize {
            cols: 41,
            rows: 20,
            pixel_width: 369,
            pixel_height: 400,
        })
    );
}
