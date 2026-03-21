mod support;

use embers_client::{Controller, KeyEvent, PresentationModel};
use embers_core::{RequestId, Size};
use embers_protocol::{ClientMessage, FloatingRequest, InputRequest, NodeRequest, SessionRequest};

use support::{
    FLOATING_ID, LEFT_LEAF_ID, NESTED_TABS_ID, SESSION_ID, demo_state, floating_focused_state,
    root_focus_state,
};

const TEST_SIZE: Size = Size {
    width: 40,
    height: 14,
};

#[test]
fn ctrl_h_focuses_neighboring_leaf() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(7), KeyEvent::Ctrl('h'))
        .expect("focus request");

    assert_eq!(
        request,
        ClientMessage::Node(NodeRequest::Focus {
            request_id: RequestId(7),
            session_id: SESSION_ID,
            node_id: LEFT_LEAF_ID,
        })
    );
}

#[test]
fn alt_digit_targets_deepest_visible_tabs_group() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(8), KeyEvent::Alt('1'))
        .expect("tab request");

    assert_eq!(
        request,
        ClientMessage::Node(NodeRequest::SelectTab {
            request_id: RequestId(8),
            tabs_node_id: NESTED_TABS_ID,
            index: 0,
        })
    );
}

#[test]
fn alt_digit_falls_back_to_root_tabs_when_focus_is_not_nested() {
    let state = root_focus_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(9), KeyEvent::Alt('2'))
        .expect("root tab request");

    assert_eq!(
        request,
        ClientMessage::Session(SessionRequest::SelectRootTab {
            request_id: RequestId(9),
            session_id: SESSION_ID,
            index: 1,
        })
    );
}

#[test]
fn escape_closes_focused_popup() {
    let state = floating_focused_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(10), KeyEvent::Escape)
        .expect("popup close request");

    assert_eq!(
        request,
        ClientMessage::Floating(FloatingRequest::Close {
            request_id: RequestId(10),
            floating_id: FLOATING_ID,
        })
    );
}

#[test]
fn plain_input_routes_to_focused_buffer() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(11), KeyEvent::Char('x'))
        .expect("input request");

    assert_eq!(
        request,
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(11),
            buffer_id: embers_core::BufferId(4),
            bytes: vec![b'x'],
        })
    );
}
