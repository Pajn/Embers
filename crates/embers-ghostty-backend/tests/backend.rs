//! What the Ghostty backend reports for known output.

use embers_core::{
    ActivityState, CellAttrs, ClipboardSelection, ClipboardWrite, CursorPosition, CursorShape,
    PtySize, SnapshotLine, TermColor,
};
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{RawByteRouter, TerminalBackend};

fn backend(cols: u16, rows: u16) -> GhosttyTerminalBackend {
    GhosttyTerminalBackend::new(PtySize::new(cols, rows), 100)
}

fn lines(backend: &GhosttyTerminalBackend) -> Vec<SnapshotLine> {
    backend.visible_snapshot(0, PtySize::new(1, 1), None).lines
}

fn text(backend: &GhosttyTerminalBackend) -> Vec<String> {
    lines(backend).into_iter().map(|line| line.text).collect()
}

/// The replies to `bytes`, routed the way the keeper routes pane output, so
/// that queries Embers answers itself are answered too.
fn replies(backend: &mut GhosttyTerminalBackend, bytes: &[u8]) -> String {
    let mut router = RawByteRouter::default();
    router.route_output(backend, bytes);
    String::from_utf8(backend.take_events().replies).expect("replies are text")
}

#[test]
fn text_wide_and_combining_characters() {
    let mut backend = backend(10, 2);
    backend.ingest_bytes("漢e\u{301}👍x\r\nab".as_bytes());
    assert_eq!(text(&backend), ["漢e\u{301}👍x", "ab"]);
}

#[test]
fn emoji_sequences_are_one_character_even_after_a_reset() {
    // A flag (white flag, ZWJ, transgender symbol), then its first column
    // overwritten: the whole flag goes, as in Ghostty.
    let flag = "\u{1F3F3}\u{FE0F}\u{200D}\u{26A7}\u{FE0F}";
    let mut backend = backend(10, 1);
    backend.ingest_bytes(format!("{flag}x\x1b[Ha").as_bytes());
    assert_eq!(text(&backend), ["a x"]);
    backend.ingest_bytes(format!("\x1bc{flag}x\x1b[Ha").as_bytes());
    assert_eq!(text(&backend), ["a x"]);
}

#[test]
fn trailing_blanks_are_trimmed_but_cells_that_show_are_kept() {
    let mut backend = backend(8, 4);
    backend.ingest_bytes(b"a   \r\nb\x1b[41m  \x1b[0m\r\nc\x1b[4m  \x1b[0m\r\nd\x1b[7m \x1b[0m");
    assert_eq!(text(&backend), ["a", "b  ", "c  ", "d "]);
}

#[test]
fn erasing_in_a_background_color_paints_the_line() {
    let mut backend = backend(4, 1);
    backend.ingest_bytes(b"\x1b[44m\x1b[K");
    let line = &lines(&backend)[0];
    assert_eq!(line.text, "    ");
    assert_eq!(line.runs.len(), 1);
    assert_eq!(line.runs[0].bg, TermColor::Indexed(4));
}

#[test]
fn styles_map_to_attributes_and_colors() {
    let mut backend = backend(20, 1);
    backend.ingest_bytes(
        b"\x1b[1;2;3;7;8;9ma\x1b[0m\
          \x1b[4mb\x1b[4:2mc\x1b[4:3md\x1b[4:4me\x1b[4:5mf\x1b[0m\
          \x1b[31;42mg\x1b[38;5;200;48;2;1;2;3mh\x1b[0m\
          \x1b[4;58:2::9:8:7mi\x1b[58:5:5mj",
    );
    let line = &lines(&backend)[0];
    let style = |index: usize| line.runs[index];
    assert_eq!(
        style(0).attrs.0,
        CellAttrs::BOLD
            | CellAttrs::DIM
            | CellAttrs::ITALIC
            | CellAttrs::INVERSE
            | CellAttrs::HIDDEN
            | CellAttrs::STRIKEOUT
    );
    let underlines: Vec<u16> = (1..=5).map(|index| style(index).attrs.0).collect();
    assert_eq!(
        underlines,
        [
            CellAttrs::UNDERLINE,
            CellAttrs::DOUBLE_UNDERLINE,
            CellAttrs::CURLY_UNDERLINE,
            CellAttrs::DOTTED_UNDERLINE,
            CellAttrs::DASHED_UNDERLINE,
        ]
    );
    assert_eq!(
        (style(6).fg, style(6).bg),
        (TermColor::Indexed(1), TermColor::Indexed(2))
    );
    assert_eq!(
        (style(7).fg, style(7).bg),
        (TermColor::Indexed(200), TermColor::Rgb { r: 1, g: 2, b: 3 })
    );
    assert_eq!(
        style(8).underline_color,
        TermColor::Rgb { r: 9, g: 8, b: 7 }
    );
    assert_eq!(style(9).underline_color, TermColor::Indexed(5));
}

#[test]
fn hyperlinks_carry_their_uri() {
    let mut backend = backend(12, 1);
    backend.ingest_bytes(b"a\x1b]8;id=x;https://e.example\x1b\\bc\x1b]8;;\x1b\\d");
    let line = &lines(&backend)[0];
    assert_eq!(line.text, "abcd");
    assert_eq!(line.links.len(), 1);
    assert_eq!(line.links[0].uri, "https://e.example");
    let linked: Vec<u16> = line.runs.iter().map(|run| run.link).collect();
    assert_eq!(linked, [0, 1, 0]);
}

#[test]
fn cursor_position_visibility_and_shape() {
    let mut backend = backend(10, 3);
    backend.ingest_bytes(b"\x1b[2;4H\x1b[5 q");
    let cursor = backend.metadata().cursor.expect("cursor shown");
    assert_eq!(cursor.position, CursorPosition { row: 1, col: 3 });
    assert_eq!(cursor.shape, CursorShape::Beam);

    backend.ingest_bytes(b"\x1b[3 q");
    assert_eq!(
        backend.metadata().cursor.map(|cursor| cursor.shape),
        Some(CursorShape::Underline)
    );

    backend.ingest_bytes(b"\x1b[?25l");
    assert_eq!(backend.metadata().cursor, None);
}

#[test]
fn modes() {
    let mut backend = backend(10, 3);
    let modes = backend.visible_snapshot(0, PtySize::new(10, 3), None).modes;
    assert!(!modes.alternate_screen && !modes.mouse_reporting && !modes.bracketed_paste);

    backend.ingest_bytes(b"\x1b[?1049h\x1b[?1000h\x1b[?1004h\x1b[?2004h\x1b[>1u");
    let modes = backend.visible_snapshot(0, PtySize::new(10, 3), None).modes;
    assert!(modes.alternate_screen);
    assert!(modes.mouse_reporting);
    assert!(modes.focus_reporting);
    assert!(modes.bracketed_paste);
    assert_eq!(modes.keyboard_mode, 1);
}

#[test]
fn title() {
    let mut backend = backend(10, 1);
    assert_eq!(backend.metadata().title, None);
    backend.ingest_bytes(b"\x1b]2;hello\x07");
    assert_eq!(backend.metadata().title.as_deref(), Some("hello"));
}

#[test]
fn scrollback_is_captured_whole_and_in_slices() {
    let mut backend = backend(6, 2);
    backend.ingest_bytes(b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(
        backend.capture_scrollback(),
        ["one", "two", "three", "four"]
    );
    assert_eq!(backend.metadata().total_lines, 4);

    let slice = backend.capture_scrollback_slice(1, 2);
    assert_eq!(slice.start_line, 1);
    assert_eq!(slice.total_lines, 4);
    let slice_text: Vec<&str> = slice.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(slice_text, ["two", "three"]);
}

#[test]
fn bell_and_clipboard_writes_are_reported_once() {
    let mut backend = backend(10, 1);
    backend.ingest_bytes(b"\x07\x1b]52;c;aGVsbG8=\x07");
    let events = backend.take_events();
    assert_eq!(events.activity, ActivityState::Bell);
    assert_eq!(
        events.clipboard,
        Some(ClipboardWrite {
            selection: ClipboardSelection::Clipboard,
            text: "hello".to_owned(),
        })
    );
    let events = backend.take_events();
    assert_eq!(events.activity, ActivityState::Activity);
    assert_eq!(events.clipboard, None);
}

#[test]
fn device_attributes_and_status_reports() {
    let mut backend = backend(10, 3);
    assert_eq!(replies(&mut backend, b"\x1b[c"), "\x1b[?6c");
    assert_eq!(replies(&mut backend, b"\x1b[2;3H\x1b[6n"), "\x1b[2;3R");
    assert_eq!(replies(&mut backend, b"\x1b[5n"), "\x1b[0n");
}

#[test]
fn the_pen_is_reported_once_through_decrqss() {
    let mut backend = backend(10, 1);
    // libghostty's own reply.
    assert_eq!(
        replies(&mut backend, b"\x1b[1;4:3;38:5:1m\x1bP$qm\x1b\\"),
        "\x1bP1$r0;1;4:3;31m\x1b\\"
    );
    // libghostty also reports the scroll region, where the alacritty backend
    // reports the request invalid.
    assert_eq!(
        replies(&mut backend, b"\x1bP$qr\x1b\\"),
        "\x1bP1$r1;1r\x1b\\"
    );
}

#[test]
fn terminfo_capabilities_are_reported_once_through_xtgettcap() {
    let mut backend = backend(10, 1);
    // `Smulx`, which libghostty answers in terminfo's own notation (`\E` for
    // escape), and `xx`, which nothing knows.
    assert_eq!(
        replies(&mut backend, b"\x1bP+q536d756c78;7878\x1b\\"),
        "\x1bP1+r536D756C78=5C455B343A25703125646D\x1b\\\x1bP0+r7878\x1b\\"
    );
}

#[test]
fn the_cell_size_is_reported_once_known() {
    let mut without_pixels = backend(10, 4);
    assert_eq!(replies(&mut without_pixels, b"\x1b[16t"), "");

    let mut sized = GhosttyTerminalBackend::new(
        PtySize {
            cols: 10,
            rows: 4,
            pixel_width: 80,
            pixel_height: 64,
        },
        100,
    );
    assert_eq!(replies(&mut sized, b"\x1b[16t"), "\x1b[6;16;8t");
    assert_eq!(replies(&mut sized, b"\x1b[14t"), "\x1b[4;64;80t");
}

#[test]
fn long_hyperlinks_are_read_whole() {
    let uri = format!("https://e.example/{}", "x".repeat(600));
    let mut backend = backend(4, 1);
    backend.ingest_bytes(format!("\x1b]8;;{uri}\x1b\\a").as_bytes());
    assert_eq!(lines(&backend)[0].links[0].uri, uri);
}

#[test]
fn scrollback_keeps_about_the_lines_asked_for() {
    let mut backend = GhosttyTerminalBackend::new(PtySize::new(40, 5), 3_000);
    for line in 0..5_000 {
        backend.ingest_bytes(format!("\x1b[3{}mline {line}\r\n", line % 8).as_bytes());
    }
    // libghostty prunes whole pages, so it keeps a page's worth more or
    // fewer.
    let total = backend.metadata().total_lines;
    assert!((2_400..3_600).contains(&total), "{total}");
}

#[test]
fn scrollback_slices_span_viewports_and_leave_the_screen_as_it_was() {
    let mut backend = backend(8, 5);
    let numbers: Vec<String> = (0..20).map(|line| format!("line {line}")).collect();
    backend.ingest_bytes(numbers.join("\r\n").as_bytes());
    let before = text(&backend);

    let slice_text = |start, count| -> Vec<String> {
        backend
            .capture_scrollback_slice(start, count)
            .lines
            .into_iter()
            .map(|line| line.text)
            .collect()
    };
    assert_eq!(slice_text(3, 9), numbers[3..12]);
    assert_eq!(slice_text(17, 10), numbers[17..20]);
    assert_eq!(backend.capture_scrollback(), numbers);

    assert_eq!(text(&backend), before);
    assert_eq!(backend.metadata().viewport_top_line, 15);
}
