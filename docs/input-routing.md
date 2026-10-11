# Input routing

This is the short-form map of how Embers routes terminal input today.

The detailed contract and rationale live in `docs/input-routing-policy.md`. This file exists as the
plan-facing entry point for the same behavior.

## Summary

- `ConfiguredClient` decides whether a key is handled locally or passed through to the focused
  buffer.
- Leader and prefix sequences stay client-local until they resolve or are cleared.
- Copy/select/search modes suppress passthrough for the bindings they own.
- Scripted `send_keys*` and `send_bytes*` actions bypass local rendering logic and write bytes to
  the target buffer runtime.
- Hidden or detached buffers do not receive arbitrary typed input unless a script or explicit
  command targets them directly.
- With `synchronize-panes` on for the active window, typed input for the focused pane is also sent
  to the window's other visible panes, encoded per pane; popups never take part (see
  `docs/layout-controls.md`).
- Replies from the host terminal are not typed input: they go to one pane only and are never
  synchronized, typed into a prompt or matched against bindings (see `handle_host_reply`).
- A left press on a split divider starts a divider drag that the client keeps until release;
  presses inside panes are routed as before.

## Current boundaries

- Terminal-facing input ultimately lands in the buffer runtime, not in layout state.
- The `RawByteRouter` remains the explicit seam for raw-byte policy, but normal live PTY input is
  currently decided earlier by the configured client and keymap.
- Config reload clears pending prefixes before the next key is interpreted under the new bindings.

## Primary regression coverage

- `crates/embers-client/tests/configured_client.rs`
  - prefix no-leak behavior
  - copy-mode passthrough suppression
  - scripted `send_keys` / `send_bytes`
  - reload clearing pending prefixes
- `crates/embers-client/tests/copy_mode.rs`
  - copy-mode motions, counts, jumps, selection and search direction
  - copy-mode bindings from config
  - mouse drag/double-click selection and forwarding to mouse-reporting programs

For the full policy text, see `docs/input-routing-policy.md`.
