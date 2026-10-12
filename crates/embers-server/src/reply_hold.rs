//! Keeping a pane's query replies in order behind its kitty graphics queries.
//!
//! Programs find out whether the terminal shows kitty graphics by sending a
//! graphics query (`ESC _ G i=<id>,a=q,... ESC \`) followed by a query every
//! terminal answers, usually DA1 (`CSI c`) or a status report (`CSI 5 n`), and
//! reading replies until that one arrives. Terminals answer in order, so one
//! that knows the protocol has answered the graphics query by then. ratatui-image,
//! yazi, chafa and timg all work this way.
//!
//! In Embers the graphics query goes to the host terminal through passthrough,
//! a round trip through the server and a client, while the pane's emulator
//! answers the queries after it at once. Without help the program reads DA1
//! long before the host's answer and decides there is no graphics support.
//! [`ReplyHold`] keeps the emulator's replies that follow a forwarded graphics
//! query and lets them go, in order, right after the host's answer has been
//! written to the pane, or after [`REPLY_HOLD_TIMEOUT`] when none comes.
//!
//! The keeper only holds replies while the server says some client forwards
//! graphics queries to a host terminal that answers them. Otherwise nobody
//! will answer, and the program gets the emulator's replies at once, which
//! correctly tell it there is no graphics support.

use std::time::{Duration, Instant};

/// How long replies wait for the host terminal's answer to a graphics query.
/// The answer normally takes one status poll (50 ms) and a client round trip;
/// this only runs out when no client forwards the query, for example because
/// the pane is not on screen, or the host does not answer after all.
pub(crate) const REPLY_HOLD_TIMEOUT: Duration = Duration::from_millis(500);

/// Most reply bytes held at once. Real replies to a detection burst are a few
/// dozen bytes; a program that keeps asking past this gets every held reply
/// at once instead of waiting.
const MAX_HELD_REPLY_BYTES: usize = 64 * 1024;

/// Replies to a pane's queries, held while forwarded kitty graphics queries
/// wait for the host terminal's answer.
#[derive(Debug, Default)]
pub(crate) struct ReplyHold {
    /// Whether some client forwards graphics queries to a host terminal that
    /// answers them. Nothing is held without one.
    enabled: bool,
    /// The forwarded queries whose answer has not reached the pane yet.
    pending: Vec<GraphicsId>,
    /// When the replies go out even without an answer.
    deadline: Option<Instant>,
    held: Vec<u8>,
}

/// How a graphics command names its image, which its reply repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphicsId {
    /// `i=<id>`.
    Image(u32),
    /// `I=<number>`.
    Number(u32),
}

impl ReplyHold {
    /// Record whether some client forwards graphics queries to a host that
    /// answers them. Turning it off returns whatever was held, to be written
    /// to the pane now.
    pub(crate) fn set_enabled(&mut self, enabled: bool) -> Vec<u8> {
        self.enabled = enabled;
        if enabled {
            return Vec::new();
        }
        self.release()
    }

    /// When the held replies go out if no answer comes, `None` while nothing
    /// is held back.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// `write`, sequences for the host terminal, was taken out of the pane's
    /// output at `now`. Every graphics query in it that the host will answer
    /// holds back the replies to what the program sends after it. Replies to
    /// what came before must already have gone through
    /// [`ReplyHold::replies`].
    pub(crate) fn forwarded(&mut self, write: &[u8], now: Instant) {
        if !self.enabled {
            return;
        }
        let queries = graphics_commands(write)
            .filter(|control| is_answered_query(control))
            .filter_map(|control| graphics_ids(control).next());
        let before = self.pending.len();
        self.pending.extend(queries);
        if self.pending.len() > before {
            self.deadline = Some(now + REPLY_HOLD_TIMEOUT);
        }
    }

    /// The emulator's `replies`, in order: what to write to the pane now. They
    /// are held while a forwarded query waits for its answer.
    pub(crate) fn replies(&mut self, replies: Vec<u8>) -> Vec<u8> {
        if self.pending.is_empty() {
            return replies;
        }
        self.held.extend_from_slice(&replies);
        if self.held.len() > MAX_HELD_REPLY_BYTES {
            return self.release();
        }
        Vec::new()
    }

    /// `input` is being written to the pane. When it holds the answers to
    /// every pending query, returns the held replies, to be written right
    /// after it.
    pub(crate) fn input(&mut self, input: &[u8]) -> Vec<u8> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        for id in graphics_commands(input).flat_map(graphics_ids) {
            if let Some(index) = self.pending.iter().position(|pending| *pending == id) {
                self.pending.remove(index);
            }
        }
        if self.pending.is_empty() {
            return self.release();
        }
        Vec::new()
    }

    /// Give up waiting once the deadline has passed at `now`: returns the
    /// held replies, to be written now.
    pub(crate) fn expire(&mut self, now: Instant) -> Vec<u8> {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return self.release();
        }
        Vec::new()
    }

    fn release(&mut self) -> Vec<u8> {
        self.pending.clear();
        self.deadline = None;
        std::mem::take(&mut self.held)
    }
}

/// The control data (what comes before `;`) of every complete kitty graphics
/// command (`ESC _ G <control> [; <payload>] ESC \`) in `bytes`.
fn graphics_commands(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = bytes;
    std::iter::from_fn(move || {
        let start = find(rest, b"\x1b_G")? + 3;
        let length = find(&rest[start..], b"\x1b\\")?;
        let command = &rest[start..start + length];
        rest = &rest[start + length + 2..];
        Some(
            command
                .split(|&byte| byte == b';')
                .next()
                .unwrap_or(command),
        )
    })
}

/// The `key=value` pairs of a graphics command's control data.
fn control_keys(control: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    control.split(|&byte| byte == b',').filter_map(|pair| {
        let separator = pair.iter().position(|&byte| byte == b'=')?;
        Some((&pair[..separator], &pair[separator + 1..]))
    })
}

/// Whether `control` is a query (`a=q`) the terminal answers: it names the
/// image, and does not ask to stay quiet (`q=1` drops the `OK`, `q=2`
/// everything).
fn is_answered_query(control: &[u8]) -> bool {
    let mut query = false;
    for (key, value) in control_keys(control) {
        match key {
            b"a" => query = value == b"q",
            b"q" if value != b"0" => return false,
            _ => {}
        }
    }
    query
}

/// The names in a graphics command's control data. A query names its image
/// one way; the terminal's answer to an `I=` query carries both, the id the
/// terminal chose and the number the query used.
fn graphics_ids(control: &[u8]) -> impl Iterator<Item = GraphicsId> {
    let parse = |value: &[u8]| std::str::from_utf8(value).ok()?.parse::<u32>().ok();
    control_keys(control).filter_map(move |(key, value)| match key {
        b"i" => parse(value).map(GraphicsId::Image),
        b"I" => parse(value).map(GraphicsId::Number),
        _ => None,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The query ratatui-image sends first.
    const QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";
    const ANSWER: &[u8] = b"\x1b_Gi=31;OK\x1b\\";

    fn enabled() -> ReplyHold {
        let mut hold = ReplyHold::default();
        assert!(hold.set_enabled(true).is_empty());
        hold
    }

    #[test]
    fn replies_after_a_query_wait_for_its_answer() {
        let mut hold = enabled();
        let now = Instant::now();
        assert_eq!(hold.replies(b"before".to_vec()), b"before");

        hold.forwarded(QUERY, now);
        assert_eq!(hold.deadline(), Some(now + REPLY_HOLD_TIMEOUT));
        assert!(hold.replies(b"\x1b[?6c".to_vec()).is_empty());
        assert!(hold.replies(b"\x1b[0n".to_vec()).is_empty());
        // Other input, and an answer for another image, do not release them.
        assert!(hold.input(b"typed").is_empty());
        assert!(hold.input(b"\x1b_Gi=7;OK\x1b\\").is_empty());

        assert_eq!(hold.input(ANSWER), b"\x1b[?6c\x1b[0n");
        assert_eq!(hold.deadline(), None);
        assert_eq!(hold.replies(b"after".to_vec()), b"after");
    }

    #[test]
    fn every_pending_query_needs_its_answer() {
        let mut hold = enabled();
        // ratatui-image's compression probe follows the plain one, here
        // wrapped together in one tmux passthrough.
        let mut write = QUERY.to_vec();
        write.extend_from_slice(b"\x1b_Gi=32,s=1,v=1,a=q,t=d,f=24,o=z;eJxj\x1b\\");
        hold.forwarded(&write, Instant::now());
        assert!(hold.replies(b"\x1b[0n".to_vec()).is_empty());

        assert!(hold.input(ANSWER).is_empty());
        assert_eq!(
            hold.input(b"\x1b_Gi=32;EINVAL:unsupported\x1b\\"),
            b"\x1b[0n"
        );
    }

    #[test]
    fn replies_go_out_at_the_deadline_without_an_answer() {
        let mut hold = enabled();
        let now = Instant::now();
        hold.forwarded(QUERY, now);
        assert!(hold.replies(b"\x1b[0n".to_vec()).is_empty());

        assert!(
            hold.expire(now + REPLY_HOLD_TIMEOUT - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(hold.expire(now + REPLY_HOLD_TIMEOUT), b"\x1b[0n");
        assert_eq!(hold.deadline(), None);
        // A late answer is just input.
        assert!(hold.input(ANSWER).is_empty());
    }

    #[test]
    fn nothing_is_held_unless_enabled() {
        let mut hold = ReplyHold::default();
        hold.forwarded(QUERY, Instant::now());
        assert_eq!(hold.deadline(), None);
        assert_eq!(hold.replies(b"\x1b[0n".to_vec()), b"\x1b[0n");
    }

    #[test]
    fn disabling_releases_what_was_held() {
        let mut hold = enabled();
        hold.forwarded(QUERY, Instant::now());
        assert!(hold.replies(b"\x1b[0n".to_vec()).is_empty());
        assert_eq!(hold.set_enabled(false), b"\x1b[0n");
        assert_eq!(hold.deadline(), None);
    }

    #[test]
    fn only_queries_the_host_answers_hold_replies() {
        let mut hold = enabled();
        for write in [
            // An image, not a query.
            &b"\x1b_Gi=31,a=T,f=100;iVBORw0KGgo=\x1b\\"[..],
            // A query without an id gets no answer.
            b"\x1b_Ga=q,s=1,v=1;AAAA\x1b\\",
            // Nor does one that asks to stay quiet.
            b"\x1b_Gi=31,a=q,q=2;AAAA\x1b\\",
            b"\x1b_Gi=31,a=q,q=1;AAAA\x1b\\",
            // An unterminated command is no command.
            b"\x1b_Gi=31,a=q;AAAA",
        ] {
            hold.forwarded(write, Instant::now());
            assert_eq!(hold.deadline(), None, "held for {write:?}");
        }
        // Image numbers work like ids.
        hold.forwarded(b"\x1b_GI=5,a=q;AAAA\x1b\\", Instant::now());
        assert!(hold.replies(b"\x1b[0n".to_vec()).is_empty());
        assert_eq!(hold.input(b"\x1b_Gi=99,I=5;OK\x1b\\"), b"\x1b[0n");
    }

    #[test]
    fn a_flood_of_replies_is_not_held() {
        let mut hold = enabled();
        hold.forwarded(QUERY, Instant::now());
        let flood = vec![b'x'; MAX_HELD_REPLY_BYTES + 1];
        assert_eq!(hold.replies(flood.clone()), flood);
        assert_eq!(hold.deadline(), None);
    }
}
