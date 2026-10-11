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
| `EMBERS_BASE_INDEX` | unset | client + CLI | Number of the first window, like tmux `base-index`. The client uses it when its Rhai config does not call `set_base_index`; the CLI uses it instead of the base index the client reported to the server, and `--base-index` overrides it. See [`commands.md`](commands.md#window-numbering). |

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
crash or SIGKILL of the server loses at most the last moment of changes: 100 ms
of layout changes, and 1.5 s of pane title, directory and foreground command
changes, which programs can make many times a second.

On startup, panes whose runtime keeper is still alive are reattached. Panes whose
keeper is gone, as after a reboot, are restored (detached buffers, which are in
no pane, are dropped rather than restarted): a fresh shell starts in the
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
status. As in tmux, keys, pastes, mouse and focus reports aimed at a dead pane
are dropped; with synchronize-panes on, the window's other panes still get
their copy.

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

## Passthrough

Programs written for tmux send sequences meant for the outer terminal wrapped
as `ESC P tmux; <sequence> ESC \`, with every ESC inside doubled. Image
viewers (image.nvim, yazi, kitty graphics previewers) send images this way, and
other programs send desktop notifications or titles. Embers accepts the same
wrapper and, once passthrough is turned on, writes the unwrapped sequence to
the host terminal of each client showing the pane, like tmux with
`set -g allow-passthrough on`.

```rhai
set_allow_passthrough("on");
```

- **Off by default.** As in tmux, passthrough is off unless the config turns
  it on. With it on, any program on screen, or `cat` of an untrusted file, can
  write raw sequences to the host terminal, bypassing `set_clipboard(false)`
  and the filtering Embers applies to what panes print.
- **Choosing which panes.** `set_allow_passthrough("on")` forwards sequences
  from panes that are on screen in the client. `set_allow_passthrough("all")`
  also forwards them from panes in hidden windows of the client's session.
  `set_allow_passthrough("off")`, the default, drops them. The setting also
  covers [unwrapped kitty graphics
  commands](#images-without-the-tmux-wrapper). Turning it on does not force
  it: nothing is forwarded to a host terminal that is [not known to support
  passthrough](#the-host-terminal).
- **Who receives them.** Like [clipboard writes](#clipboard-osc-52), the
  server sends them only to clients whose current session contains the pane;
  each client then checks whether the pane is on its screen. Sequences from a
  detached buffer, or one whose session no client is showing, are dropped.
- **Cursor.** Before writing a sequence from a pane on screen, the client moves
  the host cursor to where the pane's cursor was when the program sent it,
  translated to screen coordinates, and puts its own cursor back afterwards.
  Sequences that draw at the cursor, such as a kitty graphics placement or a
  sixel image, land where the program meant them to. A sequence from a pane
  that is not on screen (with `all`) is written without moving the cursor.
- **Order.** Sequences reach the host in the order the program sent them, each
  after a frame that shows everything the program printed before it. See the
  [render-source contract](render-source-contract.md#passthrough-sequences).
- **Size.** A sequence over 4 MiB is dropped whole, since a truncated graphics
  command would leave the host waiting for the rest of it. Large kitty images
  are not affected: programs send them as many small chunks.
- **Replies.** A host terminal's reply to a forwarded query (for example a
  kitty graphics query) arrives with the keyboard input, but the client reads
  DCS, APC and OSC strings and CSI reports with a private marker (`CSI ?`,
  `CSI >`, `CSI =`) as replies, not keys. A reply goes whole to the pane whose
  passthrough sequence last reached the host, since only panes send queries
  through passthrough; when no pane has, or that pane is gone, it goes to the
  focused pane. That includes replies that look like those to the client's
  own queries, such as DA1 or XTVERSION; only a late reply to one of those
  queries while it is still unanswered is dropped (see [the host
  terminal](#the-host-terminal)). A reply is never copied to other panes by
  synchronize-panes, typed into the command prompt or a picker's filter, or
  matched against key bindings.

Images are not part of the pane's contents in Embers: they are not kept in
snapshots or scrollback, and Embers does not move or redraw them. What a host
terminal does with an image when Embers redraws the cells under it decides how
long it stays:

- **Kitty graphics with Unicode placeholders** (`U=1`) work best. The image is
  transmitted through passthrough, and the placeholder cells the program prints
  are ordinary text, so they are part of the pane, survive redraws and scroll
  with the text.
- **Kitty graphics placed at the cursor** live on the host's image layer,
  apart from the text Embers redraws, but they do not follow the pane's text
  when it scrolls, when the layout changes or when the window is switched. The
  program has to place or delete them again, as image.nvim does when it
  redraws.
- **Sixel** images are cells, so the next frame Embers draws over those cells
  erases them. They are only shown briefly.

### Images without the tmux wrapper

Programs decide whether to wrap sequences for tmux mostly from the `TMUX`
environment variable, which Embers does not set. Programs that do not see it
send their sequences unwrapped, as to a terminal of their own:

- **Kitty graphics commands** (`ESC _ G ... ESC \`) are forwarded like wrapped
  ones, with the same setting, cursor handling and size cap. The pane's
  terminal emulator would drop them, and a program only sends them when it
  believes the terminal shows images (often from `TERM_PROGRAM`, which panes
  inherit from the environment the server started in), so passing them on is
  what the program expects.
- **Sixel** (`ESC P q ...`) is not forwarded. The emulator does not keep sixel
  images, does not move its cursor past them as a sixel terminal would, and
  does not report sixel support, so a program that checks first does not send
  any. tmux 3.4 and later draws sixel images into its own screen instead;
  Embers has no image store to do that. Wrapped sixel still goes through
  passthrough, and is erased by the next frame as described above.
- **Detection.** A program that asks the terminal whether it supports kitty
  graphics (`a=q`) and then sends a device attributes query (DA1) gets the DA1
  answer from Embers first, before the host's answer to the graphics query
  comes back, and usually concludes there is no image support. The late
  answer then arrives in the pane as input (see Replies above). tmux behaves
  the same way.

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

### The host terminal

The terminal running the client is the host terminal. Underline styles,
underline colors, OSC 8 hyperlinks and [passthrough](#passthrough) need
sequences that not every host understands, and one that does not can draw them
as garbage. So when the client attaches it finds out what the host supports
and leaves out the rest: every underline becomes a plain one, underlines take
the text color, links become plain text and passthrough is dropped. Detection
only decides whether passthrough is possible; it is still off until the
config turns it on with `set_allow_passthrough`.

To find out, the client writes three queries together with the sequences that
set up the screen and enable the kitty keyboard protocol, and reads the
replies before it draws anything:

- **XTVERSION** (`CSI > 0 q`) for the host's name.
- **XTGETTCAP** for `Smulx` and `Setulc`, the terminfo capabilities for
  underline styles and underline colors.
- **DA1** (`CSI c`). Every terminal answers it, and terminals answer queries in
  order, so its reply means every other reply has arrived. Terminals that do
  not know the first two queries ignore them.

If no DA1 reply arrives within a second the client goes on without one. Keys
typed while it waits are dropped. A reply to one of these queries that arrives
later, within ten more seconds, is recognised and dropped rather than handed to
a pane as a [reply](#passthrough). Each query takes one reply: once it has
been answered, or the ten seconds are up, an identical reply answers a query a
pane sent through passthrough and goes to that pane. Then:

- A host that answered XTGETTCAP for `Smulx` or `Setulc` gets underline
  styles or underline colors.
- A host Embers knows by name gets what it is known to support: Ghostty,
  kitty, WezTerm, foot and Contour everything; iTerm2 hyperlinks and
  passthrough; Alacritty underline styles, underline colors and hyperlinks.
  The name comes from the XTVERSION reply, or without one from `TERM_PROGRAM`
  or `TERM` (`xterm-ghostty`, `xterm-kitty`, `wezterm`, `foot`, `alacritty`).
- Any other host gets the plain forms.

There is no query for OSC 8, so hyperlinks depend on the host's name. When
detection gets it wrong, `set_terminal_features` in the config file forces a
capability on or off:

```rhai
set_terminal_features(#{ hyperlinks: true, passthrough: false });
```

The names are `underline_styles`, `underline_color`, `hyperlinks` and
`passthrough`. Capabilities the call does not name stay as detected.

### Window title

The client sets the host terminal's window title to the name of the session
it shows (OSC 2), when it attaches and whenever it switches sessions or the
session is renamed, like tmux with `set-titles on` and `set-titles-string
'#S'`. It does not restore the previous title on exit; most terminals do that
themselves.
