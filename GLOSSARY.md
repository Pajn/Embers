# Embers

A terminal multiplexer: a server keeps terminal programs running in buffers, which clients show
in panes, on this machine or on other hosts' Embers servers.

## Buffer runtimes

**Buffer runtime**:
What runs a buffer's program and emulates its terminal: a keeper, or another host's server.
_Avoid_: backend, runtime handle

**Keeper**:
A process on this machine that owns one buffer's PTY and outlives the server, so a restarted server
can take the buffer up again.

**Host**:
Another machine whose Embers server runs buffers for this one.
_Avoid_: remote, upstream (when meaning the machine)

**Placement**:
Where a buffer's program runs: in a keeper on this machine, or on a host.
_Avoid_: location, local/remote flag

**Exited**:
How a buffer ends when its program ended; nothing can take it up again.

**Interrupted**:
How a buffer ends when its buffer runtime can no longer be reached while its program may still
run, as when the server stops or a host is lost. It can be taken up again.
_Avoid_: lost (for the buffer's state)

**Release**:
The server being done with a buffer runtime, after which a keeper may exit and a host may remove an
exited buffer.
_Avoid_: join, unwatch
