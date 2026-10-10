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

New panes start at their drawn size, so a program that reads its size at startup never sees a
default size first. `BufferRequest::Create` takes an optional PTY size that the server applies
before spawning the command; without one the buffer starts at 80x24. The requester works the size
out with `spawn_pty_size`: it applies the placement that follows (split, new window or tab, popup,
replaced pane) to a copy of the session layout and projects it like a frame, so the size includes
all client chrome. The server knows the split tree but not that chrome, which is why it does not
compute the size itself.

- The interactive client's actions (`split_with`, `insert_tab_*`, `open_floating`,
  `replace_*`) and the first window of a new session use the client's own viewport.
- CLI commands (`split-window`, `new-window`, `display-popup`) use the session's viewport: the
  content area an attached client last drew it in, which each client reports with
  `SessionRequest::SetViewport` when it changes. The server keeps it after the client detaches and
  saves it with the workspace, and a new session starts with the viewport last reported for any
  session. When none is known yet, the pane starts at 80x24 and is resized once a client draws it.
- Panes restored without their keeper start at the PTY size saved with them.

The size is a best-effort start: a placement hidden behind a zoom is not sized, and the attached
client still resizes every visible pane to its drawn size on the next frame.

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
- `Interrupted -> Running` when restore starts a fresh process for that buffer (see below)
- `Exited -> cleaned up` when a detached exited buffer is removed from server state

## Keeper process lifetime

Each PTY runs inside a runtime keeper process that owns the PTY and serves captures over a
socket in the server's runtime directory.

- a clean server shutdown leaves keepers running, so a restarted server can reattach through
  their sockets and restore `Running` buffers
- a keeper keeps serving captures after its child exits while a server stays attached to it
- once no server is attached, a keeper whose child has exited has nothing left to reattach to
  and exits; a restarted server keeps the buffer record as `Exited`
- while no server is attached, a keeper also watches its socket; once the socket is removed or
  replaced no server can reattach, so the keeper stops its child (SIGHUP, then SIGKILL) and exits

## Workspace persistence

The server saves the workspace to `<socket>.workspace.json`: sessions, the layout tree (splits,
tabs, floating windows, focus and zoom) and every buffer record. For a buffer that is its id,
title, command, environment hints, last known working directory, the command line of the job last
seen in its foreground, its keeper socket path, its lifecycle state and its attachment. Terminal
contents and scrollback are not saved; they live in the keeper only.

The workspace is saved whenever it changes, not only on a clean shutdown:

- every request that changes sessions, layout or buffers (any request that emits an event other
  than a render invalidation)
- a buffer's title, working directory or foreground command changing
- a buffer exiting
- server startup, once keepers are reattached or restored

Saves are batched: the first change starts a short timer (100 ms) and one write covers every change
made before it fires. Output and activity alone never trigger a save. A write whose contents match
the previous one is skipped. Each write goes to a temp file next to the workspace file, is fsynced,
renamed over it, and the directory is fsynced, so a crash leaves either the previous file or the new
one. A crash or SIGKILL of the server therefore loses at most the changes of the last batch. A clean
shutdown writes the final state once more.

## Restore after keepers are gone

On startup the server reattaches every buffer whose keeper is still alive, exactly as after a
clean shutdown. A buffer whose keeper is gone (its socket is missing or nothing answers on it, as
after a reboot) cannot be reattached and is marked `Interrupted`. With restore on, the default,
every interrupted PTY buffer then gets a fresh keeper:

- same buffer id, title, environment hints and place in the layout, so panes come back where they
  were; detached buffers come back detached
- started in the last known working directory, or its nearest ancestor that still exists
- if the buffer ran a shell, the same shell starts. If its last foreground job was an allowlisted
  program, that command line is typed into the new shell once it prints its prompt, the way
  tmux-resurrect does, so leaving the program drops back to the shell
- if the buffer ran an allowlisted program directly, that program starts again
- anything else restores as a plain shell (`$SHELL`, or `/bin/sh`)

The allowlist matches the program name of the command line and defaults to tmux-resurrect's: `vi`,
`vim`, `nvim`, `emacs`, `man`, `less`, `more`, `tail`, `top`, `htop`, `irssi`, `weechat`, `mutt`.
The foreground job is the PTY's foreground process group, read by the keeper; it is only recorded
while it differs from the buffer's own process, so a shell at its prompt has none.

Restore can be turned off and the allowlist replaced through environment variables (see
`docs/configuration.md`). With restore off, such buffers stay `Interrupted`. `Exited` buffers are
never restarted.

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
- Workspace file format and atomic writes: `crates/embers-server/src/persist.rs`
- Restore planning and the command allowlist: `crates/embers-server/src/restore.rs`
