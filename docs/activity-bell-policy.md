# Activity and bell policy

Phase 6 locks down how background terminal updates are reflected in buffer metadata.

## Buffer states

- `Idle` - no recent background activity is recorded for the buffer.
- `Activity` - PTY output advanced the buffer without a bell.
- `Bell` - PTY output for the buffer included a bell character.

## What counts as activity

Any PTY output that advances a buffer snapshot marks that buffer as `Activity`. This is stored on the durable `Buffer` record, not on the current view, so hidden and detached buffers keep their activity state even while they are off-screen.

## What counts as a bell

If the terminal backend observes a bell during an output update, the durable `Buffer` record's stored state is set to `Bell` instead of plain `Activity`. `Bell` wins over ordinary activity for that buffer until the state is cleared by focus, which lets client and automation layers distinguish attention-worthy output from normal background chatter.

The runtime keeper only remembers the activity of the latest output it read, and the server learns it by polling, so output right after a bell would otherwise hide the bell from the next poll. The keeper therefore also counts the output chunks that rang the bell, and the server marks the buffer `Bell` whenever that count has advanced since it last looked, whether the poll or a capture brings the news. A bell is recorded once: after focus clears it, the same bell does not come back.

## Hidden and detached buffers

Hidden tabs, inactive panes, and detached buffers continue to ingest PTY output, update captures, and overwrite their stored activity state as new output arrives. Clients that attach to the buffer recover that state from the server snapshot, and automation can observe bell updates from the normal render-invalidated path.

## Reset on focus

When a buffer becomes the focused leaf, the server clears its stored status back to `Idle`. This acknowledges the previously hidden background signal without destroying terminal state or capture history.

Reveal without focus does not clear stored status. That reset only applies to the activity/bell state that had accumulated before focus, so if the focused program produces more output afterward, later runtime updates can mark the buffer active again.
