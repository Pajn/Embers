use embers_core::RequestId;
use embers_protocol::{ClientMessage, FloatingRequest, InputRequest, NodeRequest};

use crate::presentation::{NavigationDirection, PresentationModel};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Char(char),
    Bytes(Vec<u8>),
    Enter,
    Tab,
    Backspace,
    Escape,
    Ctrl(char),
    Alt(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(Option<MouseButton>),
    Drag(MouseButton),
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub row: u16,
    pub column: u16,
    pub modifiers: MouseModifiers,
    pub kind: MouseEventKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controller;

impl Controller {
    pub fn map_key(
        &self,
        presentation: &PresentationModel,
        request_id: RequestId,
        key: KeyEvent,
    ) -> Option<ClientMessage> {
        match key {
            KeyEvent::Ctrl(ch) => {
                let ch = ch.to_ascii_lowercase();
                match ch {
                    'h' | 'j' | 'k' | 'l' => {
                        let direction = match ch {
                            'h' => NavigationDirection::Left,
                            'j' => NavigationDirection::Down,
                            'k' => NavigationDirection::Up,
                            'l' => NavigationDirection::Right,
                            _ => unreachable!(),
                        };

                        Some(ClientMessage::Node(NodeRequest::Focus {
                            request_id,
                            session_id: presentation.session_id,
                            node_id: presentation.focus_target(direction)?,
                        }))
                    }
                    _ => input_request(presentation, request_id, vec![ctrl_byte(ch)?]),
                }
            }
            KeyEvent::Alt(ch) if ('1'..='9').contains(&ch) => {
                let index = ch.to_digit(10)?.saturating_sub(1);
                let index_usize = usize::try_from(index).ok()?;
                let tabs = presentation.focused_tabs()?;
                if index_usize >= tabs.tabs.len() {
                    return None;
                }

                Some(ClientMessage::Node(NodeRequest::SelectTab {
                    request_id,
                    tabs_node_id: tabs.node_id,
                    index,
                }))
            }
            KeyEvent::Alt(ch) => {
                let mut encoded = [0; 4];
                let mut bytes = vec![0x1b];
                bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                input_request(presentation, request_id, bytes)
            }
            KeyEvent::Escape => {
                if let Some(floating_id) = presentation.focused_floating_id() {
                    Some(ClientMessage::Floating(FloatingRequest::Close {
                        request_id,
                        floating_id,
                    }))
                } else {
                    input_request(presentation, request_id, vec![0x1b])
                }
            }
            KeyEvent::Char(ch) => {
                input_request(presentation, request_id, ch.to_string().into_bytes())
            }
            KeyEvent::Bytes(bytes) if !bytes.is_empty() => {
                input_request(presentation, request_id, bytes)
            }
            KeyEvent::Tab => input_request(presentation, request_id, b"\t".to_vec()),
            KeyEvent::Enter => input_request(presentation, request_id, b"\r".to_vec()),
            KeyEvent::Backspace => input_request(presentation, request_id, vec![0x7f]),
            KeyEvent::Up => input_request(presentation, request_id, b"\x1b[A".to_vec()),
            KeyEvent::Down => input_request(presentation, request_id, b"\x1b[B".to_vec()),
            KeyEvent::Right => input_request(presentation, request_id, b"\x1b[C".to_vec()),
            KeyEvent::Left => input_request(presentation, request_id, b"\x1b[D".to_vec()),
            KeyEvent::Home => input_request(presentation, request_id, b"\x1b[H".to_vec()),
            KeyEvent::End => input_request(presentation, request_id, b"\x1b[F".to_vec()),
            KeyEvent::Insert => input_request(presentation, request_id, b"\x1b[2~".to_vec()),
            KeyEvent::Delete => input_request(presentation, request_id, b"\x1b[3~".to_vec()),
            KeyEvent::PageUp => input_request(presentation, request_id, b"\x1b[5~".to_vec()),
            KeyEvent::PageDown => input_request(presentation, request_id, b"\x1b[6~".to_vec()),
            KeyEvent::Bytes(_) => None,
        }
    }
}

fn ctrl_byte(ch: char) -> Option<u8> {
    ch.is_ascii()
        .then_some((ch.to_ascii_lowercase() as u8) & 0x1f)
}

fn input_request(
    presentation: &PresentationModel,
    request_id: RequestId,
    bytes: Vec<u8>,
) -> Option<ClientMessage> {
    Some(ClientMessage::Input(InputRequest::Send {
        request_id,
        buffer_id: presentation.focused_buffer_id()?,
        bytes,
    }))
}
