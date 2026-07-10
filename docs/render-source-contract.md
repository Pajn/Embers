## Render-source contract

Phase 8 locks down which server surfaces the client uses for terminal rendering.

### Authoritative sources

The server remains authoritative for both layout and terminal state.

- `SessionSnapshot` provides layout topology plus durable buffer metadata such as title, activity, attachment, and PTY size.
- `VisibleSnapshotResponse` provides the current visible terminal surface for one buffer, including styled visible lines, cursor state, viewport position, alternate-screen mode, and other terminal-mode flags.
- Full capture and scrollback slices stay on-demand APIs and are not part of the normal render loop.

The client does not consume terminal diffs. It renders from full visible snapshots, with `RenderInvalidated` acting as a hint that a buffer should be refreshed before the next user-visible render.

### Styled visible lines

Each visible line is a `SnapshotLine { text, runs }`: the plain text plus run-length style annotations (`StyledRun`) carrying per-cell foreground/background color and attributes. Colors are semantic — named/indexed ANSI colors travel as `Indexed(n)` so the outer terminal's palette resolves them, and only true-color sequences carry explicit RGB. An empty `runs` vector means the whole line is default-styled, so plain buffers ship (and render) exactly as before. Scrollback slices used for scrolled-back views carry the same styling; full capture and helper/persistence surfaces stay plain text. The no-diff / full-snapshot model is otherwise unchanged.

### Freshness expectations

`RenderInvalidated` means the visible snapshot for that buffer may be stale. The client refreshes invalidated visible leaves (leaf nodes in the layout tree corresponding to visible buffers) before rendering and then updates the display so updated titles, alternate-screen flags, and visible lines are used together.

For event handling, the client also refreshes the affected `BufferRecord` before dispatching `RenderInvalidated` hooks. That keeps metadata-only consumers such as bell automation aligned with the latest server state.

### Metadata synchronization

Visible snapshots may carry title and mode changes that affect UI immediately. Buffer metadata still lives on the durable `BufferRecord`, so activity, bell state, and detached-buffer discovery remain queryable even when a buffer is hidden.

Hidden buffers do not eagerly fetch fresh visible snapshots just because they were invalidated. Their visible state is refreshed when they become visible or when a caller explicitly requests capture. Their metadata, however, continues to flow through buffer/session refresh paths.

### Detached buffers

Detached buffers are discovered through `BufferRequest::List` / `Get`, and their visible surface is queried explicitly through the same capture endpoints as attached buffers. That keeps detached previews and background metadata within the same contract as attached terminal runtimes.
