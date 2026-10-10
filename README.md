# Embers

Embers is a Rust terminal multiplexer built around a headless server, a terminal UI client, and an automation-friendly command line interface. It manages sessions, windows, panes, floating popups, detachable terminal buffers, scrollback/history, and configurable key bindings through a Rhai-based config API.

The project is still early, but the shape is already close to a tmux-like workflow with a programmable client layer and durable PTY-backed buffers.

## Features

- Headless Unix-socket server with interactive terminal clients
- Sessions, windows, panes, tab trees, splits, and floating popups
- Durable terminal buffers that can be detached, reattached, moved, and captured
- Layout saved as it changes and panes restored after a crash or reboot, tmux-resurrect style
- Scrollback, visible snapshots, search, and a vi-style copy mode with mouse selection (see
  [`docs/copy-mode.md`](docs/copy-mode.md))
- A session/window/pane picker, a command prompt that runs the CLI's commands,
  `display-message` and session environments, as in tmux (see
  [`docs/commands.md`](docs/commands.md))
- Scriptable Rhai config for key bindings, modes, actions, themes, tab bars, and hooks
- CLI commands for scripting and automation
- FlatBuffers protocol shared by the client and server crates

## Requirements

- Rust `1.92` or newer
- `flatc` from FlatBuffers `25.12.19`
- `mdbook` when regenerating the config API docs

If you use Nix, the included flake provides the pinned development tools:

```sh
nix develop
```

Without Nix, make sure `flatc` is available on `PATH` before building.

## Quick Start

Build the workspace:

```sh
cargo build --workspace
```

Run the interactive client:

```sh
cargo run -p embers-cli --bin embers
```

Running `embers` without a subcommand starts the background server if needed, creates or selects the default session, and attaches the terminal UI. Press `Ctrl-q` to leave the interactive client.

You can also install and run the binary directly:

```sh
cargo install --path crates/embers-cli
embers
```

## CLI Usage

Embers exposes both an interactive client and command-oriented operations through the `embers` binary.

```sh
embers --help
embers list-sessions
embers new-session work
embers new-window --title logs -- tail -f app.log
embers split-window --horizontal -- cargo test
embers list-panes
embers capture-pane
```

By default, Embers uses a runtime socket at:

- `$XDG_RUNTIME_DIR/embers/embers.sock`, when `XDG_RUNTIME_DIR` is set
- `/run/user/<uid>/embers/embers.sock`, when available
- `/tmp/embers-<uid>/embers.sock`, as a fallback

Override the socket with either `--socket` or `EMBERS_SOCKET`:

```sh
embers --socket /tmp/my-embers.sock list-sessions
EMBERS_SOCKET=/tmp/my-embers.sock embers attach
```

Windows are numbered from 0 unless a base index is set (`EMBERS_BASE_INDEX=1`,
`--base-index 1`, or `set_base_index(1)` in the config), as with tmux's
`base-index`. Closing a window renumbers the ones after it. See
[`docs/commands.md`](docs/commands.md).

The same commands can be typed at the client's command prompt, tmux's
`prefix :`, bound with `action.command_prompt()`. See
[`docs/commands.md`](docs/commands.md#command-prompt).

Useful subcommand groups include:

- `session`: `new-session`, `list-sessions`, `rename-session`, `kill-session`, `set-environment`, `show-environment`
- `window`: `new-window`, `list-windows`, `select-window`, `rename-window`, `kill-window`, `synchronize-panes`
- `pane`: `split-window`, `list-panes`, `select-pane`, `resize-pane` (`-L/-R/-U/-D [N]`, `-Z`, `--sizes`), `send-keys`, `capture-pane`, `kill-pane`
- `buffer`: `buffer show`, `buffer history`, `buffer reveal`, `buffer pipe`
- `node`: `node zoom`, `node swap`, `node break`, `node join-buffer`, `node move-before`, `node move-after`
- `popup`: `display-popup`, `kill-popup`
- `client`: `list-clients`, `detach-client`, `switch-client`, `display-message` (`-p` prints instead)
- `server`: `kill-server` (stop the server and every program it runs)

As in tmux, `kill-pane`, `kill-window`, `kill-session` and `kill-popup` end the
programs they show; pass `--detach` to keep them running as detached buffers
instead. A pane closes when its program exits, unless its buffer has
`remain-on-exit` set (`embers buffer set-option remain-on-exit on`).

## Logging

The tracing filter is resolved from the first of these that is set, highest
precedence first: `--log <FILTER>` (alias `--log-level`), `-v`/`-vv`,
`EMBERS_LOG`, `RUST_LOG`, then the default of `info`. `<FILTER>` accepts the full
`tracing` env-filter syntax (a bare level like `debug`, or per-target directives
like `embers_server=trace,info`).

```sh
embers --log-level debug list-sessions
EMBERS_LOG=embers_server=trace,info embers
```

Foreground commands log to stderr. The background server writes to a
daily-rotating file in the socket's directory, named
`embers-server.<date>.log`, retaining the most recent 7 days. The launching
command's filter is propagated to the server, and server panics are recorded in
the same log.

## Resource Limits

The server enforces operator-tunable ceilings so a runaway client cannot exhaust
host resources. Each is overridable via an environment variable:

- `EMBERS_MAX_SESSIONS` (default `256`)
- `EMBERS_MAX_BUFFERS` (default `2048`) — each buffer owns a PTY-backed process plus scrollback, so this is the dominant resource bound
- `EMBERS_MAX_SCROLLBACK_LINES` (default `10000`)

Requests that would exceed a limit are rejected with an error naming the limit;
existing sessions are unaffected.

```sh
EMBERS_MAX_BUFFERS=256 EMBERS_MAX_SCROLLBACK_LINES=2000 embers
```

See [`docs/configuration.md`](docs/configuration.md) for the full reference of
operational environment variables and flags.

## Persistence and Restore

The server saves sessions, layout and pane records next to its socket
(`<socket>.workspace.json`) whenever they change, so killing the server loses at
most the last moment of changes. Pane processes run in separate keeper processes
that outlive the server; a restarted server reattaches to them.

When a pane's keeper is gone, as after a reboot, the pane is restored in place
with a fresh shell in its last working directory. A program from a safe
allowlist (`vim`, `less`, `tail`, `htop`, and so on) that was running in it is
started again; other programs are not, and scrollback is not restored.
`EMBERS_RESTORE=off` turns this off and `EMBERS_RESTORE_COMMANDS` replaces the
allowlist. See [`docs/configuration.md`](docs/configuration.md#workspace-persistence-and-restore).

## Configuration

Embers loads configuration in this order:

1. `--config <path>`
2. `EMBERS_CONFIG`
3. The platform config directory for the `embers` application, typically `~/.config/embers/config.rhai` on Linux
4. Built-in defaults

User config is loaded as an overlay on top of the built-in defaults. The built-in config enables mouse behavior (including drag-to-copy), scroll/search modes, and tmux `copy-mode-vi` style bindings for copy mode, described in [`docs/copy-mode.md`](docs/copy-mode.md).

This repository includes a fuller example at [`config.rhai`](config.rhai). It defines a tmux-inspired leader, smart navigation for Vim/Neovim panes, split/tab helpers, popup helpers, history views, and a custom tab bar/theme.

Run with an explicit config while developing:

```sh
embers --config ./config.rhai
```

Pane resizing, zoom, dragging split dividers with the mouse, `synchronize-panes`, and a smart-splits.nvim setup are described in [`docs/layout-controls.md`](docs/layout-controls.md).

The generated config API reference lives in [`docs/config-api`](docs/config-api/index.md), with a rendered mdBook copy in [`docs/config-api-book`](docs/config-api-book/index.html).

Operational configuration (socket path, logging, resource limits, and restore via
environment variables and flags) is documented in
[`docs/configuration.md`](docs/configuration.md).

## Development

Run the test suite:

```sh
cargo test --workspace
```

Run formatting and lint checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Regenerate the config API docs:

```sh
cargo gen-docs
```

The CI workflow also runs an ignored PTY smoke test:

```sh
cargo test -p embers-test-support pty_round_trips_input -- --ignored
```

## Workspace Layout

- [`crates/embers-core`](crates/embers-core): shared IDs, geometry, snapshots, diagnostics, and error types
- [`crates/embers-protocol`](crates/embers-protocol): FlatBuffers schema, codec, framing, and protocol client
- [`crates/embers-server`](crates/embers-server): server state, persistence, terminal backend, and PTY runtime management
- [`crates/embers-client`](crates/embers-client): client state, rendering, input handling, configuration, scripting, and docs generation
- [`crates/embers-cli`](crates/embers-cli): `embers` binary, interactive terminal UI, and CLI command implementations
- [`crates/embers-test-support`](crates/embers-test-support): shared test harnesses for server, protocol, PTY, and CLI tests

## Architecture Notes

The docs directory contains design notes that are useful when changing behavior:

- [`docs/terminal-runtime.md`](docs/terminal-runtime.md): PTY-backed buffer ownership and lifecycle
- [`docs/terminal-backend-boundary.md`](docs/terminal-backend-boundary.md): server/backend responsibility boundary
- [`docs/terminal-capture-model.md`](docs/terminal-capture-model.md): terminal capture and snapshot model
- [`docs/input-routing.md`](docs/input-routing.md): input routing model
- [`docs/layout-controls.md`](docs/layout-controls.md): pane resizing, divider dragging, and synchronize-panes
- [`docs/activity-bell-policy.md`](docs/activity-bell-policy.md): activity and bell state policy
- [`docs/render-source-contract.md`](docs/render-source-contract.md): rendering source-of-truth contract

## License

This workspace is licensed under GPL-3.0-only.
