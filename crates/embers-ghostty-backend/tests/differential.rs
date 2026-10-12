//! The Ghostty backend against the alacritty one, to find where the two
//! emulators differ: random output, then the first cell, cursor or mode they
//! show differently.
//!
//! Run on demand (`cargo test -- --ignored`), since the emulators do differ.
//! The steps below steer around some of the differences found so far. Those
//! they cannot avoid:
//!
//! - Overwriting half of a wide character, alacritty fills the half left over
//!   with the pen; Ghostty blanks it, in the current background when it was
//!   the right half that was overwritten.
//! - Erasing from the right half of a wide character, Ghostty erases the
//!   whole character in the erase's background; alacritty erases from the
//!   right half on.
//! - Entering the alternate screen while on it, Ghostty clears it again;
//!   alacritty does nothing.
//! - DECALN resets the scroll region in Ghostty, as the VT100 does.
//!
//! Those the steps avoid:
//!
//! - Insert and delete line move the cursor to the first column in Ghostty,
//!   as in xterm; alacritty leaves it.
//! - A tab with the cursor waiting at the last column wraps in alacritty and
//!   not in Ghostty, as in xterm.
//! - A line feed with the cursor waiting at the last column keeps the wrap
//!   waiting in alacritty, so the next character wraps again; Ghostty drops
//!   it, as xterm does.
//! - DECALN homes the cursor and resets the pen in Ghostty, as the VT100
//!   does, and fills in the current foreground; alacritty does none of it.
//! - Entering the alternate screen, alacritty clears it in the current
//!   background and Ghostty in the default one; leaving it without having
//!   entered, Ghostty still restores the saved cursor and pen.

use embers_core::{CellAttrs, CursorState, PtySize, SnapshotLine, TermColor, TerminalModes};
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{AlacrittyTerminalBackend, TerminalBackend};
use proptest::prelude::*;
use unicode_segmentation::UnicodeSegmentation;

const COLUMNS: u16 = 16;
const ROWS: u16 = 5;

/// One cell as the user sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    text: String,
    fg: TermColor,
    bg: TermColor,
    attrs: CellAttrs,
    underline_color: TermColor,
    link: Option<String>,
}

/// What a backend shows after `bytes`: its snapshot's rows as cells, and its
/// cursor and modes.
fn screen(
    backend: &mut dyn TerminalBackend,
    bytes: &[u8],
) -> (Vec<Vec<Shown>>, Option<CursorState>, TerminalModes) {
    backend.ingest_bytes(bytes);
    let snapshot = backend.visible_snapshot(0, PtySize::new(COLUMNS, ROWS), None);
    let rows = snapshot.lines.iter().map(cells).collect();
    (rows, snapshot.cursor, snapshot.modes)
}

/// A snapshot line's cells, each normalized to how it looks: on a cell with
/// no glyph, attributes that only change a glyph show nothing, and nor does
/// the foreground unless a line or inverse video draws it. Trailing cells that
/// show nothing, linked or not, are left out; alacritty keeps a tab in the
/// cell a tab starts in, and with it the blanks before.
fn cells(line: &SnapshotLine) -> Vec<Shown> {
    let mut cells = Vec::new();
    let mut offset = 0;
    let mut runs = line.runs.iter();
    let mut run = runs.next();
    let mut run_end = run.map_or(usize::MAX, |run| run.len as usize);
    for grapheme in line.text.graphemes(true) {
        while offset >= run_end {
            run = runs.next();
            run_end += run.map_or(usize::MAX, |run| run.len as usize);
        }
        offset += grapheme.len();
        let mut cell = Shown {
            text: grapheme.to_owned(),
            fg: run.map_or(TermColor::Default, |run| run.fg),
            bg: run.map_or(TermColor::Default, |run| run.bg),
            attrs: run.map_or(CellAttrs::empty(), |run| run.attrs),
            underline_color: run.map_or(TermColor::Default, |run| run.underline_color),
            link: run
                .and_then(|run| usize::from(run.link).checked_sub(1))
                .and_then(|index| line.links.get(index))
                .map(|link| link.uri.clone()),
        };
        if cell.text == " " {
            cell.attrs.0 &=
                !(CellAttrs::BOLD | CellAttrs::DIM | CellAttrs::ITALIC | CellAttrs::HIDDEN);
            let lines = cell.attrs.contains(CellAttrs::ANY_UNDERLINE)
                || cell.attrs.contains(CellAttrs::STRIKEOUT);
            if !lines && !cell.attrs.contains(CellAttrs::INVERSE) {
                cell.fg = TermColor::Default;
            }
            if !cell.attrs.contains(CellAttrs::ANY_UNDERLINE) {
                cell.underline_color = TermColor::Default;
            }
        }
        cells.push(cell);
    }
    while cells.last().is_some_and(|cell| {
        cell.text == " " && cell.bg == TermColor::Default && cell.attrs == CellAttrs::empty()
    }) {
        cells.pop();
    }
    cells
}

/// Where the two backends first differ after `bytes`, if they do.
fn difference(bytes: &[u8]) -> Option<String> {
    let size = PtySize::new(COLUMNS, ROWS);
    let alacritty = screen(&mut AlacrittyTerminalBackend::new(size, 100), bytes);
    let ghostty = screen(&mut GhosttyTerminalBackend::new(size, 100), bytes);
    let input = String::from_utf8_lossy(bytes);
    for (row, (a, g)) in alacritty.0.iter().zip(&ghostty.0).enumerate() {
        if a != g {
            let column = a.iter().zip(g).take_while(|(a, g)| a == g).count();
            return Some(format!(
                "{input:?}: row {row}, cell {column}\n alacritty: {:?}\n ghostty:   {:?}",
                a.get(column),
                g.get(column)
            ));
        }
    }
    if (alacritty.1, alacritty.2) != (ghostty.1, ghostty.2) {
        return Some(format!(
            "{input:?}: cursor or modes\n alacritty: {:?} {:?}\n ghostty:   {:?} {:?}",
            alacritty.1, alacritty.2, ghostty.1, ghostty.2
        ));
    }
    None
}

/// One step of what a program might write.
fn step() -> impl Strategy<Value = Vec<u8>> {
    // No tabs, and no line feed without a carriage return, which ends a
    // waiting wrap in both.
    let text = prop::collection::vec(
        prop::sample::select(vec![
            "a", "Z", " ", "~", "漢", "e\u{301}", "👍", "\r", "\r\n",
        ]),
        1..8,
    )
    .prop_map(|parts| parts.concat().into_bytes());
    let sgr = prop::sample::select(vec![
        "0",
        "1",
        "2",
        "3",
        "4",
        "4:2",
        "4:3",
        "4:4",
        "4:5",
        "7",
        "8",
        "9",
        "22",
        "24",
        "27",
        "31",
        "42",
        "95",
        "106",
        "38;5;123",
        "48;5;7",
        "38;2;10;20;30",
        "48;2;200;0;100",
        "58:2::1:2:3",
        "58:5:9",
        "39",
        "49",
        "59",
    ])
    .prop_map(|sgr| format!("\x1b[{sgr}m").into_bytes());
    let cursor = (1..=ROWS, 1..=COLUMNS)
        .prop_map(|(row, column)| format!("\x1b[{row};{column}H").into_bytes());
    let edit = prop::sample::select(vec![
        "\x1b[K",
        "\x1b[1K",
        "\x1b[2K",
        "\x1b[J",
        "\x1b[@",
        "\x1b[2@",
        "\x1b[P",
        "\x1b[2P",
        "\x1b[X",
        "\x1b[2X",
        "\x1b[L\r",
        "\x1b[M\r",
        "\x1b[S",
        "\x1b[T",
        "\x1b[2;4r",
        "\x1b[r",
        "\x1bM",
        "\x1b[0m\x1b#8\x1b[H",
        "\x1b]8;;https://a.example\x1b\\",
        "\x1b]8;;\x1b\\",
        "\x1b[?25l",
        "\x1b[?25h",
        "\x1b[5 q",
        "\x1b[3 q",
        "\x1b[?2004h",
        "\x1b[?1004h",
        "\x1b[?1000h",
        "\x1b[0m\x1b[?1049h",
        "\x1b[?1049l\x1b[H\x1b[0m",
    ])
    .prop_map(|edit| edit.as_bytes().to_vec());
    prop_oneof![3 => text, 2 => sgr, 1 => cursor, 1 => edit]
}

proptest! {
    #[test]
    #[ignore = "the emulators differ; run to find where"]
    fn any_output_snapshots_the_same(steps in prop::collection::vec(step(), 1..24)) {
        let bytes = steps.concat();
        if let Some(difference) = difference(&bytes) {
            return Err(TestCaseError::fail(difference));
        }
    }
}
