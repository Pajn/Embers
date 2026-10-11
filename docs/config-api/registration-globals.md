# Registration Globals

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> bind </h2>

```rust,ignore
fn bind(mode: String, notation: String, action: Action)
fn bind(mode: String, notation: String, action_name: String)
fn bind(mode: String, notation: String, actions: Array)
```

<div>
<div class="tab">
<button group="bind" id="link-bind-Description"  class="tablinks active"
    onclick="openTab(event, 'bind', 'Description')">
Description
</button>
<button group="bind" id="link-bind-Example"  class="tablinks"
    onclick="openTab(event, 'bind', 'Example')">
Example
</button>
</div>

<div group="bind" id="bind-Description" class="tabcontent"  style="display: block;" >
Bind a key notation to an [`Action`], a string action name, or an array of actions.

Use the `Action` overload for inline builders such as `action.focus_left()`, the string
overload for a named action registered with `define_action`, or an array to chain multiple
actions in sequence.

A notation is a sequence of keys: plain characters, and names or modified keys in angle
brackets, such as `<Enter>`, `<C-a>`, `<M-h>`, `<S-Left>`, `<C-S-Tab>` or `<F5>`
(`C` Ctrl, `M` or `A` Alt, `S` Shift, `D` Super). As in tmux, an uppercase letter is the
shifted key: `<M-H>` is Alt+Shift+h and `<C-H>` is Ctrl+Shift+h, the same as `<C-S-h>`.
Legacy terminals send the same bytes for Ctrl+Shift+h as for Ctrl+h, so `<C-H>` and other
Ctrl+Shift bindings only fire from a host terminal with the kitty keyboard protocol.
</div>
<div group="bind" id="bind-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>ws", "workspace-split");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> define_action </h2>

```rust,ignore
fn define_action(name: String, callback: FnPtr)
```

<div>
<div class="tab">
<button group="define_action" id="link-define_action-Description"  class="tablinks active"
    onclick="openTab(event, 'define_action', 'Description')">
Description
</button>
</div>

<div group="define_action" id="define_action-Description" class="tabcontent"  style="display: block;" >
Register a function pointer as a named action callable from bindings.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> define_mode </h2>

```rust,ignore
fn define_mode(mode_name: String)
fn define_mode(mode_name: String, options: Map)
```

<div>
<div class="tab">
<button group="define_mode" id="link-define_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'define_mode', 'Description')">
Description
</button>
</div>

<div group="define_mode" id="define_mode-Description" class="tabcontent"  style="display: block;" >
Define a custom input mode with hooks and fallback options.

Supported options are `fallback`, `on_enter`, and `on_leave`.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> on </h2>

```rust,ignore
fn on(event_name: String, callback: FnPtr)
```

<div>
<div class="tab">
<button group="on" id="link-on-Description"  class="tablinks active"
    onclick="openTab(event, 'on', 'Description')">
Description
</button>
</div>

<div group="on" id="on-Description" class="tabcontent"  style="display: block;" >
Attach a callback to an emitted event such as `buffer_bell`.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_allow_passthrough </h2>

```rust,ignore
fn set_allow_passthrough(mode: String)
```

<div>
<div class="tab">
<button group="set_allow_passthrough" id="link-set_allow_passthrough-Description"  class="tablinks active"
    onclick="openTab(event, 'set_allow_passthrough', 'Description')">
Description
</button>
<button group="set_allow_passthrough" id="link-set_allow_passthrough-Example"  class="tablinks"
    onclick="openTab(event, 'set_allow_passthrough', 'Example')">
Example
</button>
</div>

<div group="set_allow_passthrough" id="set_allow_passthrough-Description" class="tabcontent"  style="display: block;" >
Choose which panes may write to the host terminal through tmux
passthrough, like tmux's `allow-passthrough`.

Programs that run under tmux wrap sequences meant for the outer
terminal, such as kitty graphics images or desktop notifications, as
`ESC P tmux; ... ESC \`. Embers unwraps them and writes them to the
terminal running the client, with its cursor at the pane's cursor so
images land where the program put them. Kitty graphics commands sent
without the wrapper are passed on the same way.

- `"off"` (the default, as in tmux): drop them.
- `"on"`: only from panes that are on screen.
- `"all"`: from every pane in the client's session, on screen or not.
  Sequences from a pane that is not on screen are written without
  moving the cursor.

Passthrough is off by default because it lets any program on screen,
or `cat` of an untrusted file, write raw sequences to the terminal,
bypassing `set_clipboard(false)` and the filtering Embers applies to
pane output. Turning it on only has an effect when the terminal is
known to support passthrough (see `set_terminal_features`).
</div>
<div group="set_allow_passthrough" id="set_allow_passthrough-Example" class="tabcontent"  style="display: none;" >

```rhai
set_allow_passthrough("on");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_base_index </h2>

```rust,ignore
fn set_base_index(index: int)
```

<div>
<div class="tab">
<button group="set_base_index" id="link-set_base_index-Description"  class="tablinks active"
    onclick="openTab(event, 'set_base_index', 'Description')">
Description
</button>
<button group="set_base_index" id="link-set_base_index-Example"  class="tablinks"
    onclick="openTab(event, 'set_base_index', 'Example')">
Example
</button>
</div>

<div group="set_base_index" id="set_base_index-Description" class="tabcontent"  style="display: block;" >
Number windows from `index` instead of 0, like tmux `set -g base-index 1`.

Windows are a session's root tabs. Their numbers are what
`TabInfo.number()` gives tab bar formatters and what
`action.select_window` takes. Windows are always numbered without gaps:
closing one renumbers the windows after it, like tmux with
`renumber-windows on`.

This setting covers the client and the commands it runs from the command
prompt. The client also reports it to the server for the session it
shows, and the `embers` CLI numbers that session's windows from it
unless `EMBERS_BASE_INDEX` or `--base-index` overrides it. Without this
call the client uses `EMBERS_BASE_INDEX` when it is set, and numbers
windows from 0 otherwise.
</div>
<div group="set_base_index" id="set_base_index-Example" class="tabcontent"  style="display: none;" >

```rhai
set_base_index(1);
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_clipboard </h2>

```rust,ignore
fn set_clipboard(enabled: bool)
```

<div>
<div class="tab">
<button group="set_clipboard" id="link-set_clipboard-Description"  class="tablinks active"
    onclick="openTab(event, 'set_clipboard', 'Description')">
Description
</button>
<button group="set_clipboard" id="link-set_clipboard-Example"  class="tablinks"
    onclick="openTab(event, 'set_clipboard', 'Example')">
Example
</button>
</div>

<div group="set_clipboard" id="set_clipboard-Description" class="tabcontent"  style="display: block;" >
Choose whether programs in panes may set the system clipboard.

When on (the default), an OSC 52 clipboard write from a program in a
pane, such as a Neovim yank over SSH, is passed on to the terminal
running the Embers client, which puts it on the system clipboard. This
matches tmux's `set-clipboard on`. When off, those writes are dropped.
Programs can never read the clipboard through Embers: OSC 52 queries are
always ignored.
</div>
<div group="set_clipboard" id="set_clipboard-Example" class="tabcontent"  style="display: none;" >

```rhai
set_clipboard(false);
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_detach_on_destroy </h2>

```rust,ignore
fn set_detach_on_destroy(value: String)
```

<div>
<div class="tab">
<button group="set_detach_on_destroy" id="link-set_detach_on_destroy-Description"  class="tablinks active"
    onclick="openTab(event, 'set_detach_on_destroy', 'Description')">
Description
</button>
<button group="set_detach_on_destroy" id="link-set_detach_on_destroy-Example"  class="tablinks"
    onclick="openTab(event, 'set_detach_on_destroy', 'Example')">
Example
</button>
</div>

<div group="set_detach_on_destroy" id="set_detach_on_destroy-Description" class="tabcontent"  style="display: block;" >
Choose where the client goes when the session it shows is destroyed,
like tmux's `detach-on-destroy`. A session is destroyed when its last
pane closes, because the pane's program exited or the pane was killed,
or when the session is killed.

- `"on"` (the default): detach; the client exits.
- `"off"`: switch to the session the client showed before, or else the
  first remaining session.
- `"previous"` / `"next"`: switch to the session listed before or after
  the destroyed one (`embers list-sessions` order), wrapping around.

When no other session is left, the client exits whatever the setting.
</div>
<div group="set_detach_on_destroy" id="set_detach_on_destroy-Example" class="tabcontent"  style="display: none;" >

```rhai
set_detach_on_destroy("off");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_leader </h2>

```rust,ignore
fn set_leader(notation: String)
```

<div>
<div class="tab">
<button group="set_leader" id="link-set_leader-Description"  class="tablinks active"
    onclick="openTab(event, 'set_leader', 'Description')">
Description
</button>
<button group="set_leader" id="link-set_leader-Example"  class="tablinks"
    onclick="openTab(event, 'set_leader', 'Example')">
Example
</button>
</div>

<div group="set_leader" id="set_leader-Description" class="tabcontent"  style="display: block;" >
Set the leader sequence used in binding notations.
</div>
<div group="set_leader" id="set_leader-Example" class="tabcontent"  style="display: none;" >

```rhai
set_leader("<C-a>");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_status_keys </h2>

```rust,ignore
fn set_status_keys(value: String)
```

<div>
<div class="tab">
<button group="set_status_keys" id="link-set_status_keys-Description"  class="tablinks active"
    onclick="openTab(event, 'set_status_keys', 'Description')">
Description
</button>
<button group="set_status_keys" id="link-set_status_keys-Example"  class="tablinks"
    onclick="openTab(event, 'set_status_keys', 'Example')">
Example
</button>
</div>

<div group="set_status_keys" id="set_status_keys-Description" class="tabcontent"  style="display: block;" >
Choose the keys the command prompt and the rename prompts are edited
with, like tmux's `status-keys`.

- `"emacs"` (the default): Left/Right, C-a/C-e, C-w, C-u, C-k and the
  other keys listed in the command prompt docs.
- `"vi"`: the prompt opens in insert mode, where those keys work too.
  Esc switches to normal mode, which has `h` `l` `w` `b` `e` `0` `$`
  to move, `x` `dw` `D` to delete, `cw` `C` to change, `i` `a` `I`
  `A` to insert again and `k` `j` for older and newer history lines.
  Esc in normal mode cancels the prompt; Enter runs it in either mode.

The copy-mode search prompt keeps its own keys.
</div>
<div group="set_status_keys" id="set_status_keys-Example" class="tabcontent"  style="display: none;" >

```rhai
set_status_keys("vi");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_terminal_features </h2>

```rust,ignore
fn set_terminal_features(features: Map)
```

<div>
<div class="tab">
<button group="set_terminal_features" id="link-set_terminal_features-Description"  class="tablinks active"
    onclick="openTab(event, 'set_terminal_features', 'Description')">
Description
</button>
<button group="set_terminal_features" id="link-set_terminal_features-Example"  class="tablinks"
    onclick="openTab(event, 'set_terminal_features', 'Example')">
Example
</button>
</div>

<div group="set_terminal_features" id="set_terminal_features-Description" class="tabcontent"  style="display: block;" >
Say what the terminal running the client supports, overriding what
Embers detected when it attached.

Embers asks the terminal at attach and otherwise goes by `TERM` and
`TERM_PROGRAM`, and leaves out what it cannot confirm. This sets each
named capability on (`true`) or off (`false`); capabilities not named
stay as detected. Like tmux's `terminal-features`.

- `underline_styles`: double, curly, dotted and dashed underlines
  (`CSI 4:n m`). Without it every underline is plain.
- `underline_color`: colored underlines (`CSI 58:...m`). Without it
  underlines take the text color.
- `hyperlinks`: OSC 8 links. Without it links are plain text.
- `passthrough`: sequences programs send for the terminal, such as
  images (see `set_allow_passthrough`). Without it they are dropped.
</div>
<div group="set_terminal_features" id="set_terminal_features-Example" class="tabcontent"  style="display: none;" >

```rhai
set_terminal_features(#{ hyperlinks: true, passthrough: false });
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> unbind </h2>

```rust,ignore
fn unbind(mode: String, notation: String)
```

<div>
<div class="tab">
<button group="unbind" id="link-unbind-Description"  class="tablinks active"
    onclick="openTab(event, 'unbind', 'Description')">
Description
</button>
</div>

<div group="unbind" id="unbind-Description" class="tabcontent"  style="display: block;" >
Remove a previously bound key sequence.

</div>

</div>
</div>
</br>
