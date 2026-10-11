//! Window numbering, tmux `base-index` style.
//!
//! Windows are a session's root tabs, stored in order, so their numbers are
//! always contiguous: closing one renumbers the windows after it, as tmux does
//! with `renumber-windows on`. The base index only chooses the number of the
//! first window.
//!
//! When no base index is configured, window numbers shown to users start at 0
//! and a numeric CLI target is accepted as either 0- or 1-based, rejecting a
//! number that could mean two windows. Setting a base index makes every
//! number mean exactly one window.

/// Environment variable setting the window base index for the CLI, over the
/// one the server has for the session, and for the interactive client unless
/// its config sets one.
pub const BASE_INDEX_ENV_VAR: &str = "EMBERS_BASE_INDEX";

/// Largest accepted base index. tmux allows any non-negative number; this keeps
/// window numbers comfortably inside the protocol's `u32` tab index.
pub const MAX_BASE_INDEX: u32 = 1_000_000;

/// Parse a base index setting, accepting `0..=MAX_BASE_INDEX`.
pub fn parse_base_index(value: &str) -> Option<u32> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value <= MAX_BASE_INDEX)
}

/// The base index from [`BASE_INDEX_ENV_VAR`]. Unset, empty or unparseable
/// values yield `None`, so callers keep their default.
pub fn base_index_from_env() -> Option<u32> {
    std::env::var(BASE_INDEX_ENV_VAR)
        .ok()
        .and_then(|value| parse_base_index(&value))
}

/// The number shown for the window at `position` (0-based).
pub fn window_number(position: usize, base_index: Option<u32>) -> u64 {
    position as u64 + u64::from(base_index.unwrap_or(0))
}

/// The position (0-based) of window `number` among `count` windows, or `None`
/// when no window has that number.
pub fn window_position(number: u64, count: usize, base_index: u32) -> Option<usize> {
    let position = usize::try_from(number.checked_sub(u64::from(base_index))?).ok()?;
    (position < count).then_some(position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_base_index_accepts_small_non_negative_numbers() {
        assert_eq!(parse_base_index("0"), Some(0));
        assert_eq!(parse_base_index(" 1 "), Some(1));
        assert_eq!(parse_base_index("-1"), None);
        assert_eq!(parse_base_index(""), None);
        assert_eq!(parse_base_index("one"), None);
        assert_eq!(parse_base_index("1000001"), None);
    }

    #[test]
    fn window_numbers_start_at_the_base_index() {
        assert_eq!(window_number(0, None), 0);
        assert_eq!(window_number(2, Some(1)), 3);
        assert_eq!(window_position(1, 3, 1), Some(0));
        assert_eq!(window_position(3, 3, 1), Some(2));
        assert_eq!(window_position(4, 3, 1), None);
        assert_eq!(window_position(0, 3, 1), None);
        assert_eq!(window_position(0, 3, 0), Some(0));
    }
}
