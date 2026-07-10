//! Thumbs-style hint scanning: find interesting spans (URLs, paths, hashes, …)
//! in the visible view and assign home-row labels for jump-to selection.

use embers_core::SnapshotLine;
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use crate::state::HintMatch;

/// Home-row-first label alphabet, matching tmux-thumbs' default ordering.
const HINT_ALPHABET: &str = "asdfghjkl;qwertyuiopzxcvbnm";

/// Default hint patterns: URLs, absolute/`~` paths, relative paths with at least
/// one slash, git SHAs, UUIDs, IPv4 addresses, and long numbers. Overlapping
/// matches are resolved by earliest start, then longest length; the order of the
/// patterns below only breaks exact ties that share both start and end offsets.
pub const DEFAULT_HINT_PATTERNS: &[&str] = &[
    // URLs.
    r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s()<>\[\]{}'\x22]+",
    // Absolute and ~-relative paths.
    r"~?/[a-zA-Z0-9._~@%/+-]+",
    // Relative paths containing at least one slash.
    r"[a-zA-Z0-9._-]+/[a-zA-Z0-9._~@%/+-]+",
    // UUIDs.
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
    // Git SHAs (7-40 hex).
    r"\b[0-9a-f]{7,40}\b",
    // IPv4 addresses.
    r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
    // Numbers with at least four digits.
    r"\b\d{4,}\b",
];

/// Compile pattern sources into regexes, skipping any that fail to compile.
pub fn compile_patterns(sources: &[String]) -> Vec<Regex> {
    sources
        .iter()
        .filter_map(|source| Regex::new(source).ok())
        .collect()
}

/// Compile the built-in default patterns.
pub fn default_patterns() -> Vec<Regex> {
    DEFAULT_HINT_PATTERNS
        .iter()
        .filter_map(|source| Regex::new(source).ok())
        .collect()
}

/// Assign `count` prefix-free labels: single home-row characters while they
/// suffice, otherwise two-character pairs.
pub fn assign_labels(count: usize) -> Vec<String> {
    let alphabet: Vec<char> = HINT_ALPHABET.chars().collect();
    let n = alphabet.len();
    // Two-character pairs cap the label space at n*n; never allocate or index
    // beyond that even when more matches are requested.
    let mut labels = Vec::with_capacity(count.min(n * n));
    if count <= n {
        for ch in alphabet.iter().take(count) {
            labels.push(ch.to_string());
        }
    } else {
        'outer: for first in &alphabet {
            for second in &alphabet {
                labels.push(format!("{first}{second}"));
                if labels.len() == count {
                    break 'outer;
                }
            }
        }
    }
    labels
}

/// Scan the visible `lines` for pattern matches and return labelled hints.
///
/// `top_line` is the absolute line number of the first visible line. Overlapping
/// matches keep the earliest start. Labels are assigned last-match-first (reverse
/// mode) so the bottom-most hints get the shortest labels.
pub fn scan(lines: &[SnapshotLine], patterns: &[Regex], top_line: u64) -> Vec<HintMatch> {
    let mut spans: Vec<(u64, u16, u16, String)> = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        let absolute_line = top_line.saturating_add(row as u64);
        let text = &line.text;
        let mut byte_ranges: Vec<(usize, usize)> = Vec::new();
        for pattern in patterns {
            for found in pattern.find_iter(text) {
                byte_ranges.push((found.start(), found.end()));
            }
        }
        // Earliest start first; drop overlaps against already-kept ranges.
        byte_ranges.sort_by_key(|(start, end)| (*start, std::cmp::Reverse(*end)));
        let mut kept: Vec<(usize, usize)> = Vec::new();
        for (start, end) in byte_ranges {
            if kept.last().is_some_and(|(_, prev_end)| start < *prev_end) {
                continue;
            }
            kept.push((start, end));
        }
        for (start, end) in kept {
            let start_col = UnicodeWidthStr::width(&text[..start]);
            let end_col = start_col + UnicodeWidthStr::width(&text[start..end]);
            spans.push((
                absolute_line,
                start_col.min(u16::MAX as usize) as u16,
                end_col.min(u16::MAX as usize) as u16,
                text[start..end].to_owned(),
            ));
        }
    }

    let labels = assign_labels(spans.len());
    // The label alphabet caps at n*n; if matches exceed it, only the last
    // `labels.len()` (bottom-most, reverse mode) get a hint — the rest are
    // dropped rather than indexing past the label set.
    let label_count = labels.len();
    let skip = spans.len().saturating_sub(label_count);
    spans
        .into_iter()
        .skip(skip)
        .enumerate()
        .map(|(index, (line, start_col, end_col, text))| HintMatch {
            // Reverse: the last span gets labels[0].
            label: labels[label_count - 1 - index].clone(),
            text,
            line,
            start_col,
            end_col,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use embers_core::SnapshotLine;

    fn line(text: &str) -> SnapshotLine {
        SnapshotLine::plain(text)
    }

    #[test]
    fn single_char_labels_until_alphabet_exhausted() {
        assert_eq!(assign_labels(0), Vec::<String>::new());
        assert_eq!(assign_labels(3), vec!["a", "s", "d"]);
        let n = HINT_ALPHABET.chars().count();
        assert_eq!(assign_labels(n).len(), n);
        assert!(assign_labels(n).iter().all(|label| label.len() == 1));
    }

    #[test]
    fn two_char_labels_when_over_alphabet() {
        let n = HINT_ALPHABET.chars().count();
        let labels = assign_labels(n + 1);
        assert_eq!(labels.len(), n + 1);
        assert!(labels.iter().all(|label| label.chars().count() == 2));
        assert_eq!(labels[0], "aa");
        assert_eq!(labels[1], "as");
    }

    #[test]
    fn labels_are_capped_at_two_char_capacity() {
        let n = HINT_ALPHABET.chars().count();
        // Beyond the n*n two-char capacity, the label set is capped (never
        // grows), so scan's reverse indexing can't run past it.
        assert_eq!(assign_labels(n * n).len(), n * n);
        assert_eq!(assign_labels(n * n + 1).len(), n * n);
        assert_eq!(assign_labels(n * n + 1000).len(), n * n);
    }

    #[test]
    fn scan_does_not_panic_with_more_matches_than_labels() {
        let n = HINT_ALPHABET.chars().count();
        // One number per "word"; more matches than the n*n label capacity.
        let count = n * n + 5;
        let text = (0..count)
            .map(|index| format!("{}", 1000 + index))
            .collect::<Vec<_>>()
            .join(" ");
        let hints = scan(&[SnapshotLine::plain(&text)], &default_patterns(), 0);
        // Capped at capacity; every hint carries a label.
        assert_eq!(hints.len(), n * n);
        assert!(hints.iter().all(|hint| !hint.label.is_empty()));
    }

    #[test]
    fn scans_default_patterns() {
        let patterns = default_patterns();
        let lines = vec![
            line("visit https://example.com/x for docs"),
            line("edit /tmp/config.rhai now"),
            line("sha 1a2b3c4d5e"),
        ];
        let hints = scan(&lines, &patterns, 0);
        let texts: Vec<&str> = hints.iter().map(|hint| hint.text.as_str()).collect();
        assert!(texts.contains(&"https://example.com/x"));
        assert!(texts.contains(&"/tmp/config.rhai"));
        assert!(texts.contains(&"1a2b3c4d5e"));
    }

    #[test]
    fn labels_assigned_last_match_first() {
        let patterns = default_patterns();
        let lines = vec![line("/a/one /b/two")];
        let hints = scan(&lines, &patterns, 0);
        assert_eq!(hints.len(), 2);
        // The bottom-most / right-most match gets the first (shortest) label.
        assert_eq!(hints[1].label, "a");
        assert_eq!(hints[0].label, "s");
    }

    #[test]
    fn overlapping_matches_keep_earliest() {
        let patterns = default_patterns();
        // A path that could also match the number pattern inside; only one span
        // should be kept for the overlapping region.
        let lines = vec![line("/var/log/12345")];
        let hints = scan(&lines, &patterns, 0);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].text, "/var/log/12345");
    }
}
