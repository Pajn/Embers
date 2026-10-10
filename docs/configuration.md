# Operational Configuration

This document covers the environment variables and command-line flags that
configure the Embers server and client at runtime. For the scriptable Rhai
config API (key bindings, modes, actions, themes, tab bars, hooks) see
[`config-api`](config-api/index.md).

## Environment variables

| Variable | Default | Applies to | Description |
| --- | --- | --- | --- |
| `EMBERS_SOCKET` | platform runtime socket (see below) | client + server | Path to the control socket. Overridden by `--socket`. |
| `EMBERS_CONFIG` | platform config dir | client | Path to the Rhai config file. Overridden by `--config`. |
| `EMBERS_LOG` | `info` | client + server | Tracing filter (e.g. `info`, `debug`, `embers=trace,info`). Overridden by `--log`/`-v`. Falls back to `RUST_LOG`. |
| `RUST_LOG` | unset | client + server | Standard tracing filter, used when `EMBERS_LOG` is unset. |
| `EMBERS_MAX_SESSIONS` | `256` | server | Ceiling on concurrently live sessions. |
| `EMBERS_MAX_BUFFERS` | `2048` | server | Ceiling on concurrently live buffers. Each buffer owns a PTY-backed child process plus scrollback, so this is the dominant resource bound. |
| `EMBERS_MAX_SCROLLBACK_LINES` | `10000` | server | Scrollback lines retained per buffer. |

A limit set to `0`, empty, or an unparseable value is ignored, so the default
applies.

## Socket path

When `--socket`/`EMBERS_SOCKET` is not set, the socket is resolved to the first
available of:

1. `$XDG_RUNTIME_DIR/embers/embers.sock`, when `XDG_RUNTIME_DIR` is set
2. `/run/user/<uid>/embers/embers.sock`, when available
3. `/tmp/embers-<uid>/embers.sock`, as a fallback

The default runtime directory is created with `0700` permissions and the socket
with `0600`, so other users on the host cannot reach it.

### Runtime keeper sockets

Each buffer's PTY is owned by a separate runtime keeper process, which the server
reaches over its own Unix socket. These sockets live in a directory derived from
the server socket, `<socket>.runtimes/` (for example `embers.runtimes/` next to
`embers.sock`), and are named `buffer-<id>.sock`.

Unix socket paths are limited to 104 bytes on macOS and 108 bytes on Linux. A
keeper socket that would not fit in the derived directory goes in
`/tmp/embers-<uid>/runtimes-<hash>/` instead, where `<hash>` is derived from the
server socket path, so each server keeps a stable directory of its own. Any
server socket path that can itself be bound therefore works. Keeper socket
directories are created with `0700` permissions.

## Logging

The tracing filter is resolved from the first of these that is set, highest
precedence first:

1. `--log <FILTER>` (alias `--log-level`)
2. `-v` (`debug`) or `-vv` (`trace`)
3. `EMBERS_LOG`
4. `RUST_LOG`
5. `info` (default)

`<FILTER>` accepts the full
[`tracing` env-filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
syntax, e.g. a bare level (`debug`) or per-target directives
(`embers_server=trace,info`).

```sh
embers --log-level debug list-sessions
EMBERS_LOG=embers_server=trace,info embers
```

### Client vs. server logs

Foreground invocations (interactive client, one-shot CLI commands) log to
`stderr`.

The background server has no terminal, so it writes to a **daily-rotating log
file** in the socket's directory:

- File name: `embers-server.<date>.log` (e.g. `embers-server.2026-06-20.log`)
- Rotation: daily
- Retention: the most recent 7 files; older files are pruned automatically
- Format: plain text (no ANSI styling)

For the default socket this is, for example,
`$XDG_RUNTIME_DIR/embers/embers-server.<date>.log`. A `--log`/`-v` flag passed to
the launching command is propagated to the detached server via `EMBERS_LOG`, so
the same filter applies to both. Server panics are routed through tracing into
the same log file.

## Resource limits

The server enforces operator-tunable ceilings so a misbehaving or runaway client
cannot exhaust host resources by creating unbounded sessions, buffers, or
scrollback. When a limit is reached the offending request is rejected with an
error naming the limit and its override variable; existing sessions are
unaffected.

Worst-case server memory from scrollback is bounded by roughly
`max_buffers × max_scrollback_lines × line_width`, so the buffer and scrollback
limits can be tuned together to fit a deployment.

```sh
# Tighter limits for a constrained host
EMBERS_MAX_BUFFERS=256 EMBERS_MAX_SCROLLBACK_LINES=2000 embers
```

## Clipboard (OSC 52)

Programs in panes can set the system clipboard with OSC 52, the way they would
in a terminal without a multiplexer. A Neovim yank on a remote host over SSH,
for example, reaches the clipboard of the machine running the Embers client.
This is the equivalent of tmux's `set-clipboard on`, and it is on by default.

- **Who receives a write.** The server sends it to every client whose current
  session contains the pane's buffer, including panes on hidden tabs and in
  floating windows, much as tmux sends it to every client attached to the
  pane's session. Each such client is a terminal the user may be looking at.
  Clients on other sessions do not get it, and a write from a detached buffer,
  or one whose session no client is showing, is dropped.
- **What the client does.** It re-emits the write to its host terminal as
  `OSC 52 ; c ; <base64> BEL` (or `p` for the primary selection). The host
  terminal must allow OSC 52 writes.
- **Turning it off.** Put `set_clipboard(false);` in the config file. The
  client then drops the writes it receives.
- **Reads are never answered.** An OSC 52 query (`OSC 52 ; c ; ?`) is ignored
  without a reply, so a program in a pane cannot read the clipboard through
  Embers. There is no option to allow it.
- **Size.** Writes larger than 4 MiB of text are dropped.
