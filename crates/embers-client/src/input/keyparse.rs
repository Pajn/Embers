use thiserror::Error;

use super::encoding::{KeyCode, Modifiers};

pub type KeySequence = Vec<KeyToken>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyToken {
    Char(char),
    Ctrl(char),
    Alt(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    Space,
    Leader,
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
    /// A key carrying an explicit modifier set — used for shift/multi-modifier
    /// combinations and function keys that the legacy variants above cannot
    /// express (e.g. `<C-S-Tab>`, `<S-Left>`, `<F5>`).
    Key {
        code: KeyCode,
        mods: Modifiers,
    },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum KeyParseError {
    #[error("key sequence cannot be empty")]
    EmptySequence,
    #[error("key token '<{token}>' is invalid")]
    InvalidToken { token: String },
    #[error("key modifier in '<{token}>' is invalid")]
    InvalidModifier { token: String },
    #[error("key token '<{token}>' must contain exactly one character after the modifier")]
    InvalidModifiedKey { token: String },
    #[error(
        "key token '<{token}>' combines ctrl with a non-ASCII key, which has no control encoding"
    )]
    NonAsciiControl { token: String },
    #[error("key sequence '{notation}' has an unterminated token")]
    UnterminatedToken { notation: String },
    #[error("'<leader>' cannot be used before a leader is configured")]
    MissingLeader,
}

pub fn parse_key_sequence(notation: &str) -> Result<KeySequence, KeyParseError> {
    if notation.is_empty() {
        return Err(KeyParseError::EmptySequence);
    }

    let mut sequence = Vec::new();
    let chars = notation.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '<' => {
                let mut end = index + 1;
                while end < chars.len() && chars[end] != '>' {
                    end += 1;
                }
                if end >= chars.len() {
                    return Err(KeyParseError::UnterminatedToken {
                        notation: notation.to_owned(),
                    });
                }

                let token = chars[index + 1..end].iter().collect::<String>();
                sequence.push(parse_token(&token)?);
                index = end + 1;
            }
            ' ' => {
                sequence.push(KeyToken::Space);
                index += 1;
            }
            ch => {
                sequence.push(KeyToken::Char(ch));
                index += 1;
            }
        }
    }

    Ok(sequence)
}

pub fn expand_leader(
    sequence: impl IntoIterator<Item = KeyToken>,
    leader: &[KeyToken],
) -> Result<KeySequence, KeyParseError> {
    let mut expanded = Vec::new();
    for token in sequence {
        if token == KeyToken::Leader {
            if leader.is_empty() {
                return Err(KeyParseError::MissingLeader);
            }
            expanded.extend(leader.iter().cloned());
        } else {
            expanded.push(token);
        }
    }
    Ok(expanded)
}

fn parse_token(token: &str) -> Result<KeyToken, KeyParseError> {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "leader" => Ok(KeyToken::Leader),
        "enter" | "return" | "cr" => Ok(KeyToken::Enter),
        "esc" | "escape" => Ok(KeyToken::Escape),
        "bs" | "backspace" => Ok(KeyToken::Backspace),
        "tab" => Ok(KeyToken::Tab),
        "space" => Ok(KeyToken::Space),
        "up" => Ok(KeyToken::Up),
        "down" => Ok(KeyToken::Down),
        "left" => Ok(KeyToken::Left),
        "right" => Ok(KeyToken::Right),
        "home" => Ok(KeyToken::Home),
        "end" => Ok(KeyToken::End),
        "ins" | "insert" => Ok(KeyToken::Insert),
        "del" | "delete" => Ok(KeyToken::Delete),
        "pageup" | "pgup" => Ok(KeyToken::PageUp),
        "pagedown" | "pgdown" | "pgdn" => Ok(KeyToken::PageDown),
        _ => parse_modified_token(token),
    }
}

fn parse_modified_token(token: &str) -> Result<KeyToken, KeyParseError> {
    // Consume leading `X-` modifier prefixes; whatever remains is the base key.
    // A trailing `-` (e.g. `C--`, `C-S--`) is a literal hyphen base key rather
    // than an empty modifier segment, so stop once only the base remains.
    let mut mods = Modifiers::NONE;
    let mut had_modifier = false;
    let mut key = token;
    while let Some((head, tail)) = key.split_once('-') {
        // An empty head means the remainder starts with `-`: the hyphen base key.
        if head.is_empty() {
            break;
        }
        match head.to_ascii_lowercase().as_str() {
            "c" | "ctrl" => mods.ctrl = true,
            "a" | "alt" | "m" => mods.alt = true,
            "s" | "shift" => mods.shift = true,
            "d" | "super" | "cmd" | "win" => mods.super_ = true,
            _ => {
                return Err(KeyParseError::InvalidModifier {
                    token: token.to_owned(),
                });
            }
        }
        had_modifier = true;
        key = tail;
    }

    let error = || {
        if had_modifier {
            KeyParseError::InvalidModifiedKey {
                token: token.to_owned(),
            }
        } else {
            KeyParseError::InvalidToken {
                token: token.to_owned(),
            }
        }
    };
    let code = parse_base_key(key).ok_or_else(error)?;

    // A ctrl chord needs a control encoding, which only exists for ASCII keys.
    // Reject non-ASCII combinations loudly rather than silently sending the
    // bare character when the target hasn't negotiated CSI-u.
    if mods.ctrl && matches!(code, KeyCode::Char(ch) if !ch.is_ascii()) {
        return Err(KeyParseError::NonAsciiControl {
            token: token.to_owned(),
        });
    }

    // No modifiers: preserve the legacy plain-character token.
    if !had_modifier {
        return Ok(match code {
            KeyCode::Char(ch) => KeyToken::Char(ch),
            code => KeyToken::Key {
                code,
                mods: Modifiers::NONE,
            },
        });
    }

    // Preserve the legacy single-modifier character tokens so existing bindings
    // and encodings are unchanged.
    if let KeyCode::Char(ch) = code {
        if mods
            == (Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            })
        {
            return Ok(KeyToken::Ctrl(ch.to_ascii_lowercase()));
        }
        if mods
            == (Modifiers {
                alt: true,
                ..Modifiers::NONE
            })
        {
            return Ok(KeyToken::Alt(ch.to_ascii_lowercase()));
        }
    }

    // Kitty CSI-u reports carry the lowercase code point with shift as a
    // modifier bit, so a binding spelled <C-S-T> must store 't' to compare
    // equal to the incoming host event.
    let code = match code {
        KeyCode::Char(ch) => KeyCode::Char(ch.to_ascii_lowercase()),
        other => other,
    };
    Ok(KeyToken::Key { code, mods })
}

/// Resolve the textual name of a key to a [`KeyCode`].
fn parse_base_key(key: &str) -> Option<KeyCode> {
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" | "cr" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Escape),
        "bs" | "backspace" => Some(KeyCode::Backspace),
        "tab" => Some(KeyCode::Tab),
        "space" => Some(KeyCode::Char(' ')),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "ins" | "insert" => Some(KeyCode::Insert),
        "del" | "delete" => Some(KeyCode::Delete),
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdown" | "pgdn" => Some(KeyCode::PageDown),
        other => {
            if let Some(stripped) = other.strip_prefix('f')
                && let Ok(number) = stripped.parse::<u8>()
                && (1..=12).contains(&number)
            {
                return Some(KeyCode::Function(number));
            }
            single_char_token(key).map(KeyCode::Char)
        }
    }
}

fn single_char_token(token: &str) -> Option<char> {
    let mut chars = token.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

#[cfg(test)]
mod tests {
    use super::super::encoding::{KeyCode, Modifiers};
    use super::{KeyParseError, KeyToken, expand_leader, parse_key_sequence};

    #[test]
    fn parses_shift_multi_modifier_and_function_keys() {
        assert_eq!(
            parse_key_sequence("<C-S-Tab>").unwrap(),
            vec![KeyToken::Key {
                code: KeyCode::Tab,
                mods: Modifiers {
                    shift: true,
                    ctrl: true,
                    ..Modifiers::NONE
                },
            }]
        );
        assert_eq!(
            parse_key_sequence("<S-Left>").unwrap(),
            vec![KeyToken::Key {
                code: KeyCode::Left,
                mods: Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                },
            }]
        );
        assert_eq!(
            parse_key_sequence("<F5>").unwrap(),
            vec![KeyToken::Key {
                code: KeyCode::Function(5),
                mods: Modifiers::NONE,
            }]
        );
        assert_eq!(
            parse_key_sequence("<C-Tab>").unwrap(),
            vec![KeyToken::Key {
                code: KeyCode::Tab,
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
            }]
        );
    }

    #[test]
    fn hyphen_base_key_parses_with_modifiers() {
        // `-` as the base key: the trailing hyphen must not be mistaken for an
        // empty modifier segment (which previously returned InvalidModifier).
        assert_eq!(
            parse_key_sequence("<C-->").unwrap(),
            vec![KeyToken::Ctrl('-')]
        );
        assert_eq!(
            parse_key_sequence("<C-S-->").unwrap(),
            vec![KeyToken::Key {
                code: KeyCode::Char('-'),
                mods: Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Modifiers::NONE
                },
            }]
        );
    }

    #[test]
    fn multi_modifier_char_keys_store_lowercase() {
        // Kitty reports the lowercase code point with the shift bit, so the
        // natural uppercase spelling must produce the same token.
        assert_eq!(
            parse_key_sequence("<C-S-T>").unwrap(),
            parse_key_sequence("<C-S-t>").unwrap()
        );
        assert_eq!(
            parse_key_sequence("<C-S-T>").unwrap(),
            vec![KeyToken::Key {
                code: KeyCode::Char('t'),
                mods: Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Modifiers::NONE
                },
            }]
        );
    }

    #[test]
    fn rejects_non_ascii_control_chords() {
        assert_eq!(
            parse_key_sequence("<C-é>").unwrap_err(),
            KeyParseError::NonAsciiControl {
                token: "C-é".to_owned(),
            }
        );
    }

    #[test]
    fn single_modifier_char_keys_stay_legacy() {
        assert_eq!(
            parse_key_sequence("<C-x>").unwrap(),
            vec![KeyToken::Ctrl('x')]
        );
        assert_eq!(
            parse_key_sequence("<A-z>").unwrap(),
            vec![KeyToken::Alt('z')]
        );
    }

    #[test]
    fn rejects_invalid_modifier_and_function_combinations() {
        assert_eq!(
            parse_key_sequence("<F13>").unwrap_err(),
            KeyParseError::InvalidToken {
                token: "F13".to_owned(),
            }
        );
        assert_eq!(
            parse_key_sequence("<Hyper-Tab>").unwrap_err(),
            KeyParseError::InvalidModifier {
                token: "Hyper-Tab".to_owned(),
            }
        );
    }

    #[test]
    fn parses_plain_and_modified_keys() {
        assert_eq!(
            parse_key_sequence(
                "ab<C-x><A-z><Enter><Esc><Tab><Space><Home><Insert><Delete><End><Up><PageDown>",
            )
            .unwrap(),
            vec![
                KeyToken::Char('a'),
                KeyToken::Char('b'),
                KeyToken::Ctrl('x'),
                KeyToken::Alt('z'),
                KeyToken::Enter,
                KeyToken::Escape,
                KeyToken::Tab,
                KeyToken::Space,
                KeyToken::Home,
                KeyToken::Insert,
                KeyToken::Delete,
                KeyToken::End,
                KeyToken::Up,
                KeyToken::PageDown,
            ]
        );
    }

    #[test]
    fn rejects_invalid_tokens() {
        assert_eq!(
            parse_key_sequence("<Hyper-x>").unwrap_err(),
            KeyParseError::InvalidModifier {
                token: "Hyper-x".to_owned(),
            }
        );
        assert_eq!(
            parse_key_sequence("<C-ab>").unwrap_err(),
            KeyParseError::InvalidModifiedKey {
                token: "C-ab".to_owned(),
            }
        );
    }

    #[test]
    fn expands_leader_tokens() {
        let sequence = parse_key_sequence("<leader>ws").unwrap();
        let leader = parse_key_sequence("<C-a>").unwrap();

        assert_eq!(
            expand_leader(sequence, &leader).unwrap(),
            vec![
                KeyToken::Ctrl('a'),
                KeyToken::Char('w'),
                KeyToken::Char('s'),
            ]
        );
    }

    #[test]
    fn leader_expansion_requires_configured_leader() {
        let sequence = parse_key_sequence("<leader>x").unwrap();
        assert_eq!(
            expand_leader(sequence, &[]).unwrap_err(),
            KeyParseError::MissingLeader
        );
    }
}
