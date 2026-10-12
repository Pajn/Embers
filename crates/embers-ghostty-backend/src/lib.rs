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
    /// The viewport and cursor as of the last snapshot, read through
    /// `row_iterator` and `row_cells`, which are kept to be reused.
    render_state: sys::GhosttyRenderState,
    row_iterator: sys::GhosttyRenderStateRowIterator,
    row_cells: sys::GhosttyRenderStateRowCells,
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

/// What a terminal holds in memory: see [`GhosttyTerminalBackend::memory_usage`].
#[derive(Clone, Copy, Debug)]
pub struct MemoryUsage {
    /// Physical memory its screens' pages use, compressed pages counted at
    /// their compressed size.
    pub resident_bytes: u64,
    pub compressed_pages: u64,
    pub compression_supported: bool,
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
        let mut row_iterator = ptr::null_mut();
        let mut row_cells = ptr::null_mut();
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
            check(sys::ghostty_render_state_row_iterator_new(
                ptr::null(),
                &mut row_iterator,
            ));
            check(sys::ghostty_render_state_row_cells_new(
                ptr::null(),
                &mut row_cells,
            ));
        }
        let events = Box::into_raw(Box::new(Events {
            size,
            ..Events::default()
        }));
        let mut backend = Self {
            terminal,
            render_state,
            row_iterator,
            row_cells,
            events,
            last_ingest_replies: 0,
            dirty: true,
        };
        backend.configure(max_scrollback_lines);
        backend.resize(size);
        backend
    }

    /// What the terminal holds in memory, as libghostty counts it.
    pub fn memory_usage(&self) -> MemoryUsage {
        let mut usage = sys::GhosttyTerminalMemoryUsage {
            size: size_of::<sys::GhosttyTerminalMemoryUsage>(),
            ..Default::default()
        };
        // SAFETY: the memory usage's output type is the sized struct given.
        check(unsafe {
            sys::ghostty_terminal_get(
                self.terminal,
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_MEMORY_USAGE,
                (&raw mut usage).cast(),
            )
        });
        MemoryUsage {
            resident_bytes: usage.primary_resident_bytes + usage.alternate_resident_bytes,
            compressed_pages: usage.primary_compressed_pages + usage.alternate_compressed_pages,
            compression_supported: usage.compression_supported,
        }
    }

    /// Compress the scrollback libghostty finds worth compressing, in one
    /// pass that can stall on a long scrollback. Reading compressed lines
    /// restores them. Whether this platform supports it.
    pub fn compress_scrollback(&mut self) -> bool {
        let mut result =
            sys::GhosttyTerminalCompressionResult::GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED;
        // SAFETY: a plain call on the terminal this value owns.
        check(unsafe {
            sys::ghostty_terminal_compress(
                self.terminal,
                sys::GhosttyTerminalCompressionMode::GHOSTTY_TERMINAL_COMPRESSION_MODE_FULL,
                &mut result,
            )
        });
        result != sys::GhosttyTerminalCompressionResult::GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED
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

    /// One cell anywhere on the screen, scrollback included, through a grid
    /// ref: slower per cell than the render state, which only holds the
    /// viewport.
    /// Bring the render state up to date with the terminal: the viewport and
    /// cursor read next are the terminal's now.
    fn update_render_state(&self) {
        // SAFETY: the render state belongs to this value, and nothing it
        // handed out (rows, cells) outlives the reads that follow an update.
        check(unsafe { sys::ghostty_render_state_update(self.render_state, self.terminal) });
    }

    /// The viewport's rows, from the render state as last updated.
    fn viewport_rows(&self) -> Vec<Row> {
        use sys::GhosttyRenderStateRowCellsData as Data;
        let mut rows = Vec::new();
        // SAFETY: the iterator and cells are this value's, populated from its
        // render state, and read before it is updated again; every out
        // pointer is valid for its documented type.
        unsafe {
            let mut iterator = self.row_iterator;
            check(sys::ghostty_render_state_get(
                self.render_state,
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut iterator).cast(),
            ));
            let mut utf8 = vec![0_u8; 64];
            let default_style = default_style();
            let plain = Pen::of(&default_style);
            let mut y: u32 = 0;
            while sys::ghostty_render_state_row_iterator_next(iterator) {
                let mut cells = self.row_cells;
                check(sys::ghostty_render_state_row_get(
                    iterator,
                    sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&raw mut cells).cast(),
                ));
                // The row's raw cells in one call, decoded here; libghostty is
                // only asked for what a raw cell does not hold: styles,
                // clusters of several codepoints and links.
                let mut view = sys::GhosttyCellsView {
                    ptr: ptr::null(),
                    len: 0,
                };
                check(sys::ghostty_render_state_row_get(
                    iterator,
                    sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW,
                    (&raw mut view).cast(),
                ));
                let raw_cells: &[sys::GhosttyCell] = if view.ptr.is_null() {
                    &[]
                } else {
                    std::slice::from_raw_parts(view.ptr, view.len)
                };
                let mut row = Row {
                    cells: Vec::with_capacity(raw_cells.len()),
                    ..Row::default()
                };
                // Style ids are only unique within a page, and a row lives in
                // one, so a style is reused only within its row.
                let mut last_pen: Option<(sys::GhosttyStyleId, Pen)> = None;
                for (x, &raw) in raw_cells.iter().enumerate() {
                    let x = u16::try_from(x).unwrap_or(u16::MAX);
                    let fields = RawCell::decode(raw);
                    let pen = match last_pen {
                        _ if !fields.has_styling => plain,
                        Some((id, pen)) if id == fields.style_id => pen,
                        _ => {
                            let mut style = default_style;
                            check(sys::ghostty_render_state_row_cells_select(cells, x));
                            check(sys::ghostty_render_state_row_cells_get(
                                cells,
                                Data::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                                (&raw mut style).cast(),
                            ));
                            let pen = Pen::of(&style);
                            last_pen = Some((fields.style_id, pen));
                            pen
                        }
                    };

                    let text = match fields.single_char() {
                        Some(text) => text,
                        None => {
                            check(sys::ghostty_render_state_row_cells_select(cells, x));
                            let mut cluster = String::new();
                            loop {
                                let mut buffer = sys::GhosttyBuffer {
                                    ptr: utf8.as_mut_ptr(),
                                    cap: utf8.len(),
                                    len: 0,
                                };
                                let result = sys::ghostty_render_state_row_cells_get(
                                    cells,
                                    Data::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                                    (&raw mut buffer).cast(),
                                );
                                if result == sys::GhosttyResult::GHOSTTY_OUT_OF_SPACE {
                                    utf8.resize(buffer.len, 0);
                                    continue;
                                }
                                if result == sys::GhosttyResult::GHOSTTY_SUCCESS {
                                    cluster.push_str(&String::from_utf8_lossy(&utf8[..buffer.len]));
                                }
                                break;
                            }
                            row.cluster(cluster)
                        }
                    };

                    // The render state holds no hyperlinks; the few linked
                    // cells are looked up on the terminal.
                    let link = if fields.has_link {
                        let uri = self.viewport_link(x, y);
                        row.link(uri)
                    } else {
                        0
                    };
                    row.cells.push(CellView::new(&fields, pen, text, link));
                }
                rows.push(row);
                y += 1;
            }
        }
        rows
    }

    /// The rows from `start` to `end`, counted from the top of the
    /// scrollback, read a viewport at a time through the render state, which
    /// is many times faster than a cell at a time through grid refs.
    ///
    /// Scrolls libghostty's viewport there and back to the bottom, where
    /// Embers keeps it: Embers scrolls its own copies of the screen, never
    /// libghostty's.
    fn screen_rows(&self, start: u64, end: u64) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut line = start;
        while line < end {
            self.scroll_viewport(
                sys::GhosttyTerminalScrollViewportTag::GHOSTTY_SCROLL_VIEWPORT_ROW,
                sys::GhosttyTerminalScrollViewportValue {
                    row: usize::try_from(line).unwrap_or(usize::MAX),
                },
            );
            self.update_render_state();
            // Near the bottom the viewport stops short of `line`.
            let top = self.scrollbar().offset;
            let page = self.viewport_rows();
            let page_end = top + page.len() as u64;
            if page_end <= line {
                break;
            }
            rows.extend(
                page.into_iter()
                    .skip(usize::try_from(line - top).unwrap_or(usize::MAX))
                    .take(usize::try_from(end - line).unwrap_or(usize::MAX)),
            );
            line = page_end;
        }
        self.scroll_viewport(
            sys::GhosttyTerminalScrollViewportTag::GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            sys::GhosttyTerminalScrollViewportValue { row: 0 },
        );
        rows
    }

    fn scroll_viewport(
        &self,
        tag: sys::GhosttyTerminalScrollViewportTag::Type,
        value: sys::GhosttyTerminalScrollViewportValue,
    ) {
        // SAFETY: a plain call on the terminal this value owns; nothing read
        // from the render state is held across it.
        unsafe {
            sys::ghostty_terminal_scroll_viewport(
                self.terminal,
                sys::GhosttyTerminalScrollViewport { tag, value },
            );
        }
    }

    fn viewport_link(&self, x: u16, y: u32) -> Option<String> {
        let point = sys::GhosttyPoint {
            tag: sys::GhosttyPointTag::GHOSTTY_POINT_TAG_VIEWPORT,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x, y },
            },
        };
        let mut grid_ref = sys::GhosttyGridRef {
            size: size_of::<sys::GhosttyGridRef>(),
            ..Default::default()
        };
        // SAFETY: the grid ref is read straight away, before any other
        // terminal call.
        unsafe {
            (sys::ghostty_terminal_grid_ref(self.terminal, point, &mut grid_ref)
                == sys::GhosttyResult::GHOSTTY_SUCCESS)
                .then(|| hyperlink_uri(&grid_ref))
                .flatten()
        }
    }

    /// The cursor, from the render state as last updated.
    fn render_state_cursor(&self) -> Option<CursorState> {
        use sys::GhosttyRenderStateData as Data;
        // SAFETY: each output type is the one documented for its data.
        unsafe {
            let get = |data, out: *mut c_void| {
                sys::ghostty_render_state_get(self.render_state, data, out)
                    == sys::GhosttyResult::GHOSTTY_SUCCESS
            };
            let mut visible = false;
            let mut in_viewport = false;
            if !get(
                Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
                (&raw mut visible).cast(),
            ) || !visible
                || !get(
                    Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
                    (&raw mut in_viewport).cast(),
                )
                || !in_viewport
            {
                return None;
            }
            let (mut x, mut y, mut wide_tail) = (0_u16, 0_u16, false);
            let mut style = sys::GhosttyRenderStateCursorVisualStyle::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK;
            get(
                Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
                (&raw mut x).cast(),
            );
            get(
                Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
                (&raw mut y).cast(),
            );
            get(
                Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
                (&raw mut wide_tail).cast(),
            );
            get(
                Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
                (&raw mut style).cast(),
            );
            use sys::GhosttyRenderStateCursorVisualStyle as Style;
            let shape = match style {
                Style::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Beam,
                Style::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => CursorShape::Underline,
                _ => CursorShape::Block,
            };
            Some(CursorState {
                // On the right half of a wide character, the cursor is drawn
                // on the character.
                position: CursorPosition {
                    row: y,
                    col: if wide_tail { x.saturating_sub(1) } else { x },
                },
                shape,
            })
        }
    }

    fn cursor_state(&self) -> Option<CursorState> {
        self.update_render_state();
        self.render_state_cursor()
    }

    fn metadata_with(&self, cursor: Option<CursorState>) -> BackendMetadata {
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
            cursor,
        }
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
            sys::ghostty_render_state_row_cells_free(self.row_cells);
            sys::ghostty_render_state_row_iterator_free(self.row_iterator);
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
        // One update for both the cursor and the rows.
        self.update_render_state();
        let metadata = self.metadata_with(self.render_state_cursor());
        TerminalSnapshot {
            sequence,
            size,
            cursor: metadata.cursor,
            lines: self.viewport_rows().iter().map(snapshot_line).collect(),
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
        let mut lines: Vec<String> = self
            .screen_rows(0, self.total_lines())
            .iter()
            .map(|row| snapshot_line(row).text)
            .collect();
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
        BackendScrollbackSlice {
            start_line,
            total_lines,
            lines: self
                .screen_rows(start_line, end_line)
                .iter()
                .map(snapshot_line)
                .collect(),
        }
    }

    fn metadata(&self) -> BackendMetadata {
        self.metadata_with(self.cursor_state())
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

/// A row's cells as a snapshot line, the way the alacritty backend builds one.
fn snapshot_line(row: &Row) -> SnapshotLine {
    let cells = &row.cells;
    let content_len = cells
        .iter()
        .rposition(CellView::shows)
        .map_or(0, |last| last + 1);

    let mut text = String::with_capacity(content_len);
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut links: Vec<Hyperlink> = Vec::new();
    // The line's link index for each of the row's links, once used.
    let mut line_links: Vec<u16> = vec![0; row.links.len()];
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
        if blank {
            text.push(' ');
        } else {
            row.push_text(cell.text, &mut text);
        }
        let len = u32::try_from(text.len() - start).unwrap_or(u32::MAX);

        let link = match usize::from(cell.link).checked_sub(1) {
            None => 0,
            Some(index) => {
                if line_links[index] == 0 {
                    links.push(Hyperlink {
                        id: None,
                        uri: row.links[index].clone(),
                    });
                    line_links[index] = u16::try_from(links.len()).unwrap_or(u16::MAX);
                }
                line_links[index]
            }
        };
        let pen = cell.pen;
        if !pen.is_plain() || link != 0 {
            styled = true;
        }
        match runs.last_mut() {
            Some(last)
                if last.fg == pen.fg
                    && last.bg == pen.bg
                    && last.attrs == pen.attrs
                    && last.underline_color == pen.underline_color
                    && last.link == link =>
            {
                last.len += len;
            }
            _ => runs.push(StyledRun {
                len,
                fg: pen.fg,
                bg: pen.bg,
                attrs: pen.attrs,
                underline_color: pen.underline_color,
                link,
            }),
        }
    }
    if !styled {
        runs.clear();
    }
    SnapshotLine { text, runs, links }
}

/// The parts of a raw cell the snapshot needs, read in one call.
struct RawCell {
    raw: sys::GhosttyCell,
    wide: sys::GhosttyCellWide::Type,
    content: sys::GhosttyCellContentTag::Type,
    has_text: bool,
    has_styling: bool,
    has_link: bool,
    codepoint: u32,
    style_id: sys::GhosttyStyleId,
}

/// Where each field of a raw cell lies in its 64 bits, as `(lowest bit,
/// width)`, at the pinned Ghostty commit. libghostty keeps the layout out of
/// its ABI and describes it in `ghostty_type_json` instead; a test checks
/// these against that description, so a new pin that moves a field fails it.
mod cell_bits {
    pub const CONTENT_TAG: (u32, u32) = (0, 2);
    pub const CONTENT: (u32, u32) = (2, 24);
    pub const CODEPOINT_WIDTH: u32 = 21;
    pub const STYLE_ID: (u32, u32) = (26, 16);
    pub const WIDE: (u32, u32) = (42, 2);
    pub const HYPERLINK: (u32, u32) = (45, 1);
}

fn bits(raw: sys::GhosttyCell, (lsb, width): (u32, u32)) -> u64 {
    (raw >> lsb) & ((1 << width) - 1)
}

impl RawCell {
    /// Decode `raw` without calling into libghostty: reading cell by cell
    /// through calls costs more than everything else a snapshot does.
    fn decode(raw: sys::GhosttyCell) -> Self {
        use sys::GhosttyCellContentTag as Content;
        let content = bits(raw, cell_bits::CONTENT_TAG) as sys::GhosttyCellContentTag::Type;
        let codepoint =
            (bits(raw, cell_bits::CONTENT) & ((1 << cell_bits::CODEPOINT_WIDTH) - 1)) as u32;
        let style_id = bits(raw, cell_bits::STYLE_ID) as sys::GhosttyStyleId;
        Self {
            raw,
            wide: bits(raw, cell_bits::WIDE) as sys::GhosttyCellWide::Type,
            content,
            has_text: (content == Content::GHOSTTY_CELL_CONTENT_CODEPOINT
                || content == Content::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME)
                && codepoint != 0,
            has_styling: style_id != 0,
            has_link: bits(raw, cell_bits::HYPERLINK) != 0,
            codepoint,
            style_id,
        }
    }

    /// The cell's text, when it is a single character; a cluster of several
    /// needs reading from where the cell lives.
    fn single_char(&self) -> Option<CellText> {
        if !self.has_text {
            return Some(CellText::Empty);
        }
        (self.content == sys::GhosttyCellContentTag::GHOSTTY_CELL_CONTENT_CODEPOINT)
            .then(|| char::from_u32(self.codepoint).map_or(CellText::Empty, CellText::Char))
    }

    /// The background of a blank cell erased in a background color, which
    /// holds the color instead of a style.
    fn erased_background(&self) -> Option<TermColor> {
        use sys::GhosttyCellContentTag as Content;
        let color = bits(self.raw, cell_bits::CONTENT);
        match self.content {
            Content::GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => Some(TermColor::Indexed(color as u8)),
            Content::GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => Some(TermColor::Rgb {
                r: color as u8,
                g: (color >> 8) as u8,
                b: (color >> 16) as u8,
            }),
            _ => None,
        }
    }
}

fn default_style() -> sys::GhosttyStyle {
    let mut style = sys::GhosttyStyle {
        size: size_of::<sys::GhosttyStyle>(),
        ..Default::default()
    };
    // SAFETY: fills the style it is given.
    unsafe { sys::ghostty_style_default(&mut style) };
    style
}

/// A cell's text: most cells hold one character, which needs no allocation;
/// a cluster of several is kept in its row's [`Row::clusters`].
#[derive(Clone, Copy)]
enum CellText {
    Empty,
    Char(char),
    Cluster(u32),
}

/// A style as the snapshot needs it, converted once per style in a row.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Pen {
    fg: TermColor,
    bg: TermColor,
    underline_color: TermColor,
    attrs: CellAttrs,
}

impl Pen {
    fn of(style: &sys::GhosttyStyle) -> Self {
        Self {
            fg: style_color(style.fg_color),
            bg: style_color(style.bg_color),
            underline_color: style_color(style.underline_color),
            attrs: style_attrs(style),
        }
    }

    fn is_plain(&self) -> bool {
        self.fg == TermColor::Default
            && self.bg == TermColor::Default
            && self.underline_color == TermColor::Default
            && self.attrs == CellAttrs::empty()
    }

    /// Whether a cell in this pen shows without a glyph: a background, or a
    /// line drawn through or under it.
    fn shows_alone(&self) -> bool {
        self.bg != TermColor::Default
            || self.attrs.contains(CellAttrs::INVERSE)
            || self.attrs.contains(CellAttrs::ANY_UNDERLINE)
            || self.attrs.contains(CellAttrs::STRIKEOUT)
    }
}

/// One cell as the snapshot needs it.
#[derive(Clone, Copy)]
struct CellView {
    wide: sys::GhosttyCellWide::Type,
    text: CellText,
    pen: Pen,
    /// 1-based index into the row's [`Row::links`]; 0 for none.
    link: u16,
}

impl CellView {
    fn new(cell: &RawCell, pen: Pen, text: CellText, link: u16) -> Self {
        let mut pen = pen;
        if let Some(bg) = cell.erased_background() {
            pen.bg = bg;
        }
        Self {
            wide: cell.wide,
            text,
            pen,
            link,
        }
    }

    /// Whether the cell shows anything: a glyph other than a space, or a
    /// background or line drawn without one.
    fn shows(&self) -> bool {
        let glyph = match self.text {
            CellText::Empty => false,
            CellText::Char(ch) => ch != ' ',
            CellText::Cluster(_) => true,
        };
        glyph || self.pen.shows_alone()
    }
}

/// A row's cells, with the clusters and link URIs they point into.
#[derive(Default)]
struct Row {
    cells: Vec<CellView>,
    clusters: Vec<String>,
    links: Vec<String>,
}

impl Row {
    fn cluster(&mut self, cluster: String) -> CellText {
        self.clusters.push(cluster);
        CellText::Cluster(u32::try_from(self.clusters.len() - 1).unwrap_or(u32::MAX))
    }

    fn link(&mut self, uri: Option<String>) -> u16 {
        let Some(uri) = uri else {
            return 0;
        };
        let index = match self.links.iter().position(|known| *known == uri) {
            Some(index) => index,
            None => {
                self.links.push(uri);
                self.links.len() - 1
            }
        };
        u16::try_from(index + 1).unwrap_or(u16::MAX)
    }

    fn push_text(&self, text: CellText, out: &mut String) {
        match text {
            CellText::Empty => out.push(' '),
            CellText::Char(ch) => out.push(ch),
            CellText::Cluster(index) => out.push_str(&self.clusters[index as usize]),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// # Safety
    ///
    /// `T` must be the output type `ghostty_cell_get` documents for `data`.
    unsafe fn cell_get<T: Default>(cell: sys::GhosttyCell, data: sys::GhosttyCellData::Type) -> T {
        let mut out = T::default();
        // SAFETY: as the caller promises.
        unsafe { sys::ghostty_cell_get(cell, data, (&raw mut out).cast()) };
        out
    }

    #[test]
    fn the_cell_layout_is_the_one_libghostty_describes() {
        // SAFETY: libghostty returns a static, NUL-terminated manifest.
        let manifest = unsafe { std::ffi::CStr::from_ptr(sys::ghostty_type_json()) };
        let manifest: serde_json::Value =
            serde_json::from_slice(manifest.to_bytes()).expect("the manifest is JSON");
        let cell = &manifest["types"]["GhosttyCell"]["bits"];
        let field = |value: &serde_json::Value| {
            (
                value["lsb"].as_u64().expect("lsb") as u32,
                value["width"].as_u64().expect("width") as u32,
            )
        };
        assert_eq!(field(&cell["content_tag"]), cell_bits::CONTENT_TAG);
        assert_eq!(field(&cell["content"]), cell_bits::CONTENT);
        assert_eq!(
            field(&cell["content"]["arms"]["CODEPOINT"]["bits"]["codepoint"]),
            (0, cell_bits::CODEPOINT_WIDTH)
        );
        assert_eq!(field(&cell["style_id"]), cell_bits::STYLE_ID);
        assert_eq!(field(&cell["wide"]), cell_bits::WIDE);
        assert_eq!(field(&cell["hyperlink"]), cell_bits::HYPERLINK);
    }

    #[test]
    fn decoded_cells_match_what_libghostty_reports() {
        use sys::GhosttyCellData as Data;
        let mut backend = GhosttyTerminalBackend::new(PtySize::new(12, 4), 0);
        backend.ingest_bytes(
            "a\x1b[1;31mb漢e\u{301}\x1b[0m\x1b]8;;https://e.example\x1b\\L\x1b]8;;\x1b\\\r\n\
             \x1b[44m\x1b[K\r\n\x1b[48;2;1;2;3m\x1b[K\x1b[0m👍"
                .as_bytes(),
        );
        let mut checked = 0;
        for y in 0..4 {
            for x in 0..12 {
                let point = sys::GhosttyPoint {
                    tag: sys::GhosttyPointTag::GHOSTTY_POINT_TAG_ACTIVE,
                    value: sys::GhosttyPointValue {
                        coordinate: sys::GhosttyPointCoordinate { x, y },
                    },
                };
                let mut grid_ref = sys::GhosttyGridRef {
                    size: size_of::<sys::GhosttyGridRef>(),
                    ..Default::default()
                };
                // SAFETY: each output type is the one documented for its data,
                // and the grid ref is read before any other terminal call.
                unsafe {
                    check(sys::ghostty_terminal_grid_ref(
                        backend.terminal,
                        point,
                        &mut grid_ref,
                    ));
                    let mut raw: sys::GhosttyCell = 0;
                    check(sys::ghostty_grid_ref_cell(&grid_ref, &mut raw));
                    let decoded = RawCell::decode(raw);
                    let at = format!("cell {x},{y}");
                    assert_eq!(
                        decoded.wide,
                        cell_get::<sys::GhosttyCellWide::Type>(raw, Data::GHOSTTY_CELL_DATA_WIDE),
                        "{at}"
                    );
                    assert_eq!(
                        decoded.content,
                        cell_get::<sys::GhosttyCellContentTag::Type>(
                            raw,
                            Data::GHOSTTY_CELL_DATA_CONTENT_TAG
                        ),
                        "{at}"
                    );
                    assert_eq!(
                        decoded.has_text,
                        cell_get::<bool>(raw, Data::GHOSTTY_CELL_DATA_HAS_TEXT),
                        "{at}"
                    );
                    assert_eq!(
                        decoded.has_styling,
                        cell_get::<bool>(raw, Data::GHOSTTY_CELL_DATA_HAS_STYLING),
                        "{at}"
                    );
                    assert_eq!(
                        decoded.has_link,
                        cell_get::<bool>(raw, Data::GHOSTTY_CELL_DATA_HAS_HYPERLINK),
                        "{at}"
                    );
                    assert_eq!(
                        decoded.style_id,
                        cell_get::<sys::GhosttyStyleId>(raw, Data::GHOSTTY_CELL_DATA_STYLE_ID),
                        "{at}"
                    );
                    if decoded.has_text {
                        assert_eq!(
                            decoded.codepoint,
                            cell_get::<u32>(raw, Data::GHOSTTY_CELL_DATA_CODEPOINT),
                            "{at}"
                        );
                    }
                    use sys::GhosttyCellContentTag as Content;
                    let erased = match decoded.content {
                        Content::GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => Some(TermColor::Indexed(
                            cell_get(raw, Data::GHOSTTY_CELL_DATA_COLOR_PALETTE),
                        )),
                        Content::GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
                            let rgb: sys::GhosttyColorRgb =
                                cell_get(raw, Data::GHOSTTY_CELL_DATA_COLOR_RGB);
                            Some(TermColor::Rgb {
                                r: rgb.r,
                                g: rgb.g,
                                b: rgb.b,
                            })
                        }
                        _ => None,
                    };
                    assert_eq!(decoded.erased_background(), erased, "{at}");
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 48);
    }
}
