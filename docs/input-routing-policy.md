# Input routing policy

This note defines how Embers decides whether input is handled locally or forwarded to the focused terminal buffer.

## Routing ownership

Input-routing policy currently lives in the client layer.

- `ConfiguredClient` owns key, paste, mouse, and focus routing decisions
- the server receives terminal-bound bytes as `InputRequest::Send`
- the server/runtime path writes those bytes directly to the focused buffer runtime

Today, routing is decided before the protocol request is sent. The server does not reinterpret keybindings after it receives `InputRequest::Send`.

## Key routing decision tree

For normal key events, `ConfiguredClient::handle_key` follows this order:

1. If the command prompt is open (`prompt` mode), its line editor takes the
   key; if the picker is open (`tree` mode), the picker takes it; if the
   client is in search mode, search-prompt handling wins.
2. Otherwise, the key is resolved against the current mode's bindings.
3. Exact matches execute configured actions.
4. Prefix matches stay pending and do not forward bytes yet.
5. Unmatched sequences follow the current mode's fallback policy:
   - `Passthrough`: send the key sequence to the focused buffer
   - `Ignore`: consume it locally without terminal output

That means partial leader or prefix sequences must never leak into the terminal while they are still unresolved.

## Modes and fallback

Built-in fallback policy is:

- `normal`: passthrough
- `copy`: ignore
- `search`: ignore
- `select`: ignore
- `hints`: ignore
- `prompt`: ignore (the command prompt edits its line itself)
- `tree`: ignore (the picker handles its own keys)

Changing modes clears any pending key sequence. Reloading config also clears pending input if the current mode still exists, or resets the client back to `normal` if it no longer does.

In `copy` and `select` mode, two kinds of input are handled before binding resolution:

- after `f`/`F`/`t`/`T`, the next character key is the jump target and any other key cancels the jump
- digits form a count prefix for the next binding (`0` only once a count has started), unless the mode binds that digit itself

See `docs/copy-mode.md` for copy-mode behaviour.

## Prefix and leader behavior

Leader bindings are expanded into ordinary key sequences during config compilation. At runtime there is no separate "leader state" object; the pending sequence in `InputState` is the source of truth.

While a sequence is still a prefix:

- no bytes are sent to the buffer
- no local fallback runs yet
- the client waits for the next key to decide whether the sequence resolves or falls back

## Local actions vs terminal passthrough

Most exact matches execute locally, but Embers intentionally avoids stealing some keys from terminal apps.

Configured bindings are forwarded to the terminal instead of executing locally when:

- the client is not in `copy` or `select` mode, the focused view is in alternate screen, and the bound actions are all local search/select/scroll actions, or
- the client is in normal mode with `follow_output` enabled, has no active search or selection state, and the bound actions require local search/select context

This keeps fullscreen terminal apps from losing keys that would otherwise be meaningful inside the application.

## Script-generated terminal input

Scripted actions fit into the same pipeline after binding resolution:

- `Action::SendKeys` converts notation into bytes
- `Action::SendBytes` uses the provided bytes directly
- both resolve a target buffer (`current` or explicit)
- both send `InputRequest::Send`

So scripted terminal input uses the same protocol/runtime path as unmapped passthrough keys.

## Special input paths

- paste uses `handle_paste`; if the focused buffer has bracketed paste enabled, Embers wraps the payload in `ESC [ 200~` / `ESC [ 201~`
- focus events use `handle_focus_event`; they forward `ESC [ I` / `ESC [ O` only when the program requested focus reporting
- mouse events use `handle_mouse`; they either drive local scroll/focus behavior or send encoded mouse bytes when mouse reporting is enabled. In panes without mouse reporting, left-button drags and double/triple clicks drive copy-mode selection instead; once a drag selection starts, its drag and release events stay with that pane until release
- host terminal replies (DCS, APC and OSC strings, and CSI reports with a private marker) use `handle_host_reply`; they are not typed input, so they skip the current mode, bindings and synchronize-panes, and go only to the pane whose passthrough last reached the host, or the focused pane when there is none
- input of any kind for a pane that refuses it, such as a dead pane kept by `remain-on-exit`, is dropped

## Code anchors

- Input resolution: `crates/embers-client/src/input/keymap.rs`
- Mode state and fallback policy: `crates/embers-client/src/input/modes.rs`
- Live routing and scripted action delivery: `crates/embers-client/src/configured_client.rs`
- Protocol input dispatch: `crates/embers-server/src/server.rs`
