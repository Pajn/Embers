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
