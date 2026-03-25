## Activity and bell policy

Phase 6 locks down how background terminal updates are reflected in buffer metadata.

### What counts as activity

Any PTY output that advances a buffer snapshot marks that buffer as `Activity`. This is stored on the durable `Buffer` record, not on the current view, so hidden and detached buffers keep their activity state even while they are off-screen.

### What counts as a bell

If the terminal backend observes a bell during a given output update, that update is recorded as `Bell` instead of plain `Activity`. Bell wins over ordinary activity for that update, which lets client and automation layers distinguish attention-worthy output from normal background chatter.

### Hidden and detached buffers

Hidden tabs, inactive panes, and detached buffers continue to ingest PTY output, update captures, and overwrite their stored activity state as new output arrives. Reconnecting clients recover that state from the server snapshot, and automation can observe bell updates from the normal render-invalidated path.

### Reset on reveal or focus

When a buffer becomes the focused leaf, the server clears its stored activity back to `Idle`. This acknowledges the previously hidden background signal without destroying terminal state or capture history.

That reset only applies to the activity that had accumulated before focus. If the focused program produces more output afterward, later runtime updates can mark the buffer active again.
