//! Drives the Ghostty backend the way a keeper does, with random operations
//! and output, compression included, and checks after each operation that
//! what it reports holds together. Compression must never change what a pane
//! shows: the snapshot and the whole scrollback are the same before and after
//! every pass.
//!
//! Run on demand: `cargo test -p embers-ghostty-backend --test fuzz --
//! --ignored`. `FUZZ_CASES` sets how many cases run (default 32).
//!
//! Test builds compile libghostty with Zig's safety checks. A bug inside
//! libghostty stops the process before proptest can report the case, so each
//! case is first written to `fuzz-last-case.json` in Cargo's test scratch
//! directory, or to `FUZZ_LAST_CASE`. Replay one with `FUZZ_REPLAY=<file>
//! cargo test -p embers-ghostty-backend --test fuzz replay`.

use std::path::PathBuf;

use embers_core::{PtySize, SnapshotLine, TerminalSnapshot};
use embers_ghostty_backend::GhosttyTerminalBackend;
use embers_server::{RawByteRouter, TerminalBackend};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Op {
    /// Pane output, routed as the keeper routes it.
    Output(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    Compress,
    CompressStep,
    Snapshot,
    Capture,
    Slice {
        start: u64,
        count: u32,
    },
    TakeEvents,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Case {
    cols: u16,
    rows: u16,
    max_scrollback_lines: usize,
    ops: Vec<Op>,
}

fn run(case: &Case) {
    let mut size = PtySize::new(case.cols, case.rows);
    let mut backend = GhosttyTerminalBackend::new(size, case.max_scrollback_lines);
    let mut router = RawByteRouter::default();
    for (index, op) in case.ops.iter().enumerate() {
        let at = format!("op {index} ({op:?})");
        match op {
            Op::Output(bytes) => {
                router.route_output(&mut backend, bytes);
            }
            Op::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                size = PtySize {
                    cols: *cols,
                    rows: *rows,
                    pixel_width: *pixel_width,
                    pixel_height: *pixel_height,
                };
                backend.resize(size);
            }
            Op::Compress | Op::CompressStep => {
                let before = (
                    backend.visible_snapshot(0, size, None),
                    backend.capture_scrollback(),
                );
                if matches!(op, Op::Compress) {
                    backend.compress_scrollback();
                } else {
                    backend.compress_scrollback_step();
                }
                let after = (
                    backend.visible_snapshot(0, size, None),
                    backend.capture_scrollback(),
                );
                assert!(before == after, "{at}: compression changed the pane");
            }
            Op::Snapshot => check_snapshot(&backend.visible_snapshot(0, size, None), size, &at),
            Op::Capture => {
                let lines = backend.capture_scrollback();
                let total = backend.metadata().total_lines;
                assert!(
                    lines.len() as u64 <= total,
                    "{at}: {} > {total}",
                    lines.len()
                );
            }
            Op::Slice { start, count } => {
                let slice = backend.capture_scrollback_slice(*start, *count);
                let total = backend.metadata().total_lines;
                assert_eq!(slice.total_lines, total, "{at}");
                assert!(slice.start_line <= total, "{at}");
                let expected = (*count as u64).min(total - slice.start_line);
                assert_eq!(slice.lines.len() as u64, expected, "{at}");
                // The same lines as a capture of the whole scrollback.
                let capture = backend.capture_scrollback();
                for (offset, line) in slice.lines.iter().enumerate() {
                    check_line(line, size.cols, &at);
                    let global = slice.start_line as usize + offset;
                    let captured = capture.get(global).map_or("", String::as_str);
                    assert_eq!(line.text, captured, "{at}: line {global}");
                }
            }
            Op::TakeEvents => {
                backend.take_events();
            }
        }
        // Captures scroll libghostty's viewport and must leave it at the
        // bottom.
        let metadata = backend.metadata();
        assert_eq!(
            metadata.viewport_top_line + u64::from(size.rows.max(1)),
            metadata.total_lines,
            "{at}: viewport left scrolled"
        );
    }
    check_snapshot(&backend.visible_snapshot(0, size, None), size, "end");
}

fn check_snapshot(snapshot: &TerminalSnapshot, size: PtySize, at: &str) {
    assert_eq!(snapshot.lines.len(), usize::from(size.rows.max(1)), "{at}");
    for line in &snapshot.lines {
        check_line(line, size.cols, at);
    }
    if let Some(cursor) = snapshot.cursor {
        assert!(cursor.position.row < size.rows.max(1), "{at}: {cursor:?}");
        assert!(cursor.position.col < size.cols.max(1), "{at}: {cursor:?}");
    }
}

fn check_line(line: &SnapshotLine, cols: u16, at: &str) {
    // Columns as the line's cells cover them: Ghostty clusters graphemes,
    // so an emoji sequence is one cell however many codepoints it holds.
    let columns = line.width();
    assert!(
        columns <= usize::from(cols.max(1)),
        "{at}: {columns} columns in {cols}: {line:?}"
    );
    if line.runs.is_empty() {
        assert!(line.links.is_empty(), "{at}: links without runs: {line:?}");
        return;
    }
    let mut offset = 0;
    for run in &line.runs {
        assert!(run.len > 0, "{at}: empty run: {line:?}");
        offset += run.len as usize;
        assert!(
            line.text.is_char_boundary(offset.min(line.text.len())),
            "{at}: run ends inside a character: {line:?}"
        );
        assert!(
            usize::from(run.link) <= line.links.len(),
            "{at}: link {} of {}: {line:?}",
            run.link,
            line.links.len()
        );
    }
    assert_eq!(
        offset,
        line.text.len(),
        "{at}: runs cover {offset}: {line:?}"
    );
}

/// Output a program might write, odd and well-formed alike.
fn output() -> impl Strategy<Value = Vec<u8>> {
    let text = prop::collection::vec(
        prop::sample::select(vec![
            "a",
            "Z",
            " ",
            "~",
            "漢",
            "字",
            "e\u{301}",
            "👍",
            "❤\u{fe0f}",
            "👨\u{200d}👩\u{200d}👧",
            "\u{200b}",
            "\r",
            "\n",
            "\r\n",
            "\t",
            "\x08",
            "\x07",
            "\x0b",
            "\x0c",
            "\x0e",
            "\x0f",
        ]),
        1..40,
    )
    .prop_map(|parts| parts.concat().into_bytes());
    let lines = (1..400_usize, 0..120_usize).prop_map(|(lines, width)| {
        let mut out = Vec::new();
        for line in 0..lines {
            out.extend_from_slice(format!("\x1b[3{}m{line}", line % 8).as_bytes());
            out.extend(std::iter::repeat_n(b'x', width));
            out.extend_from_slice(b"\x1b[0m\r\n");
        }
        out
    });
    let number = prop::sample::select(vec![
        "",
        "0",
        "1",
        "2",
        "3",
        "5",
        "7",
        "25",
        "999",
        "65535",
        "4294967295",
        "99999999999",
    ]);
    let csi = (
        prop::sample::select(vec!["", "?", ">", "=", "<"]),
        prop::collection::vec(number, 0..4),
        prop::sample::select(vec!["", " ", "$", "!", "\""]),
        prop::sample::select(
            "@ABCDEFGHIJKLMPSTXZ`abcdefghijlmnpqrstuxz{|}~"
                .chars()
                .collect::<Vec<_>>(),
        ),
    )
        .prop_map(|(prefix, params, intermediate, last)| {
            format!("\x1b[{prefix}{}{intermediate}{last}", params.join(";")).into_bytes()
        });
    let sgr = prop::sample::select(vec![
        "0",
        "1",
        "2",
        "3",
        "4",
        "4:2",
        "4:3",
        "4:5",
        "5",
        "7",
        "8",
        "9",
        "21",
        "22",
        "53",
        "38;5;200",
        "48;2;1;2;3",
        "58:2::9:8:7",
        "58:5:4",
        "38:2::300:0:0",
        "39;49;59",
        "1;31;42",
    ])
    .prop_map(|sgr| format!("\x1b[{sgr}m").into_bytes());
    let sequence = prop::sample::select(vec![
        "\x1b]0;title\x07",
        "\x1b]2;\x1b\\",
        "\x1b]8;id=a;https://e.example/a\x1b\\",
        "\x1b]8;;\x1b\\",
        "\x1b]8;;https://e.example/\x1b\\link\x1b]8;;\x1b\\",
        "\x1b]52;c;aGVsbG8=\x07",
        "\x1b]52;p;!!!\x07",
        "\x1b]52;c;?\x07",
        "\x1b]7;file://host/tmp/dir\x07",
        "\x1b]4;1;rgb:ff/00/00\x07",
        "\x1b]10;?\x07",
        "\x1b]133;A\x07",
        "\x1bP$qm\x1b\\",
        "\x1bP$qr\x1b\\",
        "\x1bP+q536d756c78;544e;7878\x1b\\",
        "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\",
        "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\",
        "\x1b_Ga=T,f=24,s=1,v=1;AAAA\x1b\\",
        "\x1b7",
        "\x1b8",
        "\x1b#8",
        "\x1bM",
        "\x1bD",
        "\x1bE",
        "\x1bc",
        "\x1b(0lqk\x1b(B",
        "\x1b[?1049h",
        "\x1b[?1049l",
        "\x1b[?47h",
        "\x1b[?2026h",
        "\x1b[?2026l",
        "\x1b[?7l",
        "\x1b[?7h",
        "\x1b[?6h",
        "\x1b[?69h\x1b[3;8s",
        "\x1b[2;5r",
        "\x1b[r",
        "\x1b[>1u",
        "\x1b[<u",
        "\x1b[c",
        "\x1b[6n",
        "\x1b[16t",
        "\x1b[14t",
        "\x1b[18t",
        "\x1b[>q",
    ])
    .prop_map(|sequence| sequence.as_bytes().to_vec());
    let bytes = prop::collection::vec(any::<u8>(), 1..32);
    prop::collection::vec(
        prop_oneof![4 => text, 1 => lines, 3 => csi, 2 => sgr, 3 => sequence, 1 => bytes],
        1..12,
    )
    .prop_map(|parts| parts.concat())
}

fn op() -> impl Strategy<Value = Op> {
    let resize = (
        0..220_u16,
        0..70_u16,
        prop::sample::select(vec![0_u16, 8, 10]),
    )
        .prop_map(|(cols, rows, cell)| Op::Resize {
            cols,
            rows,
            pixel_width: cols.saturating_mul(cell),
            pixel_height: rows.saturating_mul(cell * 2),
        });
    let slice = (0..2_000_u64, 0..300_u32).prop_map(|(start, count)| Op::Slice { start, count });
    prop_oneof![
        8 => output().prop_map(Op::Output),
        1 => resize,
        2 => Just(Op::Compress),
        2 => Just(Op::CompressStep),
        2 => Just(Op::Snapshot),
        1 => Just(Op::Capture),
        1 => slice,
        1 => Just(Op::TakeEvents),
    ]
}

fn case() -> impl Strategy<Value = Case> {
    (
        1..200_u16,
        1..60_u16,
        prop::sample::select(vec![0_usize, 10, 100, 500, 10_000]),
        prop::collection::vec(op(), 1..40),
    )
        .prop_map(|(cols, rows, max_scrollback_lines, ops)| Case {
            cols,
            rows,
            max_scrollback_lines,
            ops,
        })
}

/// Where each case is saved before it runs: `FUZZ_LAST_CASE`, for runs in
/// parallel, else `fuzz-last-case.json` in Cargo's test scratch directory.
fn last_case_path() -> PathBuf {
    std::env::var_os("FUZZ_LAST_CASE").map_or_else(
        || PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fuzz-last-case.json"),
        PathBuf::from,
    )
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("FUZZ_CASES").ok().and_then(|cases| cases.parse().ok()).unwrap_or(32),
        ..ProptestConfig::default()
    })]

    #[test]
    #[ignore = "fuzzing runs on demand"]
    fn the_backend_holds_together_under_any_use(case in case()) {
        let json = serde_json::to_vec(&case).expect("the case serializes");
        std::fs::write(last_case_path(), json).expect("the case is saved");
        run(&case);
    }
}

#[test]
fn replay() {
    let Some(path) = std::env::var_os("FUZZ_REPLAY") else {
        return;
    };
    let json = std::fs::read(&path).expect("the case file reads");
    let case: Case = serde_json::from_slice(&json).expect("the case parses");
    run(&case);
}
