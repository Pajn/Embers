//! Shared key encoding for both the host-terminal input controller and the
//! scripted `send_keys` path. Encodes a logical key (a [`KeyCode`] plus
//! [`Modifiers`]) either with legacy VT sequences or, when the target buffer has
//! negotiated the kitty keyboard protocol, with disambiguated CSI-u sequences.

/// Modifier flags carried by a key event or binding token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub super_: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        alt: false,
        ctrl: false,
        super_: false,
    };

    pub const fn is_empty(self) -> bool {
        !self.shift && !self.alt && !self.ctrl && !self.super_
    }

    /// The kitty modifier encoding: 1 + a bitmask (shift=1, alt=2, ctrl=4,
    /// super=8).
    pub const fn kitty_code(self) -> u32 {
        let mut mask = 0;
        if self.shift {
            mask |= 1;
        }
        if self.alt {
            mask |= 2;
        }
        if self.ctrl {
            mask |= 4;
        }
        if self.super_ {
            mask |= 8;
        }
        mask + 1
    }
}

/// A logical key, independent of its wire encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
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
    /// Function keys F1..=F12.
    Function(u8),
}

/// Kitty keyboard mode bits (a subset of the alacritty `TermMode` kitty flags).
/// `DISAMBIGUATE_ESC_CODES` is the flag that turns on CSI-u encoding. The bit
/// positions mirror the kitty protocol's own flag numbering (bit 0 disambiguate,
/// bit 3 report-all-keys) so the compact `keyboard_mode` byte never reuses a spec
/// bit position for a different meaning.
pub const KITTY_DISAMBIGUATE_ESC_CODES: u8 = 0b0000_0001;
pub const KITTY_REPORT_ALL_KEYS_AS_ESC: u8 = 0b0000_1000;

/// True when the mode requests disambiguated (CSI-u) encoding.
pub const fn mode_disambiguates(mode: u8) -> bool {
    mode & KITTY_DISAMBIGUATE_ESC_CODES != 0
}

const fn mode_reports_all_keys(mode: u8) -> bool {
    mode & KITTY_REPORT_ALL_KEYS_AS_ESC != 0
}

/// Encode a key press to the bytes a program expects, honoring the target's
/// keyboard mode.
pub fn encode_key(code: KeyCode, mods: Modifiers, mode: u8) -> Vec<u8> {
    // Either flag puts the program in CSI-u territory: disambiguate turns it on
    // for modified keys, and report-all-keys asks for every key as an escape
    // sequence. Route both through the kitty encoder; use legacy only when
    // neither bit is set.
    if mode_disambiguates(mode) || mode_reports_all_keys(mode) {
        encode_kitty(code, mods, mode)
    } else {
        encode_legacy(code, mods)
    }
}

/// The Unicode code point kitty uses to name a key in CSI-u encodings.
fn kitty_key_number(code: KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::Char(ch) => u32::from(ch),
        KeyCode::Enter => 13,
        KeyCode::Tab => 9,
        KeyCode::Backspace => 127,
        KeyCode::Escape => 27,
        // Functional keys are encoded as CSI ... letter/tilde forms instead.
        _ => return None,
    })
}

fn encode_kitty(code: KeyCode, mods: Modifiers, mode: u8) -> Vec<u8> {
    // Functional keys use the modified legacy CSI forms even under disambiguate.
    if let Some(sequence) = functional_csi(code, mods) {
        return sequence;
    }

    let Some(number) = kitty_key_number(code) else {
        return encode_legacy(code, mods);
    };

    // Disambiguation only kicks in for keys that would otherwise be ambiguous:
    // plain Tab/Enter/Esc/chars keep their literal bytes, and a shift-only
    // character still produces text (the kitty spec keeps shifted text-producing
    // keys as plain text under disambiguate), unless the program asked for every
    // key as an escape sequence.
    let shift_only_text = matches!(code, KeyCode::Char(_))
        && mods
            == (Modifiers {
                shift: true,
                ..Modifiers::NONE
            });
    if (mods.is_empty() || shift_only_text) && !mode_reports_all_keys(mode) {
        return encode_legacy(code, mods);
    }

    let modifiers = mods.kitty_code();
    if modifiers == 1 {
        format!("\x1b[{number}u").into_bytes()
    } else {
        format!("\x1b[{number};{modifiers}u").into_bytes()
    }
}

/// Legacy VT encoding, used when the kitty protocol is not active.
fn encode_legacy(code: KeyCode, mods: Modifiers) -> Vec<u8> {
    if let Some(sequence) = functional_csi(code, mods) {
        return sequence;
    }
    match code {
        KeyCode::Char(ch) => {
            // Fold shift into the character (Shift+a -> 'A'); the ctrl and alt
            // modifiers are applied on top so combinations survive.
            let ch = if mods.shift {
                ch.to_ascii_uppercase()
            } else {
                ch
            };
            let mut bytes = Vec::new();
            // Alt/Meta prefixes the whole sequence with ESC, ahead of a control
            // byte too (Ctrl+Alt+x -> ESC, then Ctrl-x).
            if mods.alt {
                bytes.push(0x1b);
            }
            if mods.ctrl
                && let Some(byte) = ctrl_byte(ch)
            {
                bytes.push(byte);
            } else {
                let mut buffer = [0; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            }
            bytes
        }
        KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Escape => {
            // Portable modified forms only: Shift+Tab is backtab, and Alt prefixes
            // ESC (matching the character handling above). Other modifier combos on
            // these keys (e.g. Ctrl+Backspace) have no portable legacy encoding, so
            // they fall back to the unmodified byte rather than an invented one.
            let base: &[u8] = if mods.shift && code == KeyCode::Tab {
                b"\x1b[Z"
            } else {
                match code {
                    KeyCode::Enter => b"\r",
                    KeyCode::Backspace => b"\x7f",
                    KeyCode::Escape => b"\x1b",
                    _ => b"\t",
                }
            };
            let mut bytes = Vec::new();
            if mods.alt {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(base);
            bytes
        }
        _ => Vec::new(),
    }
}

/// CSI sequences for functional keys (arrows, navigation, function keys),
/// inserting a modifier parameter when any modifier is held.
fn functional_csi(code: KeyCode, mods: Modifiers) -> Option<Vec<u8>> {
    let modifier = mods.kitty_code();
    let with_modifier = |suffix_letter: char| -> Vec<u8> {
        if modifier == 1 {
            format!("\x1b[{suffix_letter}").into_bytes()
        } else {
            format!("\x1b[1;{modifier}{suffix_letter}").into_bytes()
        }
    };
    let tilde = |number: u32| -> Vec<u8> {
        if modifier == 1 {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{modifier}~").into_bytes()
        }
    };
    Some(match code {
        KeyCode::Up => with_modifier('A'),
        KeyCode::Down => with_modifier('B'),
        KeyCode::Right => with_modifier('C'),
        KeyCode::Left => with_modifier('D'),
        KeyCode::Home => with_modifier('H'),
        KeyCode::End => with_modifier('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::Function(n) => return function_key_csi(n, modifier),
        _ => return None,
    })
}

/// Encode F1..=F12 using the conventional xterm sequences.
fn function_key_csi(n: u8, modifier: u32) -> Option<Vec<u8>> {
    // F1-F4 use SS3-style letters (with CSI when modified); F5-F12 use tilde
    // numbers. This matches xterm / kitty's default functional encodings.
    let sequence = match n {
        1..=4 => {
            let letter = b"PQRS"[usize::from(n - 1)] as char;
            if modifier == 1 {
                format!("\x1bO{letter}")
            } else if n == 3 {
                // `CSI 1;mods R` is also a cursor-position report, so modified
                // F3 uses its vt220 tilde number instead (as kitty does).
                format!("\x1b[13;{modifier}~")
            } else {
                format!("\x1b[1;{modifier}{letter}")
            }
        }
        5..=12 => {
            let number = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            if modifier == 1 {
                format!("\x1b[{number}~")
            } else {
                format!("\x1b[{number};{modifier}~")
            }
        }
        _ => return None,
    };
    Some(sequence.into_bytes())
}

/// Map a character to the control byte a terminal sends for Ctrl+<char>.
///
/// Letters and the `@A-Z[\]^_` block mask off the low five bits; the number row
/// and a few symbols aren't in that block and follow xterm's fixed conventions
/// (e.g. Ctrl+Space -> NUL, Ctrl+3 -> ESC). Characters with no control mapping
/// return `None`, so the caller emits the character itself instead of a blanket
/// (and wrong, e.g. Ctrl+3 -> 0x13) bitmask result.
fn ctrl_byte(ch: char) -> Option<u8> {
    let byte = match ch {
        ' ' | '2' | '@' => 0x00,
        '3' => 0x1b,
        '4' => 0x1c,
        '5' => 0x1d,
        '6' => 0x1e,
        '7' | '/' | '-' => 0x1f,
        '8' | '?' => 0x7f,
        'a'..='z' | 'A'..='Z' | '[' | '\\' | ']' | '^' | '_' => {
            (ch.to_ascii_uppercase() as u8) & 0x1f
        }
        _ => return None,
    };
    Some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers {
        shift: false,
        alt: false,
        ctrl: true,
        super_: false,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        shift: true,
        alt: false,
        ctrl: true,
        super_: false,
    };
    const CTRL_ALT: Modifiers = Modifiers {
        shift: false,
        alt: true,
        ctrl: true,
        super_: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        alt: false,
        ctrl: false,
        super_: false,
    };
    const ALT: Modifiers = Modifiers {
        shift: false,
        alt: true,
        ctrl: false,
        super_: false,
    };

    #[test]
    fn legacy_modified_c0_keys() {
        // Shift+Tab is backtab.
        assert_eq!(encode_key(KeyCode::Tab, SHIFT, 0), b"\x1b[Z".to_vec());
        // Alt prefixes ESC on the C0 keys, matching character handling.
        assert_eq!(encode_key(KeyCode::Enter, ALT, 0), b"\x1b\r".to_vec());
        // Unmodified C0 keys keep their plain legacy byte.
        assert_eq!(encode_key(KeyCode::Tab, Modifiers::NONE, 0), b"\t".to_vec());
        assert_eq!(
            encode_key(KeyCode::Backspace, Modifiers::NONE, 0),
            vec![0x7f]
        );
        // A modifier with no portable form on these keys falls back to the plain
        // byte rather than an invented sequence.
        assert_eq!(encode_key(KeyCode::Backspace, CTRL, 0), vec![0x7f]);
    }

    #[test]
    fn legacy_ctrl_char_uses_control_byte() {
        assert_eq!(encode_key(KeyCode::Char('i'), CTRL, 0), vec![0x09]);
        assert_eq!(encode_key(KeyCode::Tab, Modifiers::NONE, 0), vec![b'\t']);
    }

    #[test]
    fn disambiguate_ctrl_i_differs_from_tab() {
        let mode = KITTY_DISAMBIGUATE_ESC_CODES;
        // Ctrl+I becomes CSI 105 ; 5 u, distinct from a real Tab.
        assert_eq!(
            encode_key(KeyCode::Char('i'), CTRL, mode),
            b"\x1b[105;5u".to_vec()
        );
        assert_eq!(encode_key(KeyCode::Tab, Modifiers::NONE, mode), vec![b'\t']);
    }

    #[test]
    fn disambiguate_leaves_plain_chars_literal() {
        let mode = KITTY_DISAMBIGUATE_ESC_CODES;
        assert_eq!(encode_key(KeyCode::Char('a'), Modifiers::NONE, mode), b"a");
    }

    #[test]
    fn disambiguate_keeps_shifted_text_keys_as_text() {
        // A shift-only character still produces text under disambiguate-only
        // mode; only report-all-keys turns it into a CSI-u report.
        let mode = KITTY_DISAMBIGUATE_ESC_CODES;
        assert_eq!(encode_key(KeyCode::Char('a'), SHIFT, mode), b"A".to_vec());
        let all = KITTY_DISAMBIGUATE_ESC_CODES | KITTY_REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode_key(KeyCode::Char('a'), SHIFT, all),
            b"\x1b[97;2u".to_vec()
        );
    }

    #[test]
    fn report_all_keys_escapes_plain_chars() {
        let mode = KITTY_DISAMBIGUATE_ESC_CODES | KITTY_REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode_key(KeyCode::Char('a'), Modifiers::NONE, mode),
            b"\x1b[97u".to_vec()
        );
    }

    #[test]
    fn report_all_only_still_routes_through_kitty() {
        // report-all-keys without the disambiguate bit must still produce CSI-u,
        // not fall back to legacy encoding.
        let mode = KITTY_REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode_key(KeyCode::Char('a'), Modifiers::NONE, mode),
            b"\x1b[97u".to_vec()
        );
    }

    #[test]
    fn modified_arrows_carry_modifier_parameter() {
        assert_eq!(encode_key(KeyCode::Left, Modifiers::NONE, 0), b"\x1b[D");
        assert_eq!(
            encode_key(
                KeyCode::Left,
                Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                },
                0
            ),
            b"\x1b[1;2D".to_vec()
        );
    }

    #[test]
    fn ctrl_shift_tab_disambiguates() {
        let mode = KITTY_DISAMBIGUATE_ESC_CODES;
        assert_eq!(
            encode_key(KeyCode::Tab, CTRL_SHIFT, mode),
            b"\x1b[9;6u".to_vec()
        );
    }

    #[test]
    fn legacy_control_and_modifier_combinations() {
        // Ctrl+Space -> NUL.
        assert_eq!(encode_key(KeyCode::Char(' '), CTRL, 0), vec![0x00]);
        // Ctrl+3 -> ESC (xterm convention), not the blanket-masked 0x13.
        assert_eq!(encode_key(KeyCode::Char('3'), CTRL, 0), vec![0x1b]);
        // Ctrl+- -> US (0x1f), the readline undo chord.
        assert_eq!(encode_key(KeyCode::Char('-'), CTRL, 0), vec![0x1f]);
        // Ctrl+Alt+x -> ESC then Ctrl-x (alt prefix ahead of the control byte).
        assert_eq!(
            encode_key(KeyCode::Char('x'), CTRL_ALT, 0),
            vec![0x1b, 0x18]
        );
        // Shift folds into the character.
        assert_eq!(encode_key(KeyCode::Char('a'), SHIFT, 0), b"A".to_vec());
        // Ctrl+Shift+a stays the Ctrl-a control byte (case-insensitive).
        assert_eq!(encode_key(KeyCode::Char('a'), CTRL_SHIFT, 0), vec![0x01]);
    }

    #[test]
    fn function_keys_encode() {
        assert_eq!(
            encode_key(KeyCode::Function(1), Modifiers::NONE, 0),
            b"\x1bOP"
        );
        // Unmodified F3 is SS3 R, but the modified form avoids `CSI 1;mods R`
        // (a cursor-position report) in favor of the vt220 tilde number.
        assert_eq!(
            encode_key(KeyCode::Function(3), Modifiers::NONE, 0),
            b"\x1bOR"
        );
        assert_eq!(
            encode_key(KeyCode::Function(3), SHIFT, 0),
            b"\x1b[13;2~".to_vec()
        );
        assert_eq!(
            encode_key(KeyCode::Function(1), SHIFT, 0),
            b"\x1b[1;2P".to_vec()
        );
        assert_eq!(
            encode_key(KeyCode::Function(5), Modifiers::NONE, 0),
            b"\x1b[15~".to_vec()
        );
        assert_eq!(
            encode_key(KeyCode::Function(12), Modifiers::NONE, 0),
            b"\x1b[24~".to_vec()
        );
    }
}
