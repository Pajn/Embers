//! What the host terminal (the one running the client) supports beyond plain
//! xterm sequences, learned when the client attaches.
//!
//! The client asks the terminal who it is (XTVERSION) and whether it has the
//! terminfo capabilities for underline styles and colors (XTGETTCAP), followed
//! by a device attributes query (DA1). Every terminal answers DA1 and answers
//! queries in order, so the DA1 reply marks the end of the replies; terminals
//! that do not know the first two queries just skip them. The answers, or the
//! environment when there are none, decide the [`HostCapabilities`], and the
//! config can override each one (`set_terminal_features`). Replies that come
//! after attaching stopped waiting are recognised while their query is still
//! [pending](PendingAttachQueries).

use std::collections::BTreeSet;
use std::time::Instant;

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
    /// Terminfo capabilities XTGETTCAP answered for, present or not.
    pub answered_termcaps: BTreeSet<String>,
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
            for name in termcap_reply_names(found).into_iter().flatten() {
                self.answered_termcaps.insert(name.clone());
                self.termcaps.insert(name);
            }
        } else if let Some(missing) = body.strip_prefix(b"0+r") {
            self.answered_termcaps
                .extend(termcap_reply_names(missing).into_iter().flatten());
        }
    }
}

/// The capability names in the entries of an XTGETTCAP reply
/// (`<hex name>[=<hex value>];...`), or `None` when one is not valid hex.
fn termcap_reply_names(entries: &[u8]) -> Option<Vec<String>> {
    entries
        .split(|&byte| byte == b';')
        .map(|entry| {
            let name = entry.split(|&byte| byte == b'=').next().unwrap_or_default();
            hex_decode(name).and_then(|name| String::from_utf8(name).ok())
        })
        .collect()
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

/// The terminfo capabilities [`HOST_QUERIES`] asks XTGETTCAP about.
const QUERIED_TERMCAPS: [&str; 2] = ["Smulx", "Setulc"];

/// The [`HOST_QUERIES`] the host has not answered yet, so that their replies
/// are told apart from identical ones to queries a pane sent through
/// passthrough. A reply that answers an outstanding query is the client's and
/// marks it answered; any other reply is a pane's. A query stays outstanding
/// until its reply arrives or a deadline passes, so a host that never answers
/// one does not keep a pane from getting the same reply later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingAttachQueries {
    /// XTVERSION.
    version: bool,
    /// The XTGETTCAP capabilities not answered for yet.
    termcaps: BTreeSet<String>,
    /// DA1.
    device_attributes: bool,
    /// When whatever is still outstanding stops being expected.
    until: Option<Instant>,
}

impl PendingAttachQueries {
    /// What is outstanding once attaching stopped waiting, having read
    /// `replies`: nothing when the DA1 reply came, since it is answered last;
    /// otherwise every query whose reply has not come, until `until`.
    pub fn after(replies: &HostReplies, until: Instant) -> Self {
        if replies.device_attributes {
            return Self::default();
        }
        Self {
            version: replies.version.is_none(),
            termcaps: QUERIED_TERMCAPS
                .iter()
                .filter(|name| !replies.answered_termcaps.contains(**name))
                .map(|name| (*name).to_owned())
                .collect(),
            device_attributes: true,
            until: Some(until),
        }
    }

    /// Whether no reply is expected any more.
    pub fn is_empty(&self) -> bool {
        !self.version && self.termcaps.is_empty() && !self.device_attributes
    }

    /// Whether `reply`, one control sequence from the host terminal, answers
    /// an outstanding query at `now`, which it then marks answered. A DA1
    /// reply answers every query, as the host answers them in order.
    pub fn take_reply(&mut self, reply: &[u8], now: Instant) -> bool {
        if self.until.is_some_and(|until| now >= until) {
            *self = Self::default();
        }
        if self.is_empty() {
            return false;
        }
        if let Some(body) = reply
            .strip_prefix(b"\x1bP")
            .and_then(|body| body.strip_suffix(b"\x1b\\"))
        {
            if body.starts_with(b">|") {
                return std::mem::take(&mut self.version);
            }
            let Some(names) = body
                .strip_prefix(b"1+r")
                .or_else(|| body.strip_prefix(b"0+r"))
                .and_then(termcap_reply_names)
            else {
                return false;
            };
            if !names.iter().all(|name| self.termcaps.contains(name)) {
                return false;
            }
            for name in &names {
                self.termcaps.remove(name);
            }
            return true;
        }
        let is_device_attributes = reply
            .strip_prefix(b"\x1b[?")
            .and_then(|body| body.strip_suffix(b"c"))
            .is_some_and(|params| {
                params
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || *byte == b';')
            });
        if is_device_attributes && self.device_attributes {
            *self = Self::default();
            return true;
        }
        false
    }
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
    use std::time::Duration;

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
        assert_eq!(
            replies.answered_termcaps,
            BTreeSet::from(["Setulc".to_owned(), "Smulx".to_owned()])
        );
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

    const VERSION_REPLY: &[u8] = b"\x1bP>|ghostty 1.1.3\x1b\\";
    const SMULX_REPLY: &[u8] = b"\x1bP1+r536d756c78=5c455b343a25703125646d\x1b\\";
    const SETULC_REPLY: &[u8] = b"\x1bP0+r536574756c63\x1b\\";
    const DA1_REPLY: &[u8] = b"\x1b[?62;22c";

    #[test]
    fn nothing_is_pending_once_the_device_attributes_reply_came() {
        let now = Instant::now();
        let replies = HostReplies::parse(b"\x1bP>|ghostty 1.1.3\x1b\\\x1b[?62;22c");
        let mut pending = PendingAttachQueries::after(&replies, now + Duration::from_secs(10));
        assert!(pending.is_empty());
        for reply in [VERSION_REPLY, SMULX_REPLY, SETULC_REPLY, DA1_REPLY] {
            assert!(!pending.take_reply(reply, now));
        }
    }

    #[test]
    fn each_late_reply_answers_its_pending_query_once() {
        let now = Instant::now();
        // Attach read the version reply, then stopped waiting.
        let mut pending = PendingAttachQueries::after(
            &HostReplies::parse(VERSION_REPLY),
            now + Duration::from_secs(10),
        );
        assert!(!pending.take_reply(VERSION_REPLY, now));

        assert!(pending.take_reply(SMULX_REPLY, now));
        assert!(!pending.take_reply(SMULX_REPLY, now));
        // A capability Embers did not ask about, alone or alongside one it did.
        assert!(!pending.take_reply(b"\x1bP1+r524742=383a383a38\x1b\\", now));
        assert!(!pending.take_reply(b"\x1bP0+r536574756c63;524742\x1b\\", now));
        // Not replies to the attach queries at all: a kitty graphics reply,
        // an OSC colour reply, kitty keyboard flags, a cursor position report.
        for other in [
            &b"\x1b_Gi=31;OK\x1b\\"[..],
            b"\x1b]11;rgb:0000/0000/0000\x1b\\",
            b"\x1b[?1u",
            b"\x1b[3;5R",
        ] {
            assert!(!pending.take_reply(other, now));
        }
        assert!(pending.take_reply(SETULC_REPLY, now));
        assert!(pending.take_reply(DA1_REPLY, now));
        assert!(pending.is_empty());
        assert!(!pending.take_reply(DA1_REPLY, now));
    }

    #[test]
    fn device_attributes_answer_every_query_before_them() {
        let now = Instant::now();
        let mut pending =
            PendingAttachQueries::after(&HostReplies::default(), now + Duration::from_secs(10));
        // A host that does not know XTVERSION or XTGETTCAP answers DA1 only.
        assert!(pending.take_reply(b"\x1b[?c", now));
        assert!(!pending.take_reply(VERSION_REPLY, now));
        assert!(!pending.take_reply(SMULX_REPLY, now));
    }

    #[test]
    fn pending_queries_lapse_at_their_deadline() {
        let now = Instant::now();
        let until = now + Duration::from_secs(10);
        let mut pending = PendingAttachQueries::after(&HostReplies::default(), until);
        assert!(pending.take_reply(VERSION_REPLY, until - Duration::from_millis(1)));
        assert!(!pending.take_reply(DA1_REPLY, until));
        assert!(pending.is_empty());
    }
}
