use thiserror::Error;

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
    PageUp,
    PageDown,
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
        "pageup" | "pgup" => Ok(KeyToken::PageUp),
        "pagedown" | "pgdown" | "pgdn" => Ok(KeyToken::PageDown),
        _ => parse_modified_token(token),
    }
}

fn parse_modified_token(token: &str) -> Result<KeyToken, KeyParseError> {
    let Some((modifier, key)) = token.split_once('-') else {
        return single_char_token(token).map(KeyToken::Char).ok_or_else(|| {
            KeyParseError::InvalidToken {
                token: token.to_owned(),
            }
        });
    };

    let ch = single_char_token(key).ok_or_else(|| KeyParseError::InvalidModifiedKey {
        token: token.to_owned(),
    })?;

    match modifier.to_ascii_lowercase().as_str() {
        "c" | "ctrl" => Ok(KeyToken::Ctrl(ch.to_ascii_lowercase())),
        "a" | "alt" | "m" => Ok(KeyToken::Alt(ch.to_ascii_lowercase())),
        _ => Err(KeyParseError::InvalidModifier {
            token: token.to_owned(),
        }),
    }
}

fn single_char_token(token: &str) -> Option<char> {
    let mut chars = token.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

#[cfg(test)]
mod tests {
    use super::{expand_leader, parse_key_sequence, KeyParseError, KeyToken};

    #[test]
    fn parses_plain_and_modified_keys() {
        assert_eq!(
            parse_key_sequence("ab<C-x><A-z><Enter><Esc><Tab><Space><Up><PageDown>").unwrap(),
            vec![
                KeyToken::Char('a'),
                KeyToken::Char('b'),
                KeyToken::Ctrl('x'),
                KeyToken::Alt('z'),
                KeyToken::Enter,
                KeyToken::Escape,
                KeyToken::Tab,
                KeyToken::Space,
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
