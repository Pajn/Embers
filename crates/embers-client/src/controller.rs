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
                let direction = match ch.to_ascii_lowercase() {
                    'h' => NavigationDirection::Left,
                    'j' => NavigationDirection::Down,
                    'k' => NavigationDirection::Up,
                    'l' => NavigationDirection::Right,
                    _ => return None,
                };

                Some(ClientMessage::Node(NodeRequest::Focus {
                    request_id,
                    session_id: presentation.session_id,
                    node_id: presentation.focus_target(direction)?,
                }))
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
            KeyEvent::Alt(_) | KeyEvent::Bytes(_) => None,
        }
    }
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
