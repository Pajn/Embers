use crate::input::{KeyCode, Modifiers};

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
    /// A key carrying an explicit modifier set (shift/multi-modifier combos and
    /// function keys), produced by CSI-u aware host-terminal parsing.
    Key {
        code: KeyCode,
        mods: Modifiers,
    },
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
