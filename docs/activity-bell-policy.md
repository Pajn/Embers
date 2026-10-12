# Activity and bell policy

Phase 6 locks down how background terminal updates are reflected in buffer metadata.

## Buffer states

- `Idle` - no recent background activity is recorded for the buffer.
- `Activity` - PTY output advanced the buffer without a bell.
- `Bell` - PTY output for the buffer included a bell character.

## What counts as activity

PTY output that advances a buffer snapshot marks that buffer as `Activity`, unless a client is looking at it. This is stored on the durable `Buffer` record, not on the current view, so hidden and detached buffers keep their activity state even while they are off-screen.

## Windows a client is looking at

Like tmux, activity and bells only flag a window nobody is looking at. When a client is attached to the buffer's session and the buffer's pane is in the session's current window, or in one of its visible popups, its output and bells leave the stored state as it is. Output anywhere else sets `Activity` or `Bell` until the window is visited. Which window is current is session state on the server, shared by every client attached to the session, so the server decides this when it records the output, and every client and automation reading the buffer sees the same state.

A session no client is attached to has nobody looking at it, so its output flags every window, the current one included, as tmux does. A client attaching to the session, or switching to it, acknowledges the flags of the panes it now shows, as tmux clears the current window's flags on attach, so the client does not find its current window flagged by output it was not there to see. Tools that list sessions, such as a session picker, still see what happened in a detached session while nobody was looking.

There is no setting to turn activity off, like tmux's `monitor-activity` (which tmux leaves off by default): Embers flags background output by default. The server records activity before any client config loads, so such a setting would have to be an environment variable read by the server, and showing or hiding the `+` marker is already up to the tab bar formatter.

## What counts as a bell

If the terminal backend observes a bell during an output update, the durable `Buffer` record's stored state is set to `Bell` instead of plain `Activity`. `Bell` wins over ordinary activity for that buffer until the state is cleared by focus, which lets client and automation layers distinguish attention-worthy output from normal background chatter.

A bell in a window a client is looking at does not flag it (see above).

The runtime keeper only remembers the activity of the latest output it read, and the server learns it by polling, so output right after a bell would otherwise hide the bell from the next poll. The keeper therefore also counts the output chunks that rang the bell, and the server marks the buffer `Bell` whenever that count has advanced since it last looked, whether the poll or a capture brings the news. A bell is recorded once: after focus clears it, the same bell does not come back.

## Hidden and detached buffers

Hidden tabs, inactive panes, and detached buffers continue to ingest PTY output, update captures, and update their stored activity state as new output arrives. Ordinary output never downgrades a stored `Bell` to `Activity`: the bell stays until it is acknowledged. Clients that attach to the buffer recover that state from the server snapshot, and automation can observe bell updates from the normal render-invalidated path. The client's `buffer_bell` event follows the stored state, so it fires for bells that flag a window and not for bells in a window a client shows. There is no event for activity of its own: clients and automation read it from the buffer record, which they refetch on `render_invalidated`, and from the session snapshot, which they refetch on `node_changed` when a visit or an attach clears flags.

## Reset on focus

When a buffer becomes the focused leaf, the server clears its stored status back to `Idle`. This acknowledges the previously hidden background signal without destroying terminal state or capture history.

Focus also acknowledges the activity and bells of every other pane the session now shows, as tmux clears a window's flags when the window becomes current: they go back to `Idle` too. So does a client attaching to the session or switching to it.

Reveal without focus does not clear stored status. That reset only applies to the activity/bell state that had accumulated before focus; output in the window afterward flags it again only once no client is looking at it.
