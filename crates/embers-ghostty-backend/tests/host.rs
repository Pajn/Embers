//! What a pane shows, drawn by Embers onto a Ghostty host terminal, against
//! what the pane's emulator holds: the same cells in the same columns. The
//! host runs with grapheme clustering, as Ghostty does.

use embers_client::RenderGrid;
use embers_core::{PtySize, SnapshotLine};
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{AlacrittyTerminalBackend, TerminalBackend};

const COLUMNS: u16 = 12;
const ROWS: u16 = 2;

/// Text that emulators and Unicode lay out in different columns, or that
/// emulators split over cells.
const TRICKY: &[&str] = &[
    // Transgender flag, a family and a skin tone: emoji sequences.
    "\u{1F3F3}\u{FE0F}\u{200D}\u{26A7}\u{FE0F}",
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "\u{1F44D}\u{1F3FD}",
    // Symbols made wide by VS16, or kept narrow by VS15.
    "\u{26A7}\u{FE0F}",
    "\u{2764}\u{FE0F}",
    "\u{263A}\u{FE0E}",
    "#\u{FE0F}\u{20E3}",
    // A flag, and half of one.
    "\u{1F1F8}\u{1F1EA}",
    "\u{1F1F8}",
    // Combining marks, Hangul jamo, a Devanagari conjunct.
    "e\u{301}",
    "\u{1100}\u{1161}\u{11A8}",
    "\u{0915}\u{094D}\u{0937}",
];

/// A line's cells as `(text, width)`, up to the last that is not blank,
/// without the selector and joiner Embers leaves out of cells it draws
/// narrowed, and with a regional indicator alone in a column as the U+FFFD
/// Embers draws for it.
fn cells(line: &SnapshotLine) -> Vec<(String, u16)> {
    let mut cells: Vec<(String, u16)> = line
        .cells()
        .map(|cell| {
            let regional = |ch: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&ch);
            let text = if cell.width == 1 && cell.text.chars().any(regional) {
                "\u{FFFD}".to_owned()
            } else {
                cell.text
                    .chars()
                    .filter(|&ch| ch != '\u{FE0F}' && ch != '\u{200D}')
                    .collect()
            };
            (text, cell.width)
        })
        .collect();
    while cells.last().is_some_and(|(text, _)| text == " ") {
        cells.pop();
    }
    cells
}

fn screen(backend: &dyn TerminalBackend) -> Vec<Vec<(String, u16)>> {
    let snapshot = backend.visible_snapshot(0, PtySize::new(COLUMNS, ROWS), None);
    snapshot.lines.iter().map(cells).collect()
}

/// Draws `pane` the way the interactive client does onto a fresh Ghostty
/// host, and returns the host's screen.
fn on_host(pane: &dyn TerminalBackend) -> Vec<Vec<(String, u16)>> {
    let snapshot = pane.visible_snapshot(0, PtySize::new(COLUMNS, ROWS), None);
    let mut grid = RenderGrid::new(COLUMNS, ROWS);
    for (row, line) in snapshot.lines.iter().enumerate() {
        grid.put_snapshot_line(0, row as u16, COLUMNS, line);
    }
    let mut host = GhosttyTerminalBackend::new(PtySize::new(COLUMNS, ROWS), 0);
    host.ingest_bytes(format!("\x1b[H{}", grid.ansi_lines().join("\r\n")).as_bytes());
    screen(&host)
}

fn check(name: &str, new_pane: fn() -> Box<dyn TerminalBackend>) {
    let mut differences = Vec::new();
    for text in TRICKY {
        // At the start of a row, ending at its edge, and wrapping over it.
        for column in [1, COLUMNS - 1, COLUMNS] {
            let mut pane = new_pane();
            pane.ingest_bytes(format!("\x1b[1;{column}H{text}|").as_bytes());
            let (pane, host) = (screen(pane.as_ref()), on_host(pane.as_ref()));
            if pane != host {
                differences.push(format!(
                    "{text:?} from column {column}:\n pane {pane:?}\n host {host:?}"
                ));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "{name} panes look different on the host:\n{}",
        differences.join("\n")
    );
}

#[test]
fn ghostty_panes_look_the_same_on_a_ghostty_host() {
    check("Ghostty", || {
        Box::new(GhosttyTerminalBackend::new(PtySize::new(COLUMNS, ROWS), 0))
    });
}

#[test]
fn alacritty_panes_look_the_same_on_a_ghostty_host() {
    check("alacritty", || {
        Box::new(AlacrittyTerminalBackend::new(
            PtySize::new(COLUMNS, ROWS),
            0,
        ))
    });
}
