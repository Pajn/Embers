use serde::{Deserialize, Serialize};

/// The clipboard an OSC 52 write targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardSelection {
    /// The system clipboard (`c`).
    #[default]
    Clipboard,
    /// The primary selection (`p`; programs may also send `s`).
    Primary,
}

impl ClipboardSelection {
    /// The OSC 52 selection parameter for this clipboard.
    pub const fn osc52_parameter(self) -> char {
        match self {
            Self::Clipboard => 'c',
            Self::Primary => 'p',
        }
    }
}

/// A clipboard write (OSC 52 store) made by a program in a pane, already
/// base64-decoded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardWrite {
    pub selection: ClipboardSelection,
    pub text: String,
}

/// Largest clipboard write, in bytes of decoded text, that Embers forwards.
/// Larger writes are dropped, which keeps the event well inside one protocol
/// frame (8 MiB).
pub const MAX_CLIPBOARD_WRITE_BYTES: usize = 4 * 1024 * 1024;
