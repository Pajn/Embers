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
| `EMBERS_RESTORE` | on | server | Set to `0`, `off`, `false` or `no` to leave panes whose keeper is gone `Interrupted` instead of restoring them. |
| `EMBERS_REMAIN_ON_EXIT` | `off` | server | Default of the `remain-on-exit` pane option for panes that do not set it: `on` keeps a pane whose program exited as a dead pane, `failed` only when it exited with a nonzero status. A pane's own option wins. |
| `EMBERS_RESTORE_COMMANDS` | `vi vim nvim emacs man less more tail top htop irssi weechat mutt` | server | Programs run again when a pane is restored, separated by commas or spaces. Replaces the default; set but empty, no program is run again. |

A limit set to `0`, empty, or an unparseable value is ignored, so the default
applies.

### Set in panes

The server sets these in the environment of every pane's program:

| Variable | Value |
| --- | --- |
| `EMBERS_SOCKET` | The server's control socket, so `embers` run in a pane reaches the server the pane belongs to. |
| `EMBERS_BUFFER` | The id of the pane's buffer, like tmux's `TMUX_PANE`. It is set after the environment the pane was created with, so it always names the pane's own buffer, including after the pane is restored. |
| `TERM`, `COLORTERM` | `xterm-256color` and `truecolor` unless the pane's environment sets them (see [Terminal capabilities](#terminal-capabilities)). |

An `embers` command run inside a pane uses `EMBERS_BUFFER` when it is given no
`-t` target and `EMBERS_SOCKET` names the socket the command talks to: pane
commands (`send-keys`, `capture-pane`, `buffer set-option`, `split-window`,
`kill-pane`, ...) act on that pane, window commands on its window, session
commands on its session and `kill-popup` on the popup it is in. A buffer that
is not shown in any pane still takes buffer commands such as `send-keys`,
`capture-pane` and `buffer set-option`.

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

## Workspace persistence and restore

The server saves sessions, layout and buffer records to `<socket>.workspace.json`
whenever they change, batching bursts of changes into one atomic write, so a
crash or SIGKILL of the server loses at most the last moment of changes.

On startup, panes whose runtime keeper is still alive are reattached. Panes whose
keeper is gone, as after a reboot, are restored: a fresh shell starts in the
pane's last known working directory, in the same place in the layout and with
the same title. Terminal contents and scrollback are not restored. If the pane's
last foreground program is on the allowlist (`EMBERS_RESTORE_COMMANDS`), it is
run again in that shell; anything else is not, since re-running an arbitrary
command could repeat its side effects.

```sh
# Run only editors and ssh again
EMBERS_RESTORE_COMMANDS="vim nvim ssh" embers
# Never restart panes whose keeper is gone
EMBERS_RESTORE=off embers
```

These are read by the server when it starts, before any client connects, so they
are environment variables rather than Rhai config settings. Set them where the
server is launched; a server started by `embers` inherits that command's
environment. See [`terminal-runtime.md`](terminal-runtime.md) for exactly what is
saved and when.

## Dead panes

A pane closes when its program exits, like tmux with `remain-on-exit off`. The
`remain-on-exit` pane option keeps it as a dead pane that can still be
captured: `on` always, `failed` only when the program exited with a nonzero
status.

```sh
# Keep this pane once its program exits
embers buffer set-option -t 4 remain-on-exit on
# Keep every pane that does not say otherwise
EMBERS_REMAIN_ON_EXIT=on embers
```

The server-wide default is an environment variable for the same reason as the
restore settings: the server reads it when it starts, before any client config
is loaded.

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

A program inside Embers only uses OSC 52 if it believes the terminal supports
it; see [Terminal capabilities](#terminal-capabilities).

## Terminal capabilities

Programs in panes see `TERM=xterm-256color` and `COLORTERM=truecolor` unless
the buffer's environment sets them. Embers ships no terminfo entry of its own,
and `xterm-256color` exists on every system, including remote hosts reached
over SSH. That entry does not list the extended capabilities Embers supports:
`Smulx` (underline styles), `Setulc` (underline color) and `Ms` (OSC 52). So
that programs can still find them, Embers answers the two queries they use to
ask the terminal directly:

- **DECRQSS for SGR** (`DCS $ q m ST`) reports the current graphic rendition,
  including `4:n` underline styles and the `58` underline color. Neovim sets a
  curly underline and asks this; if the answer keeps `4:3` it uses curly
  underlines and underline colors (diagnostics, spelling) even though the
  terminfo entry lacks `Smulx`. Without the answer it falls back to plain
  underlines in the text color. Other settings get the "invalid request"
  reply.
- **XTGETTCAP** (`DCS + q <hex names> ST`) answers `Ms`, `Smulx` and
  `Setulc`, and reports any other name as unknown. Neovim asks for `Ms` and,
  when the answer is OSC 52 and no other clipboard tool is available (as on a
  typical remote host), uses OSC 52 for its `+` and `*` registers.

Programs that read capabilities only from terminfo still see plain
`xterm-256color`. For those, set `TERM` in the buffer's environment to an entry
that lists the capabilities and that is installed where the program runs.

Embers always renders underline styles, underline colors and OSC 8 hyperlinks
to the host terminal with the standard sequences (see
[the render-source contract](render-source-contract.md)), so the host terminal
must support them; Ghostty, kitty, WezTerm, foot and VTE-based terminals do.
