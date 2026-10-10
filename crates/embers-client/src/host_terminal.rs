//! What the host terminal (the one running the client) supports beyond plain
//! xterm sequences, learned when the client attaches.
//!
//! The client asks the terminal who it is (XTVERSION) and whether it has the
//! terminfo capabilities for underline styles and colors (XTGETTCAP), followed
//! by a device attributes query (DA1). Every terminal answers DA1 and answers
//! queries in order, so the DA1 reply marks the end of the replies; terminals
//! that do not know the first two queries just skip them. The answers, or the
//! environment when there are none, decide the [`HostCapabilities`], and the
//! config can override each one (`set_terminal_features`).

use std::collections::BTreeSet;

/// Sequences the renderer only emits when the host terminal supports them.
/// Without them it degrades: plain underlines, no underline color, no OSC 8
/// hyperlinks and no passthrough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostCapabilities {
    /// Underline styles (`CSI 4:n m`: double, curly, dotted, dashed).
    pub underline_styles: bool,
    /// Underline color (`CSI 58:... m`).
    pub underline_color: bool,
    /// OSC 8 hyperlinks.
    pub hyperlinks: bool,
    /// Sequences programs in panes send for the host terminal, such as images.
    pub passthrough: bool,
}

impl HostCapabilities {
    pub const ALL: Self = Self {
        underline_styles: true,
        underline_color: true,
        hyperlinks: true,
        passthrough: true,
    };

    pub const NONE: Self = Self {
        underline_styles: false,
        underline_color: false,
        hyperlinks: false,
        passthrough: false,
    };

    /// These capabilities with the config's overrides applied.
    pub fn with_overrides(self, overrides: TerminalFeatures) -> Self {
        Self {
            underline_styles: overrides.underline_styles.unwrap_or(self.underline_styles),
            underline_color: overrides.underline_color.unwrap_or(self.underline_color),
            hyperlinks: overrides.hyperlinks.unwrap_or(self.hyperlinks),
            passthrough: overrides.passthrough.unwrap_or(self.passthrough),
        }
    }
}

/// Everything on: what a client that never detects its host (an embedder, a
/// test) renders, as before detection existed.
impl Default for HostCapabilities {
    fn default() -> Self {
        Self::ALL
    }
}

/// Capabilities the config forces on or off (`set_terminal_features`), whatever
/// detection found. `None` leaves a capability to detection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalFeatures {
    pub underline_styles: Option<bool>,
    pub underline_color: Option<bool>,
    pub hyperlinks: Option<bool>,
    pub passthrough: Option<bool>,
}

impl TerminalFeatures {
    /// The names `set_terminal_features` accepts.
    pub const NAMES: [&'static str; 4] = [
        "underline_styles",
        "underline_color",
        "hyperlinks",
        "passthrough",
    ];

    /// Set the feature called `name`; `false` when there is no such feature.
    pub fn set(&mut self, name: &str, enabled: bool) -> bool {
        let slot = match name {
            "underline_styles" => &mut self.underline_styles,
            "underline_color" => &mut self.underline_color,
            "hyperlinks" => &mut self.hyperlinks,
            "passthrough" => &mut self.passthrough,
            _ => return false,
        };
        *slot = Some(enabled);
        true
    }
}

/// What the client writes to the host terminal when it attaches: XTVERSION,
/// XTGETTCAP for `Smulx` and `Setulc`, then DA1, whose reply comes last.
pub const HOST_QUERIES: &str = "\x1b[>0q\x1bP+q536d756c78;536574756c63\x1b\\\x1b[c";

/// Longest reply to [`HOST_QUERIES`] worth reading; real ones are about 100
/// bytes.
pub const MAX_HOST_REPLY_BYTES: usize = 4096;

/// What the host terminal answered to [`HOST_QUERIES`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostReplies {
    /// The XTVERSION reply, for example `ghostty 1.1.3` or `kitty(0.35.2)`.
    pub version: Option<String>,
    /// Terminfo capabilities XTGETTCAP reported as present.
    pub termcaps: BTreeSet<String>,
    /// Whether the DA1 reply arrived, so every other reply had too.
    pub device_attributes: bool,
}

impl HostReplies {
    /// Pick the replies out of what the host sent back. Anything else in
    /// `bytes` is ignored.
    pub fn parse(bytes: &[u8]) -> Self {
        let mut replies = Self::default();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0x1b {
                index += 1;
                continue;
            }
            match bytes.get(index + 1) {
                Some(b'P') => {
                    let body_start = index + 2;
                    let Some(length) = find(&bytes[body_start..], b"\x1b\\") else {
                        break;
                    };
                    replies.add_dcs(&bytes[body_start..body_start + length]);
                    index = body_start + length + 2;
                }
                Some(b'[') => {
                    let body_start = index + 2;
                    let Some(length) = bytes[body_start..]
                        .iter()
                        .position(|byte| (0x40..=0x7e).contains(byte))
                    else {
                        break;
                    };
                    let body = &bytes[body_start..body_start + length];
                    if bytes[body_start + length] == b'c' && body.starts_with(b"?") {
                        replies.device_attributes = true;
                    }
                    index = body_start + length + 1;
                }
                _ => index += 1,
            }
        }
        replies
    }

    fn add_dcs(&mut self, body: &[u8]) {
        if let Some(version) = body.strip_prefix(b">|") {
            self.version = Some(String::from_utf8_lossy(version).into_owned());
        } else if let Some(found) = body.strip_prefix(b"1+r") {
            for entry in found.split(|&byte| byte == b';') {
                let name = entry.split(|&byte| byte == b'=').next().unwrap_or_default();
                if let Some(name) = hex_decode(name).and_then(|name| String::from_utf8(name).ok()) {
                    self.termcaps.insert(name);
                }
            }
        }
    }
}

/// Whether `bytes` ends with a DA1 reply (`CSI ? ... c`), the last reply to
/// [`HOST_QUERIES`].
pub fn ends_with_device_attributes(bytes: &[u8]) -> bool {
    let Some(head) = bytes.strip_suffix(b"c") else {
        return false;
    };
    let Some(start) = head.windows(3).rposition(|window| window == b"\x1b[?") else {
        return false;
    };
    head[start + 3..]
        .iter()
        .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

/// The terminfo capabilities [`HOST_QUERIES`] asks XTGETTCAP about, hex encoded
/// as in the query and its reply.
const QUERIED_TERMCAPS: [&[u8]; 2] = [b"536d756c78", b"536574756c63"];

/// Whether `reply`, one control sequence from the host terminal, answers one
/// of the client's own queries rather than a pane's: XTVERSION, XTGETTCAP for
/// the capabilities [`HOST_QUERIES`] asks about, DA1, or the kitty keyboard
/// flags (the client owns the host's keyboard mode). These can arrive after
/// attaching stopped waiting for them, and are dropped instead of being
/// handed to a pane.
pub fn is_attach_query_reply(reply: &[u8]) -> bool {
    if let Some(body) = reply
        .strip_prefix(b"\x1bP")
        .and_then(|body| body.strip_suffix(b"\x1b\\"))
    {
        if body.starts_with(b">|") {
            return true;
        }
        let Some(entries) = body
            .strip_prefix(b"1+r")
            .or_else(|| body.strip_prefix(b"0+r"))
        else {
            return false;
        };
        return entries.split(|&byte| byte == b';').all(|entry| {
            let name = entry.split(|&byte| byte == b'=').next().unwrap_or_default();
            QUERIED_TERMCAPS
                .iter()
                .any(|queried| name.eq_ignore_ascii_case(queried))
        });
    }
    let Some(body) = reply.strip_prefix(b"\x1b[?") else {
        return false;
    };
    let Some((&final_byte, params)) = body.split_last() else {
        return false;
    };
    matches!(final_byte, b'c' | b'u')
        && params
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

/// The parts of the client's environment that name the host terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostEnvironment {
    pub term: Option<String>,
    pub term_program: Option<String>,
}

impl HostEnvironment {
    pub fn from_process() -> Self {
        Self {
            term: std::env::var("TERM").ok(),
            term_program: std::env::var("TERM_PROGRAM").ok(),
        }
    }
}

/// Terminals known to support more than [`HostCapabilities::NONE`] whether or
/// not they answer XTGETTCAP, by the name [`host_terminal_name`] gives them.
const KNOWN_TERMINALS: &[(&str, HostCapabilities)] = &[
    ("ghostty", HostCapabilities::ALL),
    ("kitty", HostCapabilities::ALL),
    ("wezterm", HostCapabilities::ALL),
    ("foot", HostCapabilities::ALL),
    ("contour", HostCapabilities::ALL),
    (
        "iterm2",
        HostCapabilities {
            underline_styles: false,
            underline_color: false,
            hyperlinks: true,
            passthrough: true,
        },
    ),
    (
        "alacritty",
        HostCapabilities {
            underline_styles: true,
            underline_color: true,
            hyperlinks: true,
            passthrough: false,
        },
    ),
];

/// The host terminal's name, lowercased: from its XTVERSION reply when it sent
/// one, since that comes from the terminal itself, else from `TERM_PROGRAM`,
/// else from `TERM`.
pub fn host_terminal_name(replies: &HostReplies, env: &HostEnvironment) -> Option<String> {
    if let Some(version) = &replies.version {
        let name: String = version
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        return (!name.is_empty()).then(|| name.to_ascii_lowercase());
    }
    if let Some(program) = env.term_program.as_deref().filter(|name| !name.is_empty()) {
        let program = program.to_ascii_lowercase();
        return Some(match program.as_str() {
            "iterm.app" => "iterm2".to_owned(),
            _ => program,
        });
    }
    let term = env.term.as_deref()?.to_ascii_lowercase();
    let name = term.strip_prefix("xterm-").unwrap_or(&term);
    let name = name.split('-').next().unwrap_or(name);
    Some(name.to_owned())
}

/// Decide what the host terminal supports: what [`KNOWN_TERMINALS`] lists for
/// it, plus underline styles and colors when XTGETTCAP reported `Smulx` and
/// `Setulc`. A terminal that is neither known nor answers gets
/// [`HostCapabilities::NONE`].
pub fn detect_host_capabilities(replies: &HostReplies, env: &HostEnvironment) -> HostCapabilities {
    let known = host_terminal_name(replies, env)
        .and_then(|name| {
            KNOWN_TERMINALS
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, capabilities)| *capabilities)
        })
        .unwrap_or(HostCapabilities::NONE);
    HostCapabilities {
        underline_styles: known.underline_styles || replies.termcaps.contains("Smulx"),
        underline_color: known.underline_color || replies.termcaps.contains("Setulc"),
        ..known
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(term: &str, term_program: Option<&str>) -> HostEnvironment {
        HostEnvironment {
            term: Some(term.to_owned()),
            term_program: term_program.map(str::to_owned),
        }
    }

    #[test]
    fn parses_version_termcaps_and_device_attributes() {
        let bytes = b"\x1bP>|ghostty 1.1.3\x1b\\\
            \x1bP1+r536d756c78=5c455b343a25703125646d\x1b\\\
            \x1bP0+r536574756c63\x1b\\\
            \x1b[?62;22c";
        let replies = HostReplies::parse(bytes);
        assert_eq!(replies.version.as_deref(), Some("ghostty 1.1.3"));
        assert_eq!(replies.termcaps, BTreeSet::from(["Smulx".to_owned()]));
        assert!(replies.device_attributes);
        assert!(ends_with_device_attributes(bytes));
    }

    #[test]
    fn known_terminal_gets_everything_from_its_version_reply() {
        let replies = HostReplies::parse(b"\x1bP>|kitty(0.35.2)\x1b\\\x1b[?62;c");
        // The version reply wins over a stale environment.
        let detected = detect_host_capabilities(&replies, &env("screen", Some("tmux")));
        assert_eq!(detected, HostCapabilities::ALL);
    }

    #[test]
    fn termcap_replies_enable_underline_styles_and_colors() {
        let replies = HostReplies::parse(
            b"\x1bP>|XTerm(388)\x1b\\\
              \x1bP1+r536d756c78=00;536574756c63=00\x1b\\\x1b[?64;1c",
        );
        let detected = detect_host_capabilities(&replies, &env("xterm-256color", None));
        assert_eq!(
            detected,
            HostCapabilities {
                underline_styles: true,
                underline_color: true,
                hyperlinks: false,
                passthrough: false,
            }
        );
    }

    #[test]
    fn without_replies_the_environment_names_the_terminal() {
        let none = HostReplies::default();
        assert!(!none.device_attributes);
        assert_eq!(
            detect_host_capabilities(&none, &env("xterm-ghostty", None)),
            HostCapabilities::ALL
        );
        assert_eq!(
            detect_host_capabilities(&none, &env("xterm-256color", Some("WezTerm"))),
            HostCapabilities::ALL
        );
        assert!(
            detect_host_capabilities(&none, &env("xterm-256color", Some("iTerm.app"))).passthrough
        );
    }

    #[test]
    fn unknown_terminal_degrades_to_nothing() {
        // Answers DA1 only, and the environment names nothing known.
        let replies = HostReplies::parse(b"\x1b[?1;2c");
        assert!(replies.device_attributes);
        assert_eq!(
            detect_host_capabilities(&replies, &env("xterm-256color", Some("Apple_Terminal"))),
            HostCapabilities::NONE
        );
        assert_eq!(
            detect_host_capabilities(&HostReplies::default(), &HostEnvironment::default()),
            HostCapabilities::NONE
        );
    }

    #[test]
    fn overrides_win_over_detection() {
        let mut features = TerminalFeatures::default();
        assert!(features.set("hyperlinks", true));
        assert!(features.set("passthrough", false));
        assert!(!features.set("sixel", true));
        let effective = HostCapabilities {
            passthrough: true,
            ..HostCapabilities::NONE
        }
        .with_overrides(features);
        assert_eq!(
            effective,
            HostCapabilities {
                hyperlinks: true,
                ..HostCapabilities::NONE
            }
        );
    }

    #[test]
    fn device_attributes_end_only_on_a_full_reply() {
        assert!(!ends_with_device_attributes(b"\x1b[?62;22"));
        assert!(!ends_with_device_attributes(b"abc"));
        assert!(!ends_with_device_attributes(b"\x1b[?62;x c"));
        assert!(ends_with_device_attributes(b"junk\x1b[?6c"));
    }

    #[test]
    fn recognises_replies_to_the_attach_queries_only() {
        for reply in [
            &b"\x1bP>|ghostty 1.1.3\x1b\\"[..],
            b"\x1bP1+r536d756c78=5c455b343a25703125646d\x1b\\",
            b"\x1bP1+r536d756c78=00;536574756c63=00\x1b\\",
            b"\x1bP0+r536574756c63\x1b\\",
            b"\x1b[?62;22c",
            b"\x1b[?c",
            b"\x1b[?15u",
        ] {
            assert!(
                is_attach_query_reply(reply),
                "{:?}",
                String::from_utf8_lossy(reply)
            );
        }
        for other in [
            // A kitty graphics reply and an OSC colour reply: for a pane.
            &b"\x1b_Gi=31;OK\x1b\\"[..],
            b"\x1b]11;rgb:0000/0000/0000\x1b\\",
            // XTGETTCAP for a capability Embers did not ask about.
            b"\x1bP1+r524742=383a383a38\x1b\\",
            // A key report and a cursor position report.
            b"\x1b[97;5u",
            b"\x1b[3;5R",
        ] {
            assert!(
                !is_attach_query_reply(other),
                "{:?}",
                String::from_utf8_lossy(other)
            );
        }
    }
}
