# Terminal runtime contract

This note locks down the runtime contract Embers uses for PTY-backed buffers.

## Runtime ownership

`Buffer` is the durable terminal runtime record. A PTY-backed `Buffer` owns:

- the PTY/process command, cwd, and environment hints
- runtime identity and lifecycle state (`Created`, `Running`, `Interrupted`, `Exited`)
- the runtime keeper socket path used to reconnect after restore
- attachment state (`Attached(NodeId)` or `Detached`)
- the authoritative PTY size policy
- terminal-facing metadata surfaced outside layout code:
  - title
  - activity/bell state
  - last snapshot sequence
- the snapshot source of truth via the buffer runtime/keeper backend

`BufferView` is a renderer/layout attachment only. It owns:

- session/layout placement (`NodeId`, parentage, split/tab membership)
- focus/zoom/follow-output view flags
- the last render size used by that view

`BufferView` does not own PTY handles, process lifetime, terminal parsing state, or scrollback.

## Closing a view vs killing a buffer

Closing a view removes the `BufferView` node and transitions the buffer to `Detached`.

- the PTY/process keeps running
- output continues accumulating
- snapshots and scrollback remain queryable
- title/activity metadata remains on the buffer record

Killing a buffer targets the runtime, not the view tree.

- the PTY child is terminated
- the buffer transitions to `Exited`
- if the buffer was still attached, it remains `Attached(NodeId)` until that view is later closed or replaced
- capture remains available from the terminal backend snapshot state
- the buffer record may later be cleaned up once it is detached

## Attachment and move semantics

`Attached -> Detached` happens when the owning view is closed or explicitly detached.

`Detached -> Attached` happens when the buffer is attached to a new leaf.

A "move" is a composite operation:

1. detach or close the old view
2. keep the buffer runtime alive
3. attach the same buffer to the target leaf

Moving tabs, leaves, or floating roots must never reset PTY state because the runtime stays with the `Buffer`, not the `BufferView`.

## PTY sizing

The server applies sizes; attached clients choose them. Before each frame the live client resizes
every visible PTY buffer to the content area it draws for that leaf (the leaf rect minus its title
row, after splits, tab bars, floating borders and zoom) through `InputRequest::Resize`, and only
when that size changed.

When several clients show the same buffer, the most recently active client wins: a client resizes a
buffer when its own wanted size changes (attach, layout change, outer-terminal resize) or after
local input, never merely because another client resized it. Hidden buffers keep their last size.

A failed resize is retried on a later frame, at most once a second, while the buffer is still a
visible running PTY. The retry is dropped if another client resized the buffer in the meantime, so
it never takes a size back that a successful request would have left alone.

## Detached buffer policy

Detached PTY buffers keep the last assigned `pty_size` until another explicit resize arrives.

While detached:

- output continues to accumulate
- full and visible snapshots remain available
- scrollback remains available
- activity/bell state continues to update on the buffer record

Reattaching a detached buffer does not recreate the process or reset terminal state. It only gives the durable runtime a new view attachment.

## Runtime lifecycle transitions

The intended transition graph is:

- `Created -> Running` when the runtime keeper is spawned or restored
- `Running -> Exited` when the child process terminates normally or is explicitly killed
- `Running/Created -> Interrupted` when a restore cannot reconnect to a keeper
- `Exited -> cleaned up` when a detached exited buffer is removed from server state

## Attachment transitions

- `Attached(NodeId) -> Detached` when a view closes or the buffer is explicitly detached
- `Detached -> Attached(NodeId)` when the buffer is attached to a leaf

The composite "move" operation is `Attached(NodeId) -> Detached -> Attached(NodeId)` while the
same runtime stays alive.

## Code anchors

- Runtime model: `crates/embers-server/src/model.rs`
- State transitions: `crates/embers-server/src/state.rs`
- Runtime keeper and PTY lifecycle: `crates/embers-server/src/buffer_runtime.rs`
- Server/runtime wiring: `crates/embers-server/src/server.rs`
