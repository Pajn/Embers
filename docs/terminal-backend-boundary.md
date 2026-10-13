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
- the remaining bytes go directly to the backend, with side scanners: one sniffs OSC 7 working-directory reports, and the others pick out the queries alacritty leaves unanswered (DECRQSS, XTGETTCAP and the cell size query `CSI 16 t`). Output is split after each such query and the backend answers it through `TerminalBackend::answer_query`, so the reply reflects the state at that byte and is queued in order with the emulator's own replies. See [Terminal capabilities](configuration.md#terminal-capabilities).

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

## Choosing the backend

Keepers emulate with `AlacrittyTerminalBackend` unless the binary chose another with `set_terminal_backend`, a factory from the pane's size and scrollback ceiling to a `Box<dyn TerminalBackend>`. A keeper is a process of its own, started from the server's executable (or `EMBERS_RUNTIME_KEEPER_BIN`), so a binary that chooses sets the factory first thing in `main`, whatever it runs as.

`embers-ghostty`, in the unpublished `crates/embers-ghostty-backend`, is the one that does: Embers with `GhosttyTerminalBackend`, on libghostty-vt, to try beside alacritty. It names itself as the keeper executable and defaults to its own socket, `embers-ghostty.sock` beside Embers' own, so its sessions and saved layout are its own. libghostty answers DECRQSS and the capabilities it knows itself; for those queries the Ghostty backend's `answer_query` adds only what libghostty left unanswered, so a program never gets two replies.

## Compaction when idle

A backend can shrink what it holds in memory in bounded steps (`TerminalBackend::compact_step`); the Ghostty backend compresses scrollback pages, which cuts a pane with full scrollback from about 16 MiB to under 1 MiB. Steps never run on the output path. The keeper counts output, resizes and reads of the scrollback as activity, and a thread of its own runs the steps once five seconds pass without any, taking the surface lock for one step at a time so output and requests get in between. Activity stops the run, which starts over when the pane is quiet again. Alacritty's backend has nothing to compact.

## Clipboard writes

The emulator runs with OSC 52 set to copy-only: stores raise a clipboard event and queries are dropped without a reply. `take_events` drains the latest store with the activity state and query replies. The keeper keeps only the most recent write, numbered, and reports the number in its status reply; when the number advances the server's status poller fetches the write with a separate `Clipboard` request, so status polls stay small and keepers that predate clipboard forwarding are never asked. The server then sends a `ClipboardEvent` straight to the clients whose current session contains the buffer, rather than broadcasting it, and each client re-emits it to its host terminal as OSC 52 unless `set_clipboard(false)` is configured. See [Clipboard (OSC 52)](configuration.md#clipboard-osc-52).

## Passthrough

The router unwraps `ESC P tmux; <payload> ESC \` the way tmux does: `ESC ESC` in the payload is one ESC, `ESC \` ends it, and an ESC before anything else is dropped. CAN or SUB, escaped or not, cancels it as it does any DCS: the payload is dropped and the byte runs as a control, so a program that gives up mid-sequence, or `cat` of a binary file, does not swallow the pane's later output. Bytes that might begin a passthrough are held back, across read chunks, until it is clear whether they do, so the emulator never sees part of one; in particular a doubled-ESC payload never runs as pane output. Right after the bytes before it are ingested, the router reads the backend's cursor position (`TerminalBackend::cursor_position`, which ignores whether the cursor is hidden) and returns the payload with it as a `PassthroughWrite`. A payload over `MAX_PASSTHROUGH_BYTES` (4 MiB) is dropped whole, and the scanner skips to its terminator (or a CAN or SUB).

The same scanner takes unwrapped kitty graphics commands, APC strings starting with `G`, out of the stream and returns them whole, so they take the same path. alacritty ignores every APC, so these were lost before, and programs only send them when they believe the terminal shows images. Like any APC one ends at the first ESC, CAN or SUB, and a command ended by anything but `ESC \` is dropped; other APC strings still go to the backend. Unwrapped sixel (`ESC P q`) stays with the backend, which ignores it: the emulator neither stores sixel images nor moves its cursor past them, and its DA1 reply does not claim sixel, so forwarding raw sixel would put the host and the pane out of step. Rendering sixel into the pane, as tmux 3.4 does, would need an image store in the backend.

The keeper queues the writes in order, numbered, under the surface lock the read loop already holds, and reports the latest number in its status reply. The queue holds at most 8 MiB and drops its oldest writes beyond that, which bounds memory while no server is attached. Unlike the latest-only clipboard write every write is kept, because an image is many sequences. When the number advances, the status poller takes the queued writes with `Passthrough { after }` requests, base64 in keeper JSON and at most 8 MiB per reply, and hands them to the server in one callback per poll. The poller seeds the number from the attach-time status, so a reattaching server drops what was queued for frames long gone.

The server forwards each batch through one task per buffer runtime rather than a task per call, so writes reach clients in the order the program sent them. It targets the same clients as a clipboard write, splits the batch into `PassthroughEvent`s of at most 6 MiB to stay under the 8 MiB frame cap, and sends them best effort: a client too slow to take one misses it but stays attached. What a client does with them is in the [render-source contract](render-source-contract.md#passthrough-sequences).

## Replies behind graphics queries

A program that detects kitty graphics sends a graphics query and then queries the emulator answers, and reads until the last answer; see [Detecting image support](configuration.md#detecting-image-support). The graphics query travels keeper, server, client, host and back as client input, while the emulator's replies are written to the PTY as soon as the read loop has ingested the output. So the keeper keeps them in order itself (`ReplyHold` in `crates/embers-server/src/reply_hold.rs`):

- `RawByteRouter::route_output_with` hands the keeper each sequence for the host as it is taken out, with the backend as it is at that byte. The keeper first drains the replies to what came before (`TerminalBackend::take_replies`) and passes them on, then, for each answered graphics query in the sequence (`a=q`, an `i=` or `I=`, not quiet), records it as pending with a deadline of 500 ms. Replies the emulator produces while a query is pending are held, in order.
- Input written to the PTY (`KeeperRequest::Write`) is scanned for graphics replies. One that names a pending query clears it; once none is pending the held replies are queued for the writer thread right after that input, so the program reads the host's answer first.
- A thread per keeper waits on a condition variable for the deadline and lets the held replies go if no answer came. Turning holding off, or the server disconnecting, lets them go at once. Held replies are capped at 64 KiB; past that they all go.

Holding only happens while the server says some client forwards graphics queries to a host that answers them (`KeeperRequest::SetHostGraphics`). Each client tells the server whether it does with `ClientRequest::SetHostGraphics` (passthrough on and a host with `kitty_graphics`); the server tells every keeper when the answer for the whole server changes, and each new keeper when it is on. It is not worked out per pane, so a pane that no such client shows waits out the deadline. Without it the emulator answers at once, which tells the program there is no graphics support. A keeper that predates the request rejects it and never holds; a server that predates it never sends it.

## Pixel size

`PtySize` carries the text area in pixels with the cells. A client that knows its terminal's cell size (`TIOCGWINSZ`) sends it with each resize (`InputRequest::Resize`) and with the size of the panes it creates; the server keeps it on the buffer record, applies it to the PTY, so `TIOCGWINSZ` in the pane reports it, and hands it to the backend, which answers the text area (`CSI 14 t`) and cell size (`CSI 16 t`) queries from it. A pixel size of zero means unknown: the backend then answers neither.

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

- Raw routing + backend trait + backend choice: `crates/embers-server/src/terminal_backend.rs`
- Ghostty backend and `embers-ghostty`: `crates/embers-ghostty-backend`
- Keeper-owned surface: `crates/embers-server/src/buffer_runtime.rs`
- Server/runtime wiring: `crates/embers-server/src/server.rs`
- Replies held behind kitty graphics queries: `crates/embers-server/src/reply_hold.rs`
- Compaction when idle: `crates/embers-server/src/idle_compaction.rs`
