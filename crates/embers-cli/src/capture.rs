//! `capture-pane -p [-S start] [-E end] [-J] [-e]`, read from the server's
//! scrollback slices.
//!
//! Lines are numbered like tmux: 0 is the first row of the screen, negative
//! numbers reach into the history and `-` is the start of the history (`-S`)
//! or the last row of the screen (`-E`). Without `-S` and `-E` the screen is
//! captured.
//!
//! Captured lines carry no wrap flag, so `-J` joins a line to the next when
//! it fills the pane's width, which is how a wrapped line ends. A line that
//! exactly fills the width without wrapping is joined too.

use embers_core::{
    BufferId, CellAttrs, MuxError, Result, SnapshotLine, StyledRun, TermColor, new_request_id,
};
use embers_protocol::{BufferRequest, ClientMessage, ServerResponse};
use unicode_width::UnicodeWidthStr;

use crate::CliConnection;

/// How many lines one scrollback request asks for.
const SLICE_LINES: u32 = 1000;

/// One end of a tmux capture range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineBound {
    /// `-`: the start of the history or the end of the screen.
    Edge,
    Line(i64),
}

pub fn parse_line_bound(value: &str) -> std::result::Result<LineBound, String> {
    if value == "-" {
        return Ok(LineBound::Edge);
    }
    value
        .parse()
        .map(LineBound::Line)
        .map_err(|_| "expected a line number or -".to_owned())
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CaptureOptions {
    pub start: Option<LineBound>,
    pub end: Option<LineBound>,
    pub join: bool,
    pub escapes: bool,
}

/// The global lines (0 is the top of the history) a capture covers, given
/// `history` lines above a screen of `rows`.
fn capture_range(
    start: Option<LineBound>,
    end: Option<LineBound>,
    history: u64,
    rows: u64,
) -> (u64, u64) {
    let total = history + rows;
    let last = total.saturating_sub(1);
    let global = |line: i64| -> u64 {
        let line = i128::from(history) + i128::from(line);
        u64::try_from(line.clamp(0, i128::from(last))).unwrap_or(0)
    };
    let first = match start {
        None => history.min(last),
        Some(LineBound::Edge) => 0,
        Some(LineBound::Line(line)) => global(line),
    };
    let final_line = match end {
        None | Some(LineBound::Edge) => last,
        Some(LineBound::Line(line)) => global(line),
    };
    if first <= final_line {
        (first, final_line)
    } else {
        (final_line, first)
    }
}

impl CliConnection {
    pub(crate) async fn capture_lines(
        &mut self,
        buffer_id: BufferId,
        options: CaptureOptions,
    ) -> Result<String> {
        let buffer = match self
            .request(ClientMessage::Buffer(BufferRequest::Get {
                request_id: new_request_id(),
                buffer_id,
            }))
            .await?
        {
            ServerResponse::Buffer(response) => response.buffer,
            other => {
                return Err(MuxError::protocol(format!(
                    "unexpected response to buffer get: {other:?}"
                )));
            }
        };
        let (_, total) = self.scrollback(buffer_id, 0, 0).await?;
        let rows = u64::from(buffer.pty_size.rows).min(total);
        let (first, last) = capture_range(options.start, options.end, total - rows, rows);
        let mut lines = Vec::new();
        let mut next = first;
        while next <= last {
            let count = u32::try_from(last - next + 1)
                .unwrap_or(u32::MAX)
                .min(SLICE_LINES);
            let (slice, _) = self.scrollback(buffer_id, next, count).await?;
            if slice.is_empty() {
                break;
            }
            next += u64::try_from(slice.len()).unwrap_or(u64::MAX);
            lines.extend(slice);
        }

        let width = usize::from(buffer.pty_size.cols);
        let mut output = Vec::new();
        let mut joined = String::new();
        for line in &lines {
            joined.push_str(&if options.escapes {
                render_with_escapes(line)
            } else {
                line.text.clone()
            });
            let wrapped = options.join && width > 0 && line.text.width() >= width;
            if !wrapped {
                output.push(std::mem::take(&mut joined));
            }
        }
        if !joined.is_empty() {
            output.push(joined);
        }
        Ok(output.join("\n"))
    }

    async fn scrollback(
        &mut self,
        buffer_id: BufferId,
        start_line: u64,
        line_count: u32,
    ) -> Result<(Vec<SnapshotLine>, u64)> {
        match self
            .request(ClientMessage::Buffer(BufferRequest::ScrollbackSlice {
                request_id: new_request_id(),
                buffer_id,
                start_line,
                line_count,
            }))
            .await?
        {
            ServerResponse::ScrollbackSlice(response) => Ok((response.lines, response.total_lines)),
            other => Err(MuxError::protocol(format!(
                "unexpected response to capture-pane: {other:?}"
            ))),
        }
    }
}

/// `line` with SGR sequences for its colours and attributes, like
/// `capture-pane -e`, ending in a reset when any style was set.
fn render_with_escapes(line: &SnapshotLine) -> String {
    if line.runs.is_empty() {
        return line.text.clone();
    }
    let mut output = String::new();
    let mut offset = 0;
    let mut styled = false;
    for run in &line.runs {
        let end = (offset + run.len as usize).min(line.text.len());
        let params = sgr_params(run);
        if !params.is_empty() || styled {
            output.push_str("\x1b[0");
            for param in &params {
                output.push(';');
                output.push_str(param);
            }
            output.push('m');
            styled = !params.is_empty();
        }
        output.push_str(line.text.get(offset..end).unwrap_or_default());
        offset = end;
    }
    output.push_str(line.text.get(offset..).unwrap_or_default());
    if styled {
        output.push_str("\x1b[0m");
    }
    output
}

fn sgr_params(run: &StyledRun) -> Vec<String> {
    let mut params = Vec::new();
    let attrs = run.attrs;
    for (flag, code) in [
        (CellAttrs::BOLD, "1"),
        (CellAttrs::DIM, "2"),
        (CellAttrs::ITALIC, "3"),
        (CellAttrs::UNDERLINE, "4"),
        (CellAttrs::DOUBLE_UNDERLINE, "4:2"),
        (CellAttrs::CURLY_UNDERLINE, "4:3"),
        (CellAttrs::DOTTED_UNDERLINE, "4:4"),
        (CellAttrs::DASHED_UNDERLINE, "4:5"),
        (CellAttrs::INVERSE, "7"),
        (CellAttrs::HIDDEN, "8"),
        (CellAttrs::STRIKEOUT, "9"),
    ] {
        if attrs.contains(flag) {
            params.push(code.to_owned());
        }
    }
    if let Some(color) = color_param(run.fg, 30, 90, 38) {
        params.push(color);
    }
    if let Some(color) = color_param(run.bg, 40, 100, 48) {
        params.push(color);
    }
    if let Some(color) = color_param(run.underline_color, 0, 0, 58) {
        params.push(color);
    }
    params
}

/// The SGR parameter selecting `color`: `base + n` for the eight basic
/// colours and `bright + n - 8` for their bright forms (when the attribute
/// has them), else the 256-colour or true-colour form of `extended`.
fn color_param(color: TermColor, base: u8, bright: u8, extended: u8) -> Option<String> {
    match color {
        TermColor::Default => None,
        TermColor::Indexed(index) if base != 0 && index < 8 => Some((base + index).to_string()),
        TermColor::Indexed(index) if bright != 0 && index < 16 => {
            Some((bright + index - 8).to_string())
        }
        TermColor::Indexed(index) => Some(format!("{extended};5;{index}")),
        TermColor::Rgb { r, g, b } => Some(format!("{extended};2;{r};{g};{b}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_ranges_count_from_the_screen_like_tmux() {
        // 100 lines of history above a 24-row screen.
        assert_eq!(capture_range(None, None, 100, 24), (100, 123));
        assert_eq!(
            capture_range(
                Some(LineBound::Line(-10)),
                Some(LineBound::Line(-1)),
                100,
                24
            ),
            (90, 99)
        );
        assert_eq!(
            capture_range(Some(LineBound::Edge), None, 100, 24),
            (0, 123)
        );
        assert_eq!(
            capture_range(
                Some(LineBound::Line(-500)),
                Some(LineBound::Line(2)),
                100,
                24
            ),
            (0, 102)
        );
        assert_eq!(
            capture_range(Some(LineBound::Line(5)), Some(LineBound::Line(1)), 100, 24),
            (101, 105)
        );
        assert_eq!(
            capture_range(None, Some(LineBound::Line(99)), 0, 24),
            (0, 23)
        );
    }

    #[test]
    fn escapes_reproduce_colours_and_attributes() {
        let run = |len, fg, attrs| StyledRun {
            len,
            fg,
            bg: TermColor::Default,
            attrs: CellAttrs(attrs),
            underline_color: TermColor::Default,
            link: 0,
        };
        let line = SnapshotLine {
            text: "red ok".to_owned(),
            runs: vec![
                run(3, TermColor::Indexed(1), CellAttrs::BOLD),
                run(1, TermColor::Default, 0),
                run(2, TermColor::Rgb { r: 1, g: 2, b: 3 }, 0),
            ],
            links: Vec::new(),
            cell_spans: Vec::new(),
        };
        assert_eq!(
            render_with_escapes(&line),
            "\x1b[0;1;31mred\x1b[0m \x1b[0;38;2;1;2;3mok\x1b[0m"
        );
        assert_eq!(render_with_escapes(&SnapshotLine::plain("plain")), "plain");
    }
}
