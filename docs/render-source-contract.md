## Render-source contract

Phase 8 locks down which server surfaces the client uses for terminal rendering.

### Authoritative sources

The server remains authoritative for both layout and terminal state.

- `SessionSnapshot` provides layout topology plus durable buffer metadata such as title, activity, attachment, and PTY size.
- `VisibleSnapshotResponse` provides the current visible terminal surface for one buffer, including styled visible lines, cursor state, viewport position, alternate-screen mode, and other terminal-mode flags.
- Full capture and scrollback slices stay on-demand APIs and are not part of the normal render loop.

The client does not consume terminal diffs. It renders from full visible snapshots, with `RenderInvalidated` acting as a hint that a buffer should be refreshed before the next user-visible render.

### Styled visible lines

Each visible line is a `SnapshotLine { text, runs }`: the plain text plus run-length style annotations (`StyledRun`) carrying per-cell foreground/background color, attributes, and underline color. Colors are semantic — named/indexed ANSI colors travel as `Indexed(n)` so the outer terminal's palette resolves them, and only true-color sequences carry explicit RGB. An empty `runs` vector means the whole line is default-styled, so plain buffers ship (and render) exactly as before. Scrollback slices used for scrolled-back views carry the same styling; full capture and helper/persistence surfaces stay plain text. The no-diff / full-snapshot model is otherwise unchanged.

### Underline style and color

Underlines keep their shape. `CellAttrs` has one exclusive flag per style — `UNDERLINE`, `DOUBLE_UNDERLINE`, `CURLY_UNDERLINE`, `DOTTED_UNDERLINE`, `DASHED_UNDERLINE` — matching SGR `4:1` to `4:5`. The underline color (SGR 58) travels on the run as `underline_color`, where `Default` means the underline follows the foreground.

On the wire the fixed-size `StyledRunWire` struct is unchanged: the style flags fit in its existing `attrs` bits, and underline colors go in a sparse `underline_colors` vector on `StyledLine` with one 8-byte entry per run that sets a color. Frames from programs that never set an underline color are byte-identical to before. In keeper JSON a default `underline_color` is omitted.

The client renderer re-emits these to the host terminal: `CSI 4 m` for a single underline, `CSI 4:n m` for the other styles, and `CSI 58:5:n m` or `CSI 58:2::r:g:b m` for the color. The host terminal must understand colon sub-parameters to draw them; Embers does not translate them for terminals that don't.

### Hyperlinks

OSC 8 hyperlinks survive from the pane to the host terminal, so links printed by programs (`ls --hyperlink`, compilers, `gh`, Neovim plugins) stay clickable. Each `SnapshotLine` carries a `links` table of `Hyperlink { id, uri }`, and each run's `link` field is `0` for no link or a 1-based index into that table (`SnapshotLine::link` resolves it). The table is per line rather than per snapshot so lines stay self-contained when they are sliced out of scrollback; a link wrapped across lines appears in each line's table and keeps its `id`, which the host terminal uses to treat the pieces as one link.

Linked text counts as styled, so a line whose only styling is a link still has `runs`. On the wire, `StyledLine` gains a `links` vector and a `run_links` vector of `ushort` parallel to `runs`; both are absent unless a run on that line is linked, so link-free frames are unchanged. Keeper JSON omits `link` when zero and `links` when empty, and scrollback slices carry the tables in a parallel `links` field.

The renderer keeps each link on the grid cell rather than in the cell style, so selection and search overlays keep it while anything drawn over the cell (borders, hint labels, popups) drops it. It emits `OSC 8 ; id=<id> ; <uri> ST` where a link starts and `OSC 8 ; ; ST` where it ends, and always closes an open link at the end of a row. Control characters are stripped from the id and URI, and `:`/`;` from the id, so pane content cannot end the sequence early and inject terminal input.

### Freshness expectations

`RenderInvalidated` means the visible snapshot for that buffer may be stale. The client refreshes invalidated visible leaves (leaf nodes in the layout tree corresponding to visible buffers) before rendering and then updates the display so updated titles, alternate-screen flags, and visible lines are used together.

For event handling, the client also refreshes the affected `BufferRecord` before dispatching `RenderInvalidated` hooks. That keeps metadata-only consumers such as bell automation aligned with the latest server state.

### Metadata synchronization

Visible snapshots may carry title and mode changes that affect UI immediately. Buffer metadata still lives on the durable `BufferRecord`, so activity, bell state, and detached-buffer discovery remain queryable even when a buffer is hidden.

Hidden buffers do not eagerly fetch fresh visible snapshots just because they were invalidated. Their visible state is refreshed when they become visible or when a caller explicitly requests capture. Their metadata, however, continues to flow through buffer/session refresh paths.

### Detached buffers

Detached buffers are discovered through `BufferRequest::List` / `Get`, and their visible surface is queried explicitly through the same capture endpoints as attached buffers. That keeps detached previews and background metadata within the same contract as attached terminal runtimes.
