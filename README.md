# Embers

Embers is a Rust terminal multiplexer built around a headless server, a terminal UI client, and an automation-friendly command line interface. It manages sessions, windows, panes, floating popups, detachable terminal buffers, scrollback/history, and configurable key bindings through a Rhai-based config API.

The project is still early, but the shape is already close to a tmux-like workflow with a programmable client layer and durable PTY-backed buffers.

## Features

- Headless Unix-socket server with interactive terminal clients
- Sessions, windows, panes, tab trees, splits, and floating popups
- Durable terminal buffers that can be detached, reattached, moved, and captured
- Scrollback, visible snapshots, search, selection, and yank-oriented client behavior
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

Useful subcommand groups include:

- `session`: `new-session`, `list-sessions`, `rename-session`, `kill-session`
- `window`: `new-window`, `list-windows`, `select-window`, `rename-window`, `kill-window`
- `pane`: `split-window`, `list-panes`, `select-pane`, `resize-pane`, `send-keys`, `capture-pane`, `kill-pane`
- `buffer`: `buffer show`, `buffer history`, `buffer reveal`, `buffer pipe`
- `node`: `node zoom`, `node swap`, `node break`, `node join-buffer`, `node move-before`, `node move-after`
- `popup`: `display-popup`, `kill-popup`

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

## Configuration

Embers loads configuration in this order:

1. `--config <path>`
2. `EMBERS_CONFIG`
3. The platform config directory for the `embers` application, typically `~/.config/embers/config.rhai` on Linux
4. Built-in defaults

User config is loaded as an overlay on top of the built-in defaults. The built-in config enables mouse behavior, scroll/search/select modes, and basic selection bindings.

This repository includes a fuller example at [`config.rhai`](config.rhai). It defines a tmux-inspired leader, smart navigation for Vim/Neovim panes, split/tab helpers, popup helpers, history views, and a custom tab bar/theme.

Run with an explicit config while developing:

```sh
embers --config ./config.rhai
```

The generated config API reference lives in [`docs/config-api`](docs/config-api/index.md), with a rendered mdBook copy in [`docs/config-api-book`](docs/config-api-book/index.html).

Operational configuration (socket path, logging, and resource limits via
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
- [`docs/activity-bell-policy.md`](docs/activity-bell-policy.md): activity and bell state policy
- [`docs/render-source-contract.md`](docs/render-source-contract.md): rendering source-of-truth contract

## License

This workspace is licensed under GPL-3.0-only.
