//! What a program in a pane writes must look the same on the host terminal
//! after Embers has drawn it as it would if written straight to a terminal.
//!
//! Both sides are read through `alacritty_terminal`, the emulator panes run
//! on: one parses the program's bytes directly, the other parses the frame
//! Embers draws from the pane's snapshot. Gaps in the emulation are the same
//! on both sides and cancel out, so a difference is Embers' own: the
//! snapshot, the client grid or the frame encoding.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{self, Color, NamedColor, Rgb};
use embers_client::{HostCapabilities, RenderGrid};
use embers_core::{PtySize, Size};
use embers_server::{AlacrittyTerminalBackend, TerminalBackend};
use proptest::prelude::*;

use super::frame_bytes;

const COLUMNS: u16 = 16;
const ROWS: u16 = 5;

/// Flags that change how a cell looks. Hidden is left out: Embers draws a
/// hidden cell as a blank instead of passing the attribute on.
const VISIBLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::STRIKEOUT)
    .union(Flags::WIDE_CHAR);

/// One cell as the user sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    text: String,
    fg: Shade,
    bg: Shade,
    underline_color: Option<Shade>,
    flags: Flags,
    link: Option<String>,
}

/// A color as the host resolves it. Embers sends the 16 named colors as
/// their palette indices, which the host paints the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shade {
    Default,
    Indexed(u8),
    Rgb(Rgb),
}

impl Shown {
    fn is_blank(&self) -> bool {
        self.text == " "
            && self.bg == Shade::Default
            && !self
                .flags
                .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
    }
}

fn shade(color: Color) -> Shade {
    match color {
        Color::Spec(rgb) => Shade::Rgb(rgb),
        Color::Indexed(index) => Shade::Indexed(index),
        Color::Named(named) => match named {
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => Shade::Default,
            NamedColor::DimBlack => Shade::Indexed(0),
            NamedColor::DimRed => Shade::Indexed(1),
            NamedColor::DimGreen => Shade::Indexed(2),
            NamedColor::DimYellow => Shade::Indexed(3),
            NamedColor::DimBlue => Shade::Indexed(4),
            NamedColor::DimMagenta => Shade::Indexed(5),
            NamedColor::DimCyan => Shade::Indexed(6),
            NamedColor::DimWhite => Shade::Indexed(7),
            named => Shade::Indexed(named as u8),
        },
    }
}

fn terminal() -> (Term<VoidListener>, ansi::Processor) {
    let size = TermSize::new(usize::from(COLUMNS), usize::from(ROWS));
    (
        Term::new(Config::default(), &size, VoidListener),
        ansi::Processor::new(),
    )
}

/// The screen as the user sees it: each row's cells up to the last one that
/// shows anything.
///
/// A hidden glyph shows as a blank in each column it covers. Half a wide
/// character whose other half was overwritten or erased shows as a plain
/// blank: the emulator keeps that half, with its style, but a terminal shows
/// neither half, and Ghostty clears the half that is left to a blank cell.
fn screen(term: &Term<VoidListener>) -> Vec<Vec<Shown>> {
    let grid = term.grid();
    let columns = grid.columns();
    (0..grid.screen_lines())
        .map(|line| {
            let row = &grid[Line(line as i32)];
            let flags = |column: usize| row[Column(column)].flags;
            let mut cells = Vec::new();
            for column in 0..columns {
                let cell = &row[Column(column)];
                let shown = |text: &str, flags: Flags| Shown {
                    text: text.to_owned(),
                    fg: shade(cell.fg),
                    bg: shade(cell.bg),
                    underline_color: cell.underline_color().map(shade),
                    flags: flags & VISIBLE_FLAGS,
                    link: cell.hyperlink().map(|link| link.uri().to_owned()),
                };
                let blank_flags = cell.flags - Flags::WIDE_CHAR;
                let broken_half = || Shown {
                    text: " ".to_owned(),
                    fg: Shade::Default,
                    bg: Shade::Default,
                    underline_color: None,
                    flags: Flags::empty(),
                    link: None,
                };
                if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    if column == 0 || !flags(column - 1).contains(Flags::WIDE_CHAR) {
                        cells.push(broken_half());
                    }
                    continue;
                }
                if cell.flags.contains(Flags::WIDE_CHAR)
                    && (column + 1 == columns
                        || !flags(column + 1).contains(Flags::WIDE_CHAR_SPACER))
                {
                    cells.push(broken_half());
                    continue;
                }
                if cell.flags.contains(Flags::HIDDEN) {
                    let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                        2
                    } else {
                        1
                    };
                    cells.extend((0..width).map(|_| shown(" ", blank_flags)));
                    continue;
                }
                let mut text = String::new();
                if cell.c == '\t' {
                    text.push(' ');
                } else {
                    text.push(cell.c);
                    // Alacritty gives each part of an emoji sequence a cell
                    // of its own, which Embers draws without the selector
                    // and joiner so no host draws the part wide.
                    text.extend(
                        cell.zerowidth()
                            .into_iter()
                            .flatten()
                            .filter(|&&ch| ch != '\u{FE0F}' && ch != '\u{200D}'),
                    );
                }
                // Hosts disagree on a regional indicator alone, half a flag,
                // which Embers draws as U+FFFD; whole flags it draws as they
                // are. Either way only the columns can be compared.
                if text
                    .chars()
                    .any(|ch| ('\u{1F1E6}'..='\u{1F1FF}').contains(&ch))
                {
                    text = "\u{FFFD}".to_owned();
                }
                cells.push(shown(&text, cell.flags));
            }
            while cells.last().is_some_and(Shown::is_blank) {
                cells.pop();
            }
            cells
        })
        .collect()
}

/// The screen a program writing `bytes` sees on a terminal of its own.
fn direct(bytes: &[u8]) -> Vec<Vec<Shown>> {
    let (mut term, mut parser) = terminal();
    parser.advance(&mut term, bytes);
    screen(&term)
}

/// The screen the host shows after Embers draws a pane the program wrote
/// `bytes` to, filling the whole terminal.
fn through_embers(bytes: &[u8]) -> Vec<Vec<Shown>> {
    let mut pane = AlacrittyTerminalBackend::new(PtySize::new(COLUMNS, ROWS), 0);
    pane.ingest_bytes(bytes);
    let snapshot = pane.visible_snapshot(0, PtySize::new(COLUMNS, ROWS), None);

    let mut grid = RenderGrid::new(COLUMNS, ROWS);
    for (row, line) in snapshot.lines.iter().enumerate() {
        grid.put_snapshot_line(0, row as u16, COLUMNS, line);
    }
    let frame = frame_bytes(
        &grid,
        Size {
            width: COLUMNS,
            height: ROWS,
        },
        None,
        None,
        HostCapabilities::ALL,
        b"",
    );

    let (mut host, mut parser) = terminal();
    parser.advance(&mut host, &frame);
    screen(&host)
}

/// Where the screen through Embers first differs from the direct one.
fn first_difference(bytes: &[u8]) -> Option<String> {
    let embers = through_embers(bytes);
    let direct = direct(bytes);
    let (row, (embers, direct)) = embers
        .iter()
        .zip(&direct)
        .enumerate()
        .find(|(_, (embers, direct))| embers != direct)?;
    let column = embers
        .iter()
        .zip(direct)
        .position(|(embers, direct)| embers != direct)
        .unwrap_or(embers.len().min(direct.len()));
    Some(format!(
        "bytes {:?} differ on row {row} at cell {column}:\n through Embers: {:?}\n direct:         {:?}",
        String::from_utf8_lossy(bytes),
        embers.get(column),
        direct.get(column),
    ))
}

fn assert_round_trip(bytes: &[u8]) {
    if let Some(difference) = first_difference(bytes) {
        panic!("{difference}");
    }
}

#[test]
fn plain_text_and_line_editing() {
    assert_round_trip(b"hello\r\nworld\x1b[1;3Hxy\x1b[2;2H\x1b[K\x1b[3;1Habc\tdef");
}

#[test]
fn colors_and_attributes() {
    assert_round_trip(
        b"\x1b[1;31mred\x1b[0m \x1b[2;3;9mdim\x1b[0m\r\n\
          \x1b[38;5;200;48;2;1;2;3mx\x1b[7my\x1b[0m\r\n\
          \x1b[92;44mbright\x1b[39;49m",
    );
}

#[test]
fn underline_styles_and_colors() {
    assert_round_trip(b"\x1b[4mone\x1b[4:3;58:2::255:0:0mtwo\x1b[4:2;58:5:4mthree\x1b[0m");
}

#[test]
fn trailing_painted_cells() {
    assert_round_trip(b"a\x1b[41m   \x1b[0m\r\nb\x1b[7m  \x1b[0m\r\nc\x1b[4m  \x1b[0m");
}

#[test]
fn hidden_text() {
    assert_round_trip(b"see \x1b[8msecret\x1b[0m done");
}

#[test]
fn hyperlinks() {
    assert_round_trip(b"go \x1b]8;;https://example.com\x1b\\here\x1b]8;;\x1b\\ now");
}

#[test]
fn wide_and_combining_characters() {
    assert_round_trip("漢字 e\u{301} 👍\r\n0123456789abcde漢".as_bytes());
}

/// Transgender flag: white flag, VS16, ZWJ, transgender symbol, VS16.
const FLAG: &str = "\u{1F3F3}\u{FE0F}\u{200D}\u{26A7}\u{FE0F}";

#[test]
fn emoji_sequences_keep_the_columns_the_emulator_gave_them() {
    // In the middle of a row, wrapped at its last column, overwritten from
    // the start, and a symbol made wide by VS16 on its own.
    assert_round_trip(format!("ab {FLAG} cd").as_bytes());
    assert_round_trip(format!("0123456789abcde{FLAG}xy").as_bytes());
    assert_round_trip(format!("\x1b[1;6H{FLAG}x\x1b[1;6Ha").as_bytes());
    assert_round_trip("a \u{26A7}\u{FE0F} b \u{2764}\u{FE0F} c #\u{FE0F}\u{20E3}".as_bytes());
}

#[test]
fn erase_with_background_color() {
    assert_round_trip(b"\x1b[44m\x1b[2J\x1b[Htext\x1b[0m");
}

#[test]
fn screen_alignment_pattern() {
    assert_round_trip(b"\x1b#8");
}

/// One step of what a program might write.
fn step() -> impl Strategy<Value = Vec<u8>> {
    let text = prop::collection::vec(
        prop::sample::select(vec![
            "a",
            "Z",
            " ",
            "~",
            "漢",
            "e\u{301}",
            "👍",
            "\t",
            "\r",
            "\n",
            FLAG,
            "\u{26A7}\u{FE0F}",
            "\u{1F1F8}",
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
        "\x1b[L",
        "\x1b[M",
        "\x1b[S",
        "\x1b[T",
        "\x1b[2;4r",
        "\x1b[r",
        "\x1bM",
        "\x1b#8",
        "\x1b]8;;https://a.example\x1b\\",
        "\x1b]8;;\x1b\\",
    ])
    .prop_map(|edit| edit.as_bytes().to_vec());
    prop_oneof![3 => text, 2 => sgr, 1 => cursor, 1 => edit]
}

proptest! {
    #[test]
    fn any_output_looks_the_same_through_embers(
        steps in prop::collection::vec(step(), 1..24),
    ) {
        let bytes = steps.concat();
        if let Some(difference) = first_difference(&bytes) {
            return Err(TestCaseError::fail(difference));
        }
    }
}
