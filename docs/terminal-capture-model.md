# Terminal capture model

This note defines the terminal snapshot and capture semantics Embers exposes from PTY-backed buffers.

## Terminology

- durable buffer runtime - the long-lived PTY runtime owned by `BufferRuntimeHandle`, including its
  runtime keeper process and the terminal state it preserves for a `Buffer`
- backend - the `TerminalBackend` implementation used by that runtime keeper to store and expose
  visible state, full capture, and scrollback

## Capture surfaces

Embers exposes three related but distinct capture surfaces for PTY buffers:

- `capture_snapshot`: the full backend-defined capture for the buffer
- `capture_visible_snapshot`: the renderer-facing view of the currently visible screen
- `capture_scrollback_slice`: a paged read over the backend-defined scrollback history

All three are sourced from the durable buffer runtime (`BufferRuntimeHandle` -> runtime keeper -> `TerminalBackend`), not from layout state.

## Full snapshot semantics

`capture_snapshot` is the "capture pane/buffer" source of truth for PTY buffers.

It returns:

- the current snapshot sequence
- the buffer's current PTY size
- the backend's full captured lines
- the terminal title if the backend has one
- the buffer cwd tracked by the server

For PTY buffers, this is a runtime capture, not a view capture. Moving, detaching, or reattaching the buffer does not change which terminal state is returned.

## Visible snapshot semantics

`capture_visible_snapshot` is the authoritative render input for a PTY buffer.

It returns:

- the current visible lines from the active screen
- viewport position and total line count
- terminal mode bits such as alternate screen, mouse reporting, focus reporting, and bracketed paste
- cursor metadata
- size, sequence, title, and cwd

The visible snapshot always reflects the screen the backend considers active at the moment of capture.

## Scrollback semantics

`capture_scrollback_slice` pages through backend-defined history without changing visible state.

It returns:

- `start_line`: the effective start of the returned slice
- `total_lines`: the full scrollback length at capture time
- `lines`: the requested window into that history

Repeated reads without new output should be stable: the same buffer state should yield the same full snapshot, visible snapshot, and scrollback slice.

## Detached and exited buffers

Detached PTY buffers remain capturable.

While detached:

- full capture remains available
- visible snapshot remains available
- scrollback slices remain available
- title and other backend metadata remain available
- the most recent PTY size remains the size reported by capture APIs until another resize arrives

Exited PTY buffers also remain capturable as long as the buffer record still exists. Writes,
resizes, and kills must fail after exit, but capture reads continue to work. In practice callers
should expect an error result rather than a silent no-op: writes and kills surface a conflict such
as `buffer 12 has already exited`, while resize requests can fail with `buffer runtime has already
exited`.

## Alternate-screen policy

Alternate-screen ownership stays in the backend.

- `capture_visible_snapshot` reflects the active screen and reports whether alternate screen is active
- full capture and scrollback semantics follow the backend's own history model
- leaving alternate screen returns visible capture to the primary screen state the backend preserved

## Testing notes

Embers locks down the observable behavior with tests rather than layering extra server-side
alternate-screen state on top of the backend.

## Resize behavior

Resize updates the PTY and the keeper-owned backend surface. Future full and visible captures report the latest size and remain coherent across:

- direct resize requests
- detach while preserving the retained size
- reattach into differently sized views once a new resize is applied

## Code anchors

As of 2026-04-16:

- Runtime capture implementation: `crates/embers-server/src/server.rs`
- Runtime keeper capture surface: `crates/embers-server/src/buffer_runtime.rs`
- Backend-visible snapshot behavior: `crates/embers-server/src/terminal_backend.rs`
- PTY integration coverage: `crates/embers-test-support/tests/buffer_runtime.rs`
