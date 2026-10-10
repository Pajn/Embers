use crate::CursorPosition;

/// A sequence a program in a pane asked to have written to the host terminal,
/// as tmux does with `allow-passthrough`: the unwrapped payload of
/// `ESC P tmux; ... ESC \`, or a kitty graphics command sent without the
/// wrapper (`ESC _ G ... ESC \`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassthroughWrite {
    /// The bytes to write to the host terminal, already unwrapped.
    pub bytes: Vec<u8>,
    /// The pane's cursor at the point in the output where the sequence ended.
    /// Clients put the host cursor there before writing `bytes`, because
    /// graphics protocols place images at the cursor.
    pub cursor: CursorPosition,
}

/// Largest passthrough payload, in bytes, that Embers forwards. Larger
/// sequences are dropped whole: a truncated graphics command would leave the
/// host terminal waiting for the rest of it. The cap keeps one payload well
/// inside a protocol frame (8 MiB), so a payload never has to be split.
pub const MAX_PASSTHROUGH_BYTES: usize = 4 * 1024 * 1024;
