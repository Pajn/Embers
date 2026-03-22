mod support;

use embers_client::{Controller, KeyEvent, PresentationModel};
use embers_core::{RequestId, Size};
use embers_protocol::{ClientMessage, FloatingRequest, InputRequest, NodeRequest};

use support::{
    FLOATING_ID, FOCUSED_BUFFER_ID, LEFT_LEAF_ID, NESTED_TABS_ID, ROOT_TABS_ID, SESSION_ID,
    demo_state, floating_focused_state, root_focus_state, root_split_state,
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
fn alt_digit_targets_root_tabs_when_focus_is_not_nested() {
    let state = root_focus_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(9), KeyEvent::Alt('2'))
        .expect("root tab request");

    assert_eq!(
        request,
        ClientMessage::Node(NodeRequest::SelectTab {
            request_id: RequestId(9),
            tabs_node_id: ROOT_TABS_ID,
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
            buffer_id: FOCUSED_BUFFER_ID,
            bytes: vec![b'x'],
        })
    );
}

#[test]
fn alt_digit_is_ignored_without_focused_tabs_context() {
    let state = root_split_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    assert_eq!(
        Controller.map_key(&presentation, RequestId(12), KeyEvent::Alt('1')),
        None
    );
}

#[test]
fn unbound_ctrl_key_is_forwarded_to_the_focused_buffer() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(13), KeyEvent::Ctrl('z'))
        .expect("input request");

    assert_eq!(
        request,
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(13),
            buffer_id: FOCUSED_BUFFER_ID,
            bytes: vec![0x1a],
        })
    );
}

#[test]
fn unbound_alt_key_is_forwarded_to_the_focused_buffer() {
    let state = demo_state();
    let presentation =
        PresentationModel::project(&state, SESSION_ID, TEST_SIZE).expect("projection succeeds");

    let request = Controller
        .map_key(&presentation, RequestId(14), KeyEvent::Alt('x'))
        .expect("input request");

    assert_eq!(
        request,
        ClientMessage::Input(InputRequest::Send {
            request_id: RequestId(14),
            buffer_id: FOCUSED_BUFFER_ID,
            bytes: vec![0x1b, b'x'],
        })
    );
}
