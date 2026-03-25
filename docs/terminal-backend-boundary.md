# Terminal backend boundary

This note defines the PTY-to-render pipeline boundary Embers relies on today.

## Pipeline

The active PTY pipeline is:

```text
PTY bytes
  -> RawByteRouter
  -> TerminalBackend
  -> snapshot / metadata / damage
  -> protocol responses and render invalidation
```

For live PTY buffers, that pipeline runs inside the runtime keeper (`KeeperSurface` in `crates/embers-server/src/buffer_runtime.rs`). The server process talks to the keeper over the runtime socket and does not own a second terminal parser.

## Raw routing seam

`RawByteRouter` is the only layer allowed to inspect or rewrite raw terminal bytes before they hit the backend.

Today it is intentionally minimal:

- input routing is passthrough
- output routing forwards bytes directly to the backend

That seam exists so future work can add:

- protocol-aware passthrough decisions
- special-case interception
- metadata extraction that must happen before backend ingestion

The router should not own terminal screen state, scrollback, or view/layout concerns.

## Backend ownership

`TerminalBackend` owns terminal emulation state:

- ANSI/terminal parsing
- primary/alternate screen state
- cursor state
- scrollback capture
- visible snapshot generation
- damage tracking
- terminal mode reporting (alternate screen, mouse, focus, bracketed paste)

Backends must be able to:

- ingest output bytes
- resize
- produce a visible snapshot
- produce full capture / scrollback slices
- surface metadata
- surface one-shot activity and damage signals

## Metadata outside the backend

The server still owns buffer-level metadata records:

- buffer title field mirrored onto the `Buffer`
- activity/bell state mirrored onto the `Buffer`
- last snapshot sequence on the `Buffer`
- render invalidation events

The backend reports the current terminal metadata; the server persists the last observed values onto the durable buffer record.

## Alternate-screen policy

Alternate-screen state is owned by the backend.

- `visible_snapshot` always reflects the currently active screen
- the `alternate_screen` mode bit tells clients whether the visible snapshot is from the alternate buffer
- when alternate screen exits, the visible snapshot returns to the primary screen state managed by the backend
- full capture and scrollback are backend-defined state, not layout-defined state

In practice, Embers currently inherits alternate-screen capture semantics from the alacritty backend implementation, and the tests lock that behavior down at the backend boundary.

## Code anchors

- Raw routing + backend trait: `crates/embers-server/src/terminal_backend.rs`
- Keeper-owned surface: `crates/embers-server/src/buffer_runtime.rs`
- Server/runtime wiring: `crates/embers-server/src/server.rs`
