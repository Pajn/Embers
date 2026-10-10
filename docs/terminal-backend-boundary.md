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
- output routing first takes tmux passthrough sequences (`ESC P tmux; ... ESC \`) and unwrapped kitty graphics commands (`ESC _ G ... ESC \`) out of the stream (see below); the backend never sees them
- the remaining bytes go directly to the backend, with two side scanners: one sniffs OSC 7 working-directory reports, and one picks out the DCS queries alacritty leaves unanswered (DECRQSS and XTGETTCAP). Output is split after each such query and the backend answers it through `TerminalBackend::answer_query`, so the reply reflects the state at that byte and is queued in order with the emulator's own replies. See [Terminal capabilities](configuration.md#terminal-capabilities).

The seam is also where further special-case interception, or metadata extraction that must happen before backend ingestion, belongs.

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
- surface OSC 52 clipboard writes
- report the cursor position, shown or hidden, for passthrough

## Clipboard writes

The emulator runs with OSC 52 set to copy-only: stores raise a clipboard event and queries are dropped without a reply. `take_events` drains the latest store with the activity state and query replies. The keeper keeps only the most recent write, numbered, and reports the number in its status reply; when the number advances the server's status poller fetches the write with a separate `Clipboard` request, so status polls stay small and keepers that predate clipboard forwarding are never asked. The server then sends a `ClipboardEvent` straight to the clients whose current session contains the buffer, rather than broadcasting it, and each client re-emits it to its host terminal as OSC 52 unless `set_clipboard(false)` is configured. See [Clipboard (OSC 52)](configuration.md#clipboard-osc-52).

## Passthrough

The router unwraps `ESC P tmux; <payload> ESC \` the way tmux does: `ESC ESC` in the payload is one ESC, `ESC \` ends it, and an ESC before anything else is dropped. Bytes that might begin a passthrough are held back, across read chunks, until it is clear whether they do, so the emulator never sees part of one; in particular a doubled-ESC payload never runs as pane output. Right after the bytes before it are ingested, the router reads the backend's cursor position (`TerminalBackend::cursor_position`, which ignores whether the cursor is hidden) and returns the payload with it as a `PassthroughWrite`. A payload over `MAX_PASSTHROUGH_BYTES` (4 MiB) is dropped whole, and the scanner skips to its terminator.

The same scanner takes unwrapped kitty graphics commands, APC strings starting with `G`, out of the stream and returns them whole, so they take the same path. alacritty ignores every APC, so these were lost before, and programs only send them when they believe the terminal shows images. Like any APC one ends at the first ESC, and a command ended by anything but `ESC \` is dropped; other APC strings still go to the backend. Unwrapped sixel (`ESC P q`) stays with the backend, which ignores it: the emulator neither stores sixel images nor moves its cursor past them, and its DA1 reply does not claim sixel, so forwarding raw sixel would put the host and the pane out of step. Rendering sixel into the pane, as tmux 3.4 does, would need an image store in the backend.

The keeper queues the writes in order, numbered, under the surface lock the read loop already holds, and reports the latest number in its status reply. The queue holds at most 8 MiB and drops its oldest writes beyond that, which bounds memory while no server is attached. Unlike the latest-only clipboard write every write is kept, because an image is many sequences. When the number advances, the status poller takes the queued writes with `Passthrough { after }` requests, base64 in keeper JSON and at most 8 MiB per reply, and hands them to the server in one callback per poll. The poller seeds the number from the attach-time status, so a reattaching server drops what was queued for frames long gone.

The server forwards each batch through one task per buffer runtime rather than a task per call, so writes reach clients in the order the program sent them. It targets the same clients as a clipboard write, splits the batch into `PassthroughEvent`s of at most 6 MiB to stay under the 8 MiB frame cap, and sends them best effort: a client too slow to take one misses it but stays attached. What a client does with them is in the [render-source contract](render-source-contract.md#passthrough-sequences).

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
