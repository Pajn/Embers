//! A [`TerminalBackend`] on libghostty-vt, Ghostty's terminal emulation, to
//! try beside [`embers_server::AlacrittyTerminalBackend`].
//!
//! It follows the alacritty backend wherever Embers has a choice: the same
//! snapshot lines (trailing blanks trimmed, cells that show without a glyph
//! kept, half a wide character left by an edit drawn as a blank), the same
//! modes and the same replies to the queries Embers answers itself. What
//! differs is the emulation, and one thing libghostty does not report:
//! hyperlink ids, so snapshot links carry only their URI.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

use embers_core::{
    CellAttrs, ClipboardSelection, ClipboardWrite, CursorPosition, CursorShape, CursorState,
    Hyperlink, MAX_CLIPBOARD_WRITE_BYTES, PtySize, SnapshotLine, StyledRun, TermColor,
    TerminalModes, TerminalSnapshot,
};
use embers_ghostty_sys as sys;
use embers_server::{
    BackendDamage, BackendEvents, BackendMetadata, BackendScrollbackSlice, TerminalBackend,
    TerminalQuery, sgr_report,
};

/// The DEC private modes the backend reports, as libghostty numbers them.
const MODE_FOCUS_EVENT: u16 = 1004;
const MODE_UTF8_MOUSE: u16 = 1005;
const MODE_SGR_MOUSE: u16 = 1006;
const MODE_BRACKETED_PASTE: u16 = 2004;

pub struct GhosttyTerminalBackend {
    terminal: sys::GhosttyTerminal,
    /// Only for the cursor shape, which libghostty reports nowhere else.
    render_state: sys::GhosttyRenderState,
    /// Owned here and handed to libghostty's callbacks as their userdata.
    events: *mut Events,
    /// Where the replies to the output last ingested start, so a query
    /// libghostty has answered is not answered again.
    last_ingest_replies: usize,
    dirty: bool,
}

// SAFETY: the terminal, render state and events are reached only through this
// value, and libghostty keeps no per-thread state, so the value can move to
// another thread between calls. It is not `Sync`.
unsafe impl Send for GhosttyTerminalBackend {}

impl std::fmt::Debug for GhosttyTerminalBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GhosttyTerminalBackend")
            .field("metadata", &self.metadata())
            .finish()
    }
}

/// What libghostty's callbacks report while output is ingested, drained by
/// [`TerminalBackend::take_events`].
#[derive(Default)]
struct Events {
    replies: Vec<u8>,
    bell: bool,
    clipboard: Option<ClipboardWrite>,
    /// The PTY size, for the size reports.
    size: PtySize,
}

impl GhosttyTerminalBackend {
    /// `max_scrollback_lines` is the scrollback ceiling, as for
    /// [`embers_server::AlacrittyTerminalBackend::new`].
    pub fn new(size: PtySize, max_scrollback_lines: usize) -> Self {
        let mut terminal = ptr::null_mut();
        let mut render_state = ptr::null_mut();
        // SAFETY: plain constructors writing to the out pointers given.
        unsafe {
            check(sys::ghostty_terminal_new(
                ptr::null(),
                &mut terminal,
                size.cols.max(1),
                size.rows.max(1),
            ));
            check(sys::ghostty_render_state_new(
                ptr::null(),
                &mut render_state,
            ));
        }
        let events = Box::into_raw(Box::new(Events {
            size,
            ..Events::default()
        }));
        let mut backend = Self {
            terminal,
            render_state,
            events,
            last_ingest_replies: 0,
            dirty: true,
        };
        backend.configure(max_scrollback_lines);
        backend.resize(size);
        backend
    }

    fn configure(&mut self, max_scrollback_lines: usize) {
        use sys::GhosttyTerminalOption as Option;
        let write_pty: sys::GhosttyTerminalWritePtyFn = Some(on_write_pty);
        let bell: sys::GhosttyTerminalBellFn = Some(on_bell);
        let size: sys::GhosttyTerminalSizeFn = Some(on_size);
        let device_attributes: sys::GhosttyTerminalDeviceAttributesFn = Some(on_device_attributes);
        let clipboard_write: sys::GhosttyTerminalClipboardWriteFn = Some(on_clipboard_write);
        // SAFETY: callbacks and userdata are passed as the pointers
        // themselves, other values by pointer, as `ghostty_terminal_set`
        // documents; `events` outlives the terminal (see `Drop`).
        unsafe {
            self.set(Option::GHOSTTY_TERMINAL_OPT_USERDATA, self.events.cast());
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                write_pty.map_or(ptr::null(), |f| f as *const c_void),
            );
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_BELL,
                bell.map_or(ptr::null(), |f| f as *const c_void),
            );
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_SIZE,
                size.map_or(ptr::null(), |f| f as *const c_void),
            );
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
                device_attributes.map_or(ptr::null(), |f| f as *const c_void),
            );
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                clipboard_write.map_or(ptr::null(), |f| f as *const c_void),
            );
            // Embers limits scrollback in lines; libghostty's default byte
            // limit would otherwise come first.
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                ptr::null(),
            );
            self.set(
                Option::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                (&raw const max_scrollback_lines).cast(),
            );
        }
    }

    /// # Safety
    ///
    /// `value` must be what `ghostty_terminal_set` documents for `option`.
    unsafe fn set(&mut self, option: sys::GhosttyTerminalOption::Type, value: *const c_void) {
        // SAFETY: as the caller promises.
        check(unsafe { sys::ghostty_terminal_set(self.terminal, option, value) });
    }

    /// `data` from the terminal, or `None` when it has no value.
    ///
    /// # Safety
    ///
    /// `T` must be the output type `ghostty_terminal_get` documents for `data`.
    unsafe fn get<T: Default>(&self, data: sys::GhosttyTerminalData::Type) -> Option<T> {
        let mut out = T::default();
        // SAFETY: as the caller promises.
        let result =
            unsafe { sys::ghostty_terminal_get(self.terminal, data, (&raw mut out).cast()) };
        (result == sys::GhosttyResult::GHOSTTY_SUCCESS).then_some(out)
    }

    fn get_u16(&self, data: sys::GhosttyTerminalData::Type) -> u16 {
        // SAFETY: only called for data whose output type is uint16_t.
        unsafe { self.get(data) }.unwrap_or(0)
    }

    fn mode(&self, mode: u16) -> bool {
        let mut config = sys::GhosttyTerminalModeConfig { mode, value: false };
        // SAFETY: GHOSTTY_TERMINAL_DATA_MODE reads and fills a mode config.
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_MODE,
                (&raw mut config).cast(),
            )
        };
        result == sys::GhosttyResult::GHOSTTY_SUCCESS && config.value
    }

    fn string(&self, data: sys::GhosttyTerminalData::Type) -> Option<String> {
        // SAFETY: only called for data whose output type is GhosttyString,
        // which borrows from the terminal until its next mutation; it is
        // copied out before then.
        let string: sys::GhosttyString = unsafe { self.get(data) }?;
        if string.ptr.is_null() || string.len == 0 {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(string.ptr, string.len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn events(&mut self) -> &mut Events {
        // SAFETY: `events` is owned by this value and libghostty only hands
        // it to callbacks during calls made through `&mut self`, none of
        // which is running while this borrow lives.
        unsafe { &mut *self.events }
    }

    fn scrollbar(&self) -> sys::GhosttyTerminalScrollbar {
        // SAFETY: the scrollbar's output type is GhosttyTerminalScrollbar.
        unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_SCROLLBAR) }
            .unwrap_or_default()
    }

    fn cols(&self) -> u16 {
        self.get_u16(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_COLS)
    }

    fn rows(&self) -> u16 {
        self.get_u16(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_ROWS)
    }

    fn cell(&self, tag: sys::GhosttyPointTag::Type, x: u16, y: u32) -> Option<CellView> {
        let point = sys::GhosttyPoint {
            tag,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x, y },
            },
        };
        let mut grid_ref = sys::GhosttyGridRef {
            size: size_of::<sys::GhosttyGridRef>(),
            ..Default::default()
        };
        // SAFETY: the grid ref is read before any further terminal call and
        // every out pointer is valid for its documented type.
        unsafe {
            if sys::ghostty_terminal_grid_ref(self.terminal, point, &mut grid_ref)
                != sys::GhosttyResult::GHOSTTY_SUCCESS
            {
                return None;
            }
            let mut cell: sys::GhosttyCell = 0;
            check(sys::ghostty_grid_ref_cell(&grid_ref, &mut cell));
            let wide: sys::GhosttyCellWide::Type =
                cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_WIDE);
            let tag: sys::GhosttyCellContentTag::Type =
                cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_CONTENT_TAG);

            let has_styling: bool =
                cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_HAS_STYLING);
            let has_text: bool = cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_HAS_TEXT);
            let has_link: bool =
                cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_HAS_HYPERLINK);

            let mut style = sys::GhosttyStyle {
                size: size_of::<sys::GhosttyStyle>(),
                ..Default::default()
            };
            if has_styling {
                check(sys::ghostty_grid_ref_style(&grid_ref, &mut style));
            } else {
                sys::ghostty_style_default(&mut style);
            }
            let mut bg = style_color(style.bg_color);
            // A blank cell erased in a background color holds the color
            // instead of a style.
            if tag == sys::GhosttyCellContentTag::GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE {
                let index: sys::GhosttyColorPaletteIndex =
                    cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_COLOR_PALETTE);
                bg = TermColor::Indexed(index);
            } else if tag == sys::GhosttyCellContentTag::GHOSTTY_CELL_CONTENT_BG_COLOR_RGB {
                let rgb: sys::GhosttyColorRgb =
                    cell_get(cell, sys::GhosttyCellData::GHOSTTY_CELL_DATA_COLOR_RGB);
                bg = TermColor::Rgb {
                    r: rgb.r,
                    g: rgb.g,
                    b: rgb.b,
                };
            }

            let mut codepoints = [0_u32; 16];
            let mut len = 0;
            let mut text = String::new();
            if has_text
                && sys::ghostty_grid_ref_graphemes(
                    &grid_ref,
                    codepoints.as_mut_ptr(),
                    codepoints.len(),
                    &mut len,
                ) == sys::GhosttyResult::GHOSTTY_SUCCESS
            {
                text.extend(codepoints[..len].iter().filter_map(|&c| char::from_u32(c)));
            }

            let link = if has_link {
                hyperlink_uri(&grid_ref)
            } else {
                None
            };

            Some(CellView {
                wide,
                text,
                fg: style_color(style.fg_color),
                bg,
                underline_color: style_color(style.underline_color),
                attrs: style_attrs(&style),
                link,
            })
        }
    }

    fn row(&self, tag: sys::GhosttyPointTag::Type, y: u32) -> Vec<CellView> {
        (0..self.cols())
            .map_while(|x| self.cell(tag, x, y))
            .collect()
    }

    /// One row as a snapshot line, the way the alacritty backend builds one.
    fn styled_line(&self, tag: sys::GhosttyPointTag::Type, y: u32) -> SnapshotLine {
        let cells = self.row(tag, y);
        let content_len = cells
            .iter()
            .rposition(|cell| cell.shows())
            .map_or(0, |last| last + 1);

        let mut text = String::new();
        let mut runs: Vec<StyledRun> = Vec::new();
        let mut links: Vec<Hyperlink> = Vec::new();
        let mut styled = false;
        for column in 0..content_len {
            let cell = &cells[column];
            let blank = match cell.wide {
                sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_SPACER_HEAD => continue,
                sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_SPACER_TAIL => {
                    let covered = column.checked_sub(1).is_some_and(|left| {
                        cells[left].wide == sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_WIDE
                    });
                    if covered {
                        continue;
                    }
                    true
                }
                sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_WIDE => {
                    cells.get(column + 1).is_none_or(|right| {
                        right.wide != sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_SPACER_TAIL
                    })
                }
                _ => false,
            };

            let start = text.len();
            if blank || cell.text.is_empty() {
                text.push(' ');
            } else {
                text.push_str(&cell.text);
            }
            let len = u32::try_from(text.len() - start).unwrap_or(u32::MAX);

            let link = match &cell.link {
                None => 0,
                Some(uri) => {
                    let position = links.iter().position(|known| known.uri == *uri);
                    let index = position.unwrap_or_else(|| {
                        links.push(Hyperlink {
                            id: None,
                            uri: uri.clone(),
                        });
                        links.len() - 1
                    });
                    u16::try_from(index + 1).unwrap_or(u16::MAX)
                }
            };
            if cell.fg != TermColor::Default
                || cell.bg != TermColor::Default
                || cell.attrs != CellAttrs::empty()
                || cell.underline_color != TermColor::Default
                || link != 0
            {
                styled = true;
            }
            match runs.last_mut() {
                Some(last)
                    if last.fg == cell.fg
                        && last.bg == cell.bg
                        && last.attrs == cell.attrs
                        && last.underline_color == cell.underline_color
                        && last.link == link =>
                {
                    last.len += len;
                }
                _ => runs.push(StyledRun {
                    len,
                    fg: cell.fg,
                    bg: cell.bg,
                    attrs: cell.attrs,
                    underline_color: cell.underline_color,
                    link,
                }),
            }
        }
        if !styled {
            runs.clear();
        }
        SnapshotLine { text, runs, links }
    }

    fn cursor_state(&self) -> Option<CursorState> {
        // SAFETY: cursor visibility's output type is bool.
        let visible: bool =
            unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE) }
                .unwrap_or(false);
        if !visible {
            return None;
        }
        let mut position = self.cursor_position();
        // On the right half of a wide character, the cursor is drawn on the
        // character.
        if position.col > 0 {
            let y = u32::from(position.row);
            let tag = sys::GhosttyPointTag::GHOSTTY_POINT_TAG_ACTIVE;
            let on_tail = self.cell(tag, position.col, y).is_some_and(|cell| {
                cell.wide == sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_SPACER_TAIL
            });
            let left_wide = self
                .cell(tag, position.col - 1, y)
                .is_some_and(|cell| cell.wide == sys::GhosttyCellWide::GHOSTTY_CELL_WIDE_WIDE);
            if on_tail && left_wide {
                position.col -= 1;
            }
        }
        let mut style: sys::GhosttyRenderStateCursorVisualStyle::Type =
            sys::GhosttyRenderStateCursorVisualStyle::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK;
        // SAFETY: the render state belongs to this value and is only updated
        // here; the cursor style's output type is the visual style enum.
        unsafe {
            check(sys::ghostty_render_state_update(
                self.render_state,
                self.terminal,
            ));
            check(sys::ghostty_render_state_get(
                self.render_state,
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
                (&raw mut style).cast(),
            ));
        }
        use sys::GhosttyRenderStateCursorVisualStyle as Style;
        let shape = match style {
            Style::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Beam,
            Style::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorShape::Underline,
            _ => CursorShape::Block,
        };
        Some(CursorState { position, shape })
    }

    fn terminal_modes(&self) -> TerminalModes {
        // SAFETY: the output types are the screen enum, bool and uint8_t.
        let (screen, tracking, kitty) = unsafe {
            (
                self.get::<sys::GhosttyTerminalScreen::Type>(
                    sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                ),
                self.get::<bool>(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING),
                self.get::<u8>(
                    sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                ),
            )
        };
        TerminalModes {
            alternate_screen: screen
                == Some(sys::GhosttyTerminalScreen::GHOSTTY_TERMINAL_SCREEN_ALTERNATE),
            // As alacritty's backend counts it: any tracking, or a mouse
            // report encoding.
            mouse_reporting: tracking.unwrap_or(false)
                || self.mode(MODE_SGR_MOUSE)
                || self.mode(MODE_UTF8_MOUSE),
            focus_reporting: self.mode(MODE_FOCUS_EVENT),
            bracketed_paste: self.mode(MODE_BRACKETED_PASTE),
            // Kitty's own flag bits; Embers passes on disambiguate (bit 0)
            // and report-all-keys (bit 3).
            keyboard_mode: kitty.unwrap_or(0) & 0b0000_1001,
        }
    }

    fn total_lines(&self) -> u64 {
        // SAFETY: total rows' output type is size_t.
        unsafe { self.get::<usize>(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_TOTAL_ROWS) }
            .unwrap_or(0) as u64
    }
}

impl Drop for GhosttyTerminalBackend {
    fn drop(&mut self) {
        // SAFETY: each handle was created in `new` and is freed once; the
        // terminal goes before the events its callbacks point to.
        unsafe {
            sys::ghostty_render_state_free(self.render_state);
            sys::ghostty_terminal_free(self.terminal);
            drop(Box::from_raw(self.events));
        }
    }
}

impl TerminalBackend for GhosttyTerminalBackend {
    fn ingest_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.last_ingest_replies = self.events().replies.len();
        // SAFETY: the callbacks this runs only touch `events`, which nothing
        // else borrows during the call.
        unsafe { sys::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
        self.dirty = true;
    }

    fn resize(&mut self, size: PtySize) {
        self.events().size = size;
        let cell = size.cell_size().unwrap_or_default();
        // SAFETY: a plain call on the terminal this value owns.
        check(unsafe {
            sys::ghostty_terminal_resize(
                self.terminal,
                size.cols.max(1),
                size.rows.max(1),
                u32::from(cell.width),
                u32::from(cell.height),
            )
        });
        self.dirty = true;
    }

    fn visible_snapshot(
        &self,
        sequence: u64,
        size: PtySize,
        cwd: Option<PathBuf>,
    ) -> TerminalSnapshot {
        let metadata = self.metadata();
        let tag = sys::GhosttyPointTag::GHOSTTY_POINT_TAG_VIEWPORT;
        TerminalSnapshot {
            sequence,
            size,
            cursor: metadata.cursor,
            lines: (0..u32::from(self.rows()))
                .map(|y| self.styled_line(tag, y))
                .collect(),
            title: metadata.title,
            cwd,
            viewport_top_line: metadata.viewport_top_line,
            total_lines: metadata.total_lines,
            modes: TerminalModes {
                alternate_screen: metadata.alternate_screen,
                mouse_reporting: metadata.mouse_reporting,
                focus_reporting: metadata.focus_reporting,
                bracketed_paste: metadata.bracketed_paste,
                keyboard_mode: metadata.keyboard_mode,
            },
        }
    }

    fn capture_scrollback(&self) -> Vec<String> {
        let tag = sys::GhosttyPointTag::GHOSTTY_POINT_TAG_SCREEN;
        let total = u32::try_from(self.total_lines()).unwrap_or(u32::MAX);
        let mut lines: Vec<String> = (0..total).map(|y| self.styled_line(tag, y).text).collect();
        while matches!(lines.last(), Some(last) if last.is_empty()) {
            lines.pop();
        }
        lines
    }

    fn capture_scrollback_slice(&self, start_line: u64, line_count: u32) -> BackendScrollbackSlice {
        let total_lines = self.total_lines();
        let start_line = start_line.min(total_lines);
        let end_line = start_line
            .saturating_add(u64::from(line_count))
            .min(total_lines);
        let tag = sys::GhosttyPointTag::GHOSTTY_POINT_TAG_SCREEN;
        BackendScrollbackSlice {
            start_line,
            total_lines,
            lines: (start_line..end_line)
                .map(|line| self.styled_line(tag, u32::try_from(line).unwrap_or(u32::MAX)))
                .collect(),
        }
    }

    fn metadata(&self) -> BackendMetadata {
        let modes = self.terminal_modes();
        BackendMetadata {
            title: self.string(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_TITLE),
            viewport_top_line: self.scrollbar().offset,
            total_lines: self.total_lines(),
            alternate_screen: modes.alternate_screen,
            mouse_reporting: modes.mouse_reporting,
            focus_reporting: modes.focus_reporting,
            bracketed_paste: modes.bracketed_paste,
            keyboard_mode: modes.keyboard_mode,
            cursor: self.cursor_state(),
        }
    }

    fn take_events(&mut self) -> BackendEvents {
        let events = self.events();
        BackendEvents {
            activity: if std::mem::take(&mut events.bell) {
                embers_core::ActivityState::Bell
            } else {
                embers_core::ActivityState::Activity
            },
            replies: std::mem::take(&mut events.replies),
            clipboard: events.clipboard.take(),
        }
    }

    fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.events().replies)
    }

    fn take_damage(&mut self) -> BackendDamage {
        if std::mem::take(&mut self.dirty) {
            BackendDamage::Full
        } else {
            BackendDamage::None
        }
    }

    fn answer_query(&mut self, query: &TerminalQuery) {
        // libghostty answers some queries itself. Its replies to the output
        // just ingested, which ended with this query.
        let start = self.last_ingest_replies.min(self.events().replies.len());
        let answered = String::from_utf8_lossy(&self.events().replies[start..]).into_owned();
        let reply = match query {
            // libghostty reports the pen itself.
            TerminalQuery::RequestSetting(_) if answered.contains("$r") => return,
            TerminalQuery::RequestSetting(setting) if setting == b"m" => {
                let mut pen = sys::GhosttyStyle {
                    size: size_of::<sys::GhosttyStyle>(),
                    ..Default::default()
                };
                // SAFETY: the cursor style's output type is GhosttyStyle.
                unsafe {
                    sys::ghostty_terminal_get(
                        self.terminal,
                        sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_STYLE,
                        (&raw mut pen).cast(),
                    );
                }
                let report = sgr_report(
                    style_attrs(&pen),
                    style_color(pen.fg_color),
                    style_color(pen.bg_color),
                    style_color(pen.underline_color),
                );
                format!("\x1bP1$r{report}m\x1b\\")
            }
            TerminalQuery::RequestSetting(_) => "\x1bP0$r\x1b\\".to_owned(),
            // libghostty answers the capabilities it knows and leaves the
            // rest unanswered, where a terminal reports them unknown.
            TerminalQuery::Termcap(payload) => payload
                .split(|&byte| byte == b';')
                .filter(|name| {
                    let name = String::from_utf8_lossy(name).to_ascii_uppercase();
                    !answered.to_ascii_uppercase().contains(&format!("+R{name}"))
                })
                .map(embers_server::termcap_reply)
                .collect(),
            // libghostty answers the size reports itself, through `on_size`.
            TerminalQuery::CellSize => return,
        };
        self.events().replies.extend_from_slice(reply.as_bytes());
    }

    fn cursor_position(&self) -> CursorPosition {
        CursorPosition {
            row: self.get_u16(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_Y),
            col: self.get_u16(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_X),
        }
    }
}

/// One cell as the snapshot needs it.
struct CellView {
    wide: sys::GhosttyCellWide::Type,
    text: String,
    fg: TermColor,
    bg: TermColor,
    underline_color: TermColor,
    attrs: CellAttrs,
    link: Option<String>,
}

impl CellView {
    /// Whether the cell shows anything: a glyph other than a space, or a
    /// background or line drawn without one.
    fn shows(&self) -> bool {
        (!self.text.is_empty() && self.text != " ")
            || self.bg != TermColor::Default
            || self.attrs.contains(CellAttrs::INVERSE)
            || self.attrs.contains(CellAttrs::ANY_UNDERLINE)
            || self.attrs.contains(CellAttrs::STRIKEOUT)
    }
}

fn style_color(color: sys::GhosttyStyleColor) -> TermColor {
    // SAFETY: the tag says which field of the union holds the value.
    unsafe {
        match color.tag {
            sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_PALETTE => {
                TermColor::Indexed(color.value.palette)
            }
            sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_RGB => TermColor::Rgb {
                r: color.value.rgb.r,
                g: color.value.rgb.g,
                b: color.value.rgb.b,
            },
            _ => TermColor::Default,
        }
    }
}

fn style_attrs(style: &sys::GhosttyStyle) -> CellAttrs {
    let mut attrs = CellAttrs::empty();
    for (on, attr) in [
        (style.bold, CellAttrs::BOLD),
        (style.faint, CellAttrs::DIM),
        (style.italic, CellAttrs::ITALIC),
        (style.inverse, CellAttrs::INVERSE),
        (style.invisible, CellAttrs::HIDDEN),
        (style.strikethrough, CellAttrs::STRIKEOUT),
    ] {
        if on {
            attrs.insert(attr);
        }
    }
    use sys::GhosttySgrUnderline as Underline;
    let underline = match style.underline {
        Underline::GHOSTTY_SGR_UNDERLINE_SINGLE => Some(CellAttrs::UNDERLINE),
        Underline::GHOSTTY_SGR_UNDERLINE_DOUBLE => Some(CellAttrs::DOUBLE_UNDERLINE),
        Underline::GHOSTTY_SGR_UNDERLINE_CURLY => Some(CellAttrs::CURLY_UNDERLINE),
        Underline::GHOSTTY_SGR_UNDERLINE_DOTTED => Some(CellAttrs::DOTTED_UNDERLINE),
        Underline::GHOSTTY_SGR_UNDERLINE_DASHED => Some(CellAttrs::DASHED_UNDERLINE),
        _ => None,
    };
    if let Some(underline) = underline {
        attrs.insert(underline);
    }
    attrs
}

/// The URI of the link on the cell `grid_ref` points at.
///
/// # Safety
///
/// `grid_ref` must be valid: no terminal call may have come since it was made.
unsafe fn hyperlink_uri(grid_ref: &sys::GhosttyGridRef) -> Option<String> {
    let mut uri = vec![0_u8; 256];
    loop {
        let mut len = 0;
        // SAFETY: as the caller promises; the buffer holds `uri.len()` bytes.
        let result = unsafe {
            sys::ghostty_grid_ref_hyperlink_uri(grid_ref, uri.as_mut_ptr(), uri.len(), &mut len)
        };
        match result {
            sys::GhosttyResult::GHOSTTY_SUCCESS if len > 0 => {
                return Some(String::from_utf8_lossy(&uri[..len]).into_owned());
            }
            // Too small: `len` is the size needed.
            sys::GhosttyResult::GHOSTTY_OUT_OF_SPACE if len > uri.len() => uri.resize(len, 0),
            _ => return None,
        }
    }
}

/// # Safety
///
/// `T` must be the output type `ghostty_cell_get` documents for `data`.
unsafe fn cell_get<T: Default>(cell: sys::GhosttyCell, data: sys::GhosttyCellData::Type) -> T {
    let mut out = T::default();
    // SAFETY: as the caller promises.
    unsafe { sys::ghostty_cell_get(cell, data, (&raw mut out).cast()) };
    out
}

/// libghostty only fails these calls on arguments this crate gets right, so a
/// failure is a bug here.
fn check(result: sys::GhosttyResult::Type) {
    assert_eq!(
        result,
        sys::GhosttyResult::GHOSTTY_SUCCESS,
        "libghostty-vt call failed"
    );
}

/// The backend's events, from a callback's userdata.
///
/// # Safety
///
/// `userdata` must be the backend's `events` pointer, which libghostty hands
/// to callbacks only during calls made through `&mut` the backend.
unsafe fn events<'a>(userdata: *mut c_void) -> &'a mut Events {
    unsafe { &mut *userdata.cast::<Events>() }
}

unsafe extern "C" fn on_write_pty(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    // SAFETY: libghostty passes our userdata and `len` valid bytes at `data`.
    unsafe {
        events(userdata)
            .replies
            .extend_from_slice(std::slice::from_raw_parts(data, len));
    }
}

unsafe extern "C" fn on_bell(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: libghostty passes our userdata.
    unsafe { events(userdata).bell = true };
}

/// The size reports (`CSI 14/16/18/19 t`), only answered once the pixel size
/// is known, as in the alacritty backend: a program that gets no answer falls
/// back to a guess of its own, while a zero would break its arithmetic.
unsafe extern "C" fn on_size(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttySizeReportSize,
) -> bool {
    // SAFETY: libghostty passes our userdata and a valid out pointer.
    unsafe {
        let size = events(userdata).size;
        let Some(cell) = size.cell_size() else {
            return false;
        };
        *out = sys::GhosttySizeReportSize {
            rows: size.rows,
            columns: size.cols,
            cell_width: u32::from(cell.width),
            cell_height: u32::from(cell.height),
        };
        true
    }
}

/// Device attributes as alacritty reports them: DA1 a VT102 (`CSI ? 6 c`),
/// which programs mostly send to learn that every query before it has been
/// answered.
unsafe extern "C" fn on_device_attributes(
    _terminal: sys::GhosttyTerminal,
    _userdata: *mut c_void,
    out: *mut sys::GhosttyDeviceAttributes,
) -> bool {
    // SAFETY: libghostty passes a valid out pointer.
    unsafe {
        let attributes = &mut *out;
        attributes.primary.conformance_level = 6;
        attributes.primary.num_features = 0;
        attributes.secondary.device_type = 0;
        attributes.secondary.firmware_version = 0;
        attributes.secondary.rom_cartridge = 1;
    }
    true
}

/// OSC 52 writes, kept as text for clients to forward. Reads are never
/// answered (no read callback is set), as in the alacritty backend.
unsafe extern "C" fn on_clipboard_write(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) {
    // SAFETY: libghostty passes our userdata and a write request whose
    // contents stay valid for this call, and expects its reply before the
    // call returns.
    unsafe {
        let request = &*write;
        let contents: &[sys::GhosttyClipboardContent] =
            if request.contents.is_null() || request.contents_len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(request.contents, request.contents_len)
            };
        let text = contents.iter().find_map(|content| {
            let mime = ghostty_str(content.mime);
            mime.starts_with("text/plain")
                .then(|| ghostty_str(content.data))
        });
        let result = match text {
            Some(text) if text.len() <= MAX_CLIPBOARD_WRITE_BYTES => {
                let selection = if request.location
                    == sys::GhosttyClipboardLocation::GHOSTTY_CLIPBOARD_LOCATION_STANDARD
                {
                    ClipboardSelection::Clipboard
                } else {
                    ClipboardSelection::Primary
                };
                events(userdata).clipboard = Some(ClipboardWrite { selection, text });
                sys::GhosttyClipboardWriteResult::GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS
            }
            _ => sys::GhosttyClipboardWriteResult::GHOSTTY_CLIPBOARD_WRITE_RESULT_DENIED,
        };
        if let Some(reply) = request.reply {
            let answer = sys::GhosttyClipboardWriteReply {
                size: size_of::<sys::GhosttyClipboardWriteReply>(),
                result,
                remember: false,
            };
            reply(write, &answer);
        }
    }
}

/// # Safety
///
/// `string` must point to `len` readable bytes, or be null.
unsafe fn ghostty_str(string: sys::GhosttyString) -> String {
    if string.ptr.is_null() {
        return String::new();
    }
    // SAFETY: as the caller promises.
    let bytes = unsafe { std::slice::from_raw_parts(string.ptr, string.len) };
    String::from_utf8_lossy(bytes).into_owned()
}
