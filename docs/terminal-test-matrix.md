# Terminal test matrix

This matrix maps the terminal-validation plan to the concrete regression suites that now guard the
behavior.

## Runtime and backend contracts

| Concern | Primary tests | Notes |
| --- | --- | --- |
| Buffer runtime ownership and PTY lifecycle | `crates/embers-test-support/tests/buffer_runtime.rs` | Locks down runtime state transitions and detached-buffer policy. |
| Backend boundary and capture semantics | `crates/embers-server/tests/backend.rs` and `crates/embers-server/tests/buffer_lifecycle.rs` | Verifies backend ownership, activity bookkeeping, and server-side lifecycle rules. |
| Byte-stream features and alternate-screen parsing | `crates/terminal-backend/tests/*` | Covers escape-sequence handling, visible snapshots, and parser/backend invariants. |

## Client and render-source contracts

| Concern | Primary tests | Notes |
| --- | --- | --- |
| Input routing and scripted actions | `crates/embers-client/tests/configured_client.rs` | Prefix handling, passthrough rules, scripted `send_keys` / `send_bytes`, reload behavior. |
| Activity, bell, and hidden-buffer metadata | `crates/embers-client/tests/e2e.rs` and `crates/embers-server/tests/buffer_lifecycle.rs` | Hidden and detached buffers keep activity, bell, and continuity metadata coherent. |
| Render invalidation and snapshot freshness | `crates/embers-client/tests/configured_client.rs` and `crates/embers-client/tests/e2e.rs` | Confirms the client renders from refreshed authoritative snapshots, not stale cache. |
| Full-screen and alternate-screen behavior | `crates/embers-client/tests/e2e.rs` | Verifies enter/exit semantics, hidden fullscreen buffers, and primary-screen restoration. |

## Real PTY end-to-end workflows

| Workflow | Primary tests | What it proves |
| --- | --- | --- |
| Spawn real client in PTY and run shell I/O | `crates/embers-cli/tests/interactive.rs::embers_without_subcommand_starts_server_and_client` | Embers can host a real shell and stay reachable through both the live client and CLI. |
| Attach, switch, detach, and reveal live clients | `crates/embers-cli/tests/interactive.rs` | Live PTY clients stay synchronized with server session targeting. |
| Local scrollback and OSC52 selection | `crates/embers-cli/tests/interactive.rs` | Client-local terminal UX features work under a real PTY. |
| Scripted input path in a live PTY client | `crates/embers-cli/tests/interactive.rs::scripted_input_bindings_reach_the_live_terminal_in_pty` | Scripted bindings still reach the focused terminal runtime end to end. |
| Config reload during terminal interaction | `crates/embers-cli/tests/interactive.rs::config_reload_updates_live_bindings_without_breaking_terminal_io` | Reloaded bindings take effect without breaking ongoing terminal input/output. |
| Split, move, detach, and reattach continuity | `crates/embers-cli/tests/interactive.rs::live_pty_client_preserves_buffers_across_layout_and_attachment_changes` | Layout and attachment changes do not reset the underlying shell process. |
| Hidden-buffer bell visibility and reveal continuity | `crates/embers-cli/tests/interactive.rs::hidden_buffer_bells_surface_in_the_attached_client_and_reveal_buffered_output` | Hidden buffers keep accumulating output, surface bell state, and reveal coherent content later. |
| Full-screen app entry and exit in the live client | `crates/embers-cli/tests/interactive.rs::fullscreen_terminal_transitions_render_in_the_live_client_pty` | The attached PTY client renders alternate-screen transitions the same way the backend models them. |

## Harness notes

- `crates/embers-test-support/src/pty.rs` provides the reusable PTY harness used by the CLI
  integration tests.
- PTY read helpers include the recent output tail in timeout errors so failures are debuggable
  without rerunning under a separate recorder.
- `crates/embers-cli/tests/interactive.rs` serializes PTY-heavy tests with a shared lock to reduce
  flaky PTY pressure in CI.
