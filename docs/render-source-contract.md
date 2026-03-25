## Render-source contract

Phase 8 locks down which server surfaces the client uses for terminal rendering.

### Authoritative sources

The server remains authoritative for both layout and terminal state.

- `SessionSnapshot` provides layout topology plus durable buffer metadata such as title, activity, attachment, and PTY size.
- `VisibleSnapshotResponse` provides the current visible terminal surface for one buffer, including visible lines, cursor state, viewport position, alternate-screen mode, and other terminal-mode flags.
- Full capture and scrollback slices stay on-demand APIs and are not part of the normal render loop.

The client does not consume terminal diffs. It renders from full visible snapshots, with `RenderInvalidated` acting as a hint that a buffer should be refreshed before the next user-visible render.

### Freshness expectations

`RenderInvalidated` means the visible snapshot for that buffer may be stale. The configured client refreshes invalidated visible leaves before rendering and re-projects the presentation after those refreshes so updated titles, alternate-screen flags, and visible lines are used together.

For event handling, the client also refreshes the affected `BufferRecord` before dispatching render-invalidated hooks. That keeps metadata-only consumers such as bell automation aligned with the latest server state.

### Metadata synchronization

Visible snapshots may carry title and mode changes that affect UI immediately. Buffer metadata still lives on the durable `BufferRecord`, so activity, bell state, and detached-buffer discovery remain queryable even when a buffer is hidden.

Hidden buffers do not eagerly fetch fresh visible snapshots just because they were invalidated. Their visible state is refreshed when they become visible or when a caller explicitly requests capture. Their metadata, however, continues to flow through buffer/session refresh paths.

### Detached buffers

Detached buffers are discovered through `BufferRequest::List` / `Get`, and their visible surface is queried explicitly through the same capture endpoints as attached buffers. That keeps detached previews and background metadata within the same contract as attached terminal runtimes.
