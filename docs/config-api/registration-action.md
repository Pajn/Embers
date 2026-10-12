# Action (Registration)

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> break_current_node </h2>

```rust,ignore
fn break_current_node(_: ActionApi, destination: "tab" | "floating") -> Action
```

<div>
<div class="tab">
<button group="break_current_node" id="link-break_current_node-Description"  class="tablinks active"
    onclick="openTab(event, 'break_current_node', 'Description')">
Description
</button>
</div>

<div group="break_current_node" id="break_current_node-Description" class="tabcontent"  style="display: block;" >
Break the current node into a new tab or floating window.
`destination` accepts `tab` or `floating`.
Example: `action.break_current_node("floating")`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> cancel_search </h2>

```rust,ignore
fn cancel_search(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="cancel_search" id="link-cancel_search-Description"  class="tablinks active"
    onclick="openTab(event, 'cancel_search', 'Description')">
Description
</button>
</div>

<div group="cancel_search" id="cancel_search-Description" class="tabcontent"  style="display: block;" >
Cancel the active search.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> cancel_selection </h2>

```rust,ignore
fn cancel_selection(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="cancel_selection" id="link-cancel_selection-Description"  class="tablinks active"
    onclick="openTab(event, 'cancel_selection', 'Description')">
Description
</button>
</div>

<div group="cancel_selection" id="cancel_selection-Description" class="tabcontent"  style="display: block;" >
Clear the current selection and return to copy mode with the cursor
where the selection ended.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> chain </h2>

```rust,ignore
fn chain(_: ActionApi, actions: Array) -> Action
```

<div>
<div class="tab">
<button group="chain" id="link-chain-Description"  class="tablinks active"
    onclick="openTab(event, 'chain', 'Description')">
Description
</button>
</div>

<div group="chain" id="chain-Description" class="tabcontent"  style="display: block;" >
Chain multiple actions into one composite action.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> choose_session </h2>

```rust,ignore
fn choose_session(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="choose_session" id="link-choose_session-Description"  class="tablinks active"
    onclick="openTab(event, 'choose_session', 'Description')">
Description
</button>
<button group="choose_session" id="link-choose_session-Example"  class="tablinks"
    onclick="openTab(event, 'choose_session', 'Example')">
Example
</button>
</div>

<div group="choose_session" id="choose_session-Description" class="tabcontent"  style="display: block;" >
Open the picker on the sessions, collapsed, like tmux `choose-tree -s`
(`prefix s`). See `choose_tree` for the keys.
</div>
<div group="choose_session" id="choose_session-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>S", action.choose_session());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> choose_tree </h2>

```rust,ignore
fn choose_tree(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="choose_tree" id="link-choose_tree-Description"  class="tablinks active"
    onclick="openTab(event, 'choose_tree', 'Description')">
Description
</button>
<button group="choose_tree" id="link-choose_tree-Example"  class="tablinks"
    onclick="openTab(event, 'choose_tree', 'Example')">
Example
</button>
</div>

<div group="choose_tree" id="choose_tree-Description" class="tabcontent"  style="display: block;" >
Open the picker on a tree of every session, window and pane, like
tmux `choose-tree`. `j`/`k` or the arrows move, `l`/`h` (or Right/Left)
expand and collapse, Enter switches to the selected item, `x` kills it
after asking, `/` or typing other text filters, and `q`/Esc closes.
</div>
<div group="choose_tree" id="choose_tree-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>T", action.choose_tree());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> choose_window </h2>

```rust,ignore
fn choose_window(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="choose_window" id="link-choose_window-Description"  class="tablinks active"
    onclick="openTab(event, 'choose_window', 'Description')">
Description
</button>
<button group="choose_window" id="link-choose_window-Example"  class="tablinks"
    onclick="openTab(event, 'choose_window', 'Example')">
Example
</button>
</div>

<div group="choose_window" id="choose_window-Description" class="tabcontent"  style="display: block;" >
Open the picker with every session expanded to its windows, like tmux
`choose-tree -w` (`prefix w`). See `choose_tree` for the keys.
</div>
<div group="choose_window" id="choose_window-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>\"", action.choose_window());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> clear_pending_keys </h2>

```rust,ignore
fn clear_pending_keys(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="clear_pending_keys" id="link-clear_pending_keys-Description"  class="tablinks active"
    onclick="openTab(event, 'clear_pending_keys', 'Description')">
Description
</button>
</div>

<div group="clear_pending_keys" id="clear_pending_keys-Description" class="tabcontent"  style="display: block;" >
Clear any partially-entered key sequence.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> close_floating </h2>

```rust,ignore
fn close_floating(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="close_floating" id="link-close_floating-Description"  class="tablinks active"
    onclick="openTab(event, 'close_floating', 'Description')">
Description
</button>
</div>

<div group="close_floating" id="close_floating-Description" class="tabcontent"  style="display: block;" >
Close the currently focused floating window. Its buffers keep running,
detached; use `kill_floating` to end them.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> close_floating_id </h2>

```rust,ignore
fn close_floating_id(_: ActionApi, floating_id: int) -> Action
```

<div>
<div class="tab">
<button group="close_floating_id" id="link-close_floating_id-Description"  class="tablinks active"
    onclick="openTab(event, 'close_floating_id', 'Description')">
Description
</button>
</div>

<div group="close_floating_id" id="close_floating_id-Description" class="tabcontent"  style="display: block;" >
Close a floating window by id. Its buffers keep running, detached.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> close_node </h2>

```rust,ignore
fn close_node(_: ActionApi, node_id: int) -> Action
```

<div>
<div class="tab">
<button group="close_node" id="link-close_node-Description"  class="tablinks active"
    onclick="openTab(event, 'close_node', 'Description')">
Description
</button>
</div>

<div group="close_node" id="close_node-Description" class="tabcontent"  style="display: block;" >
Close a view by node id. The buffers it shows keep running, detached.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> close_view </h2>

```rust,ignore
fn close_view(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="close_view" id="link-close_view-Description"  class="tablinks active"
    onclick="openTab(event, 'close_view', 'Description')">
Description
</button>
</div>

<div group="close_view" id="close_view-Description" class="tabcontent"  style="display: block;" >
Close the currently focused view. Its buffer keeps running, detached;
use `kill_view` to end it.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> command_prompt </h2>

```rust,ignore
fn command_prompt(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="command_prompt" id="link-command_prompt-Description"  class="tablinks active"
    onclick="openTab(event, 'command_prompt', 'Description')">
Description
</button>
<button group="command_prompt" id="link-command_prompt-Example"  class="tablinks"
    onclick="openTab(event, 'command_prompt', 'Example')">
Example
</button>
</div>

<div group="command_prompt" id="command_prompt-Description" class="tabcontent"  style="display: block;" >
Open the command prompt (tmux `:`). Enter runs the typed line with the
same commands and options as the `embers` CLI, such as
`split-window --horizontal`, `rename-window logs` or `select-window -t 2`;
commands without `-t` apply to the current session, window and pane.
Up and Down browse earlier lines; Esc cancels.
</div>
<div group="command_prompt" id="command_prompt-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>:", action.command_prompt());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> command_prompt_template </h2>

```rust,ignore
fn command_prompt_template(_: ActionApi, prompt: String, initial: String, template: String) -> Action
```

<div>
<div class="tab">
<button group="command_prompt_template" id="link-command_prompt_template-Description"  class="tablinks active"
    onclick="openTab(event, 'command_prompt_template', 'Description')">
Description
</button>
<button group="command_prompt_template" id="link-command_prompt_template-Example"  class="tablinks"
    onclick="openTab(event, 'command_prompt_template', 'Example')">
Example
</button>
</div>

<div group="command_prompt_template" id="command_prompt_template-Description" class="tabcontent"  style="display: block;" >
Open a prompt labelled `prompt` with `initial` typed (both may use
formats); Enter runs `template` with `%%` replaced by the input as one
word, like tmux `command-prompt -p prompt -I initial template`.
</div>
<div group="command_prompt_template" id="command_prompt_template-Example" class="tabcontent"  style="display: none;" >

```rhai
action.command_prompt_template("(new window) ", "", "new-window --title %%")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> command_prompt_with </h2>

```rust,ignore
fn command_prompt_with(_: ActionApi, initial: String) -> Action
```

<div>
<div class="tab">
<button group="command_prompt_with" id="link-command_prompt_with-Description"  class="tablinks active"
    onclick="openTab(event, 'command_prompt_with', 'Description')">
Description
</button>
<button group="command_prompt_with" id="link-command_prompt_with-Example"  class="tablinks"
    onclick="openTab(event, 'command_prompt_with', 'Example')">
Example
</button>
</div>

<div group="command_prompt_with" id="command_prompt_with-Description" class="tabcontent"  style="display: block;" >
Open the command prompt with `initial` already typed, like tmux
`command-prompt -I`. The text may use the formats `display_message`
accepts, such as `#W`.
</div>
<div group="command_prompt_with" id="command_prompt_with-Example" class="tabcontent"  style="display: none;" >

```rhai
action.command_prompt_with("split-window -- ")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> commit_search </h2>

```rust,ignore
fn commit_search(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="commit_search" id="link-commit_search-Description"  class="tablinks active"
    onclick="openTab(event, 'commit_search', 'Description')">
Description
</button>
</div>

<div group="commit_search" id="commit_search-Description" class="tabcontent"  style="display: block;" >
Finalize the active search, keep the current match and cursor position, and leave search
mode with the committed result in place.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> confirm </h2>

```rust,ignore
fn confirm(_: ActionApi, prompt: String, action: ?) -> Action
```

<div>
<div class="tab">
<button group="confirm" id="link-confirm-Description"  class="tablinks active"
    onclick="openTab(event, 'confirm', 'Description')">
Description
</button>
<button group="confirm" id="link-confirm-Example"  class="tablinks"
    onclick="openTab(event, 'confirm', 'Example')">
Example
</button>
</div>

<div group="confirm" id="confirm-Description" class="tabcontent"  style="display: block;" >
Ask before running `action`, like tmux `confirm-before`: `prompt` is
shown in the status line, expanded as a format (`#W`, `#D`, ...), and
`action`, an action or an array of actions, runs only if the next key
is `y`. Any other key cancels.
</div>
<div group="confirm" id="confirm-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>x", action.confirm("kill-pane #D? (y/n)", action.run_command("kill-pane")));
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_back_to_indentation </h2>

```rust,ignore
fn copy_back_to_indentation(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_back_to_indentation" id="link-copy_back_to_indentation-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_back_to_indentation', 'Description')">
Description
</button>
</div>

<div group="copy_back_to_indentation" id="copy_back_to_indentation-Description" class="tabcontent"  style="display: block;" >
Move to the first non-blank character of the line (`^`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_bottom_line </h2>

```rust,ignore
fn copy_bottom_line(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_bottom_line" id="link-copy_bottom_line-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_bottom_line', 'Description')">
Description
</button>
</div>

<div group="copy_bottom_line" id="copy_bottom_line-Description" class="tabcontent"  style="display: block;" >
Move to the bottom row of the view (`L`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_cursor_down </h2>

```rust,ignore
fn copy_cursor_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_cursor_down" id="link-copy_cursor_down-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_cursor_down', 'Description')">
Description
</button>
</div>

<div group="copy_cursor_down" id="copy_cursor_down-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor down (`j`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_cursor_left </h2>

```rust,ignore
fn copy_cursor_left(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_cursor_left" id="link-copy_cursor_left-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_cursor_left', 'Description')">
Description
</button>
</div>

<div group="copy_cursor_left" id="copy_cursor_left-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor left (`h`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_cursor_right </h2>

```rust,ignore
fn copy_cursor_right(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_cursor_right" id="link-copy_cursor_right-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_cursor_right', 'Description')">
Description
</button>
</div>

<div group="copy_cursor_right" id="copy_cursor_right-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor right (`l`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_cursor_up </h2>

```rust,ignore
fn copy_cursor_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_cursor_up" id="link-copy_cursor_up-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_cursor_up', 'Description')">
Description
</button>
</div>

<div group="copy_cursor_up" id="copy_cursor_up-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor up (`k`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_end_of_line </h2>

```rust,ignore
fn copy_end_of_line(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_end_of_line" id="link-copy_end_of_line-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_end_of_line', 'Description')">
Description
</button>
</div>

<div group="copy_end_of_line" id="copy_end_of_line-Description" class="tabcontent"  style="display: block;" >
Move to the last character of the line (`$`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_halfpage_down </h2>

```rust,ignore
fn copy_halfpage_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_halfpage_down" id="link-copy_halfpage_down-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_halfpage_down', 'Description')">
Description
</button>
</div>

<div group="copy_halfpage_down" id="copy_halfpage_down-Description" class="tabcontent"  style="display: block;" >
Scroll the view and move the cursor down half a page (`C-d`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_halfpage_up </h2>

```rust,ignore
fn copy_halfpage_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_halfpage_up" id="link-copy_halfpage_up-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_halfpage_up', 'Description')">
Description
</button>
</div>

<div group="copy_halfpage_up" id="copy_halfpage_up-Description" class="tabcontent"  style="display: block;" >
Scroll the view and move the cursor up half a page (`C-u`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_history_bottom </h2>

```rust,ignore
fn copy_history_bottom(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_history_bottom" id="link-copy_history_bottom-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_history_bottom', 'Description')">
Description
</button>
</div>

<div group="copy_history_bottom" id="copy_history_bottom-Description" class="tabcontent"  style="display: block;" >
Move to the last line of the buffer (`G`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_history_top </h2>

```rust,ignore
fn copy_history_top(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_history_top" id="link-copy_history_top-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_history_top', 'Description')">
Description
</button>
</div>

<div group="copy_history_top" id="copy_history_top-Description" class="tabcontent"  style="display: block;" >
Move to the first line of history (`gg`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_jump_again </h2>

```rust,ignore
fn copy_jump_again(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_jump_again" id="link-copy_jump_again-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_jump_again', 'Description')">
Description
</button>
</div>

<div group="copy_jump_again" id="copy_jump_again-Description" class="tabcontent"  style="display: block;" >
Repeat the last character jump (`;`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_jump_backward </h2>

```rust,ignore
fn copy_jump_backward(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_jump_backward" id="link-copy_jump_backward-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_jump_backward', 'Description')">
Description
</button>
</div>

<div group="copy_jump_backward" id="copy_jump_backward-Description" class="tabcontent"  style="display: block;" >
Wait for a character and move onto its previous occurrence on the line
(`F`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_jump_forward </h2>

```rust,ignore
fn copy_jump_forward(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_jump_forward" id="link-copy_jump_forward-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_jump_forward', 'Description')">
Description
</button>
</div>

<div group="copy_jump_forward" id="copy_jump_forward-Description" class="tabcontent"  style="display: block;" >
Wait for a character and move onto its next occurrence on the line (`f`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_jump_reverse </h2>

```rust,ignore
fn copy_jump_reverse(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_jump_reverse" id="link-copy_jump_reverse-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_jump_reverse', 'Description')">
Description
</button>
</div>

<div group="copy_jump_reverse" id="copy_jump_reverse-Description" class="tabcontent"  style="display: block;" >
Repeat the last character jump in the opposite direction (`,`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_jump_to_backward </h2>

```rust,ignore
fn copy_jump_to_backward(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_jump_to_backward" id="link-copy_jump_to_backward-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_jump_to_backward', 'Description')">
Description
</button>
</div>

<div group="copy_jump_to_backward" id="copy_jump_to_backward-Description" class="tabcontent"  style="display: block;" >
Wait for a character and move just after its previous occurrence on the
line (`T`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_jump_to_forward </h2>

```rust,ignore
fn copy_jump_to_forward(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_jump_to_forward" id="link-copy_jump_to_forward-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_jump_to_forward', 'Description')">
Description
</button>
</div>

<div group="copy_jump_to_forward" id="copy_jump_to_forward-Description" class="tabcontent"  style="display: block;" >
Wait for a character and move just before its next occurrence on the
line (`t`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_middle_line </h2>

```rust,ignore
fn copy_middle_line(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_middle_line" id="link-copy_middle_line-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_middle_line', 'Description')">
Description
</button>
</div>

<div group="copy_middle_line" id="copy_middle_line-Description" class="tabcontent"  style="display: block;" >
Move to the middle row of the view (`M`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_next_space </h2>

```rust,ignore
fn copy_next_space(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_next_space" id="link-copy_next_space-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_next_space', 'Description')">
Description
</button>
</div>

<div group="copy_next_space" id="copy_next_space-Description" class="tabcontent"  style="display: block;" >
Move to the start of the next whitespace-separated word (`W`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_next_space_end </h2>

```rust,ignore
fn copy_next_space_end(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_next_space_end" id="link-copy_next_space_end-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_next_space_end', 'Description')">
Description
</button>
</div>

<div group="copy_next_space_end" id="copy_next_space_end-Description" class="tabcontent"  style="display: block;" >
Move to the end of the next whitespace-separated word (`E`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_next_word </h2>

```rust,ignore
fn copy_next_word(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_next_word" id="link-copy_next_word-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_next_word', 'Description')">
Description
</button>
</div>

<div group="copy_next_word" id="copy_next_word-Description" class="tabcontent"  style="display: block;" >
Move to the start of the next word (`w`). Whitespace separates words, and
a run of ASCII punctuation other than `_` is a word of its own.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_next_word_end </h2>

```rust,ignore
fn copy_next_word_end(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_next_word_end" id="link-copy_next_word_end-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_next_word_end', 'Description')">
Description
</button>
</div>

<div group="copy_next_word_end" id="copy_next_word_end-Description" class="tabcontent"  style="display: block;" >
Move to the end of the next word (`e`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_page_down </h2>

```rust,ignore
fn copy_page_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_page_down" id="link-copy_page_down-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_page_down', 'Description')">
Description
</button>
</div>

<div group="copy_page_down" id="copy_page_down-Description" class="tabcontent"  style="display: block;" >
Scroll the view and move the cursor down a page (`C-f`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_page_up </h2>

```rust,ignore
fn copy_page_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_page_up" id="link-copy_page_up-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_page_up', 'Description')">
Description
</button>
</div>

<div group="copy_page_up" id="copy_page_up-Description" class="tabcontent"  style="display: block;" >
Scroll the view and move the cursor up a page (`C-b`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_previous_space </h2>

```rust,ignore
fn copy_previous_space(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_previous_space" id="link-copy_previous_space-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_previous_space', 'Description')">
Description
</button>
</div>

<div group="copy_previous_space" id="copy_previous_space-Description" class="tabcontent"  style="display: block;" >
Move to the start of the previous whitespace-separated word (`B`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_previous_word </h2>

```rust,ignore
fn copy_previous_word(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_previous_word" id="link-copy_previous_word-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_previous_word', 'Description')">
Description
</button>
</div>

<div group="copy_previous_word" id="copy_previous_word-Description" class="tabcontent"  style="display: block;" >
Move to the start of the previous word (`b`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_selection </h2>

```rust,ignore
fn copy_selection(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_selection" id="link-copy_selection-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_selection', 'Description')">
Description
</button>
</div>

<div group="copy_selection" id="copy_selection-Description" class="tabcontent"  style="display: block;" >
Copy the current selection into the clipboard (OSC 52) and leave copy
mode. Same as `yank_selection`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_start_of_line </h2>

```rust,ignore
fn copy_start_of_line(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_start_of_line" id="link-copy_start_of_line-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_start_of_line', 'Description')">
Description
</button>
</div>

<div group="copy_start_of_line" id="copy_start_of_line-Description" class="tabcontent"  style="display: block;" >
Move to the first column of the line (`0`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> copy_top_line </h2>

```rust,ignore
fn copy_top_line(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="copy_top_line" id="link-copy_top_line-Description"  class="tablinks active"
    onclick="openTab(event, 'copy_top_line', 'Description')">
Description
</button>
</div>

<div group="copy_top_line" id="copy_top_line-Description" class="tabcontent"  style="display: block;" >
Move to the top row of the view (`H`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> detach_buffer </h2>

```rust,ignore
fn detach_buffer(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="detach_buffer" id="link-detach_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'detach_buffer', 'Description')">
Description
</button>
</div>

<div group="detach_buffer" id="detach_buffer-Description" class="tabcontent"  style="display: block;" >
Detach the currently focused buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> detach_buffer_id </h2>

```rust,ignore
fn detach_buffer_id(_: ActionApi, buffer_id: int) -> Action
```

<div>
<div class="tab">
<button group="detach_buffer_id" id="link-detach_buffer_id-Description"  class="tablinks active"
    onclick="openTab(event, 'detach_buffer_id', 'Description')">
Description
</button>
</div>

<div group="detach_buffer_id" id="detach_buffer_id-Description" class="tabcontent"  style="display: block;" >
Detach a buffer by id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> display_message </h2>

```rust,ignore
fn display_message(_: ActionApi, message: String) -> Action
fn display_message(_: ActionApi, message: String, duration_ms: int) -> Action
```

<div>
<div class="tab">
<button group="display_message" id="link-display_message-Description"  class="tablinks active"
    onclick="openTab(event, 'display_message', 'Description')">
Description
</button>
<button group="display_message" id="link-display_message-Example"  class="tablinks"
    onclick="openTab(event, 'display_message', 'Example')">
Example
</button>
</div>

<div group="display_message" id="display_message-Description" class="tabcontent"  style="display: block;" >
Show a message in the status line for a few seconds, like tmux
`display-message`. The text may use the format variables `#S` (session),
`#I` (window number), `#W` (window name), `#D` (pane id), `#T` (pane
title), `#{pane_current_path}`, `#{pane_current_command}` and
`#{@name}` (the pane's buffer option `name`), expanded for the focused
pane; `##` is a literal `#`.
</div>
<div group="display_message" id="display_message-Example" class="tabcontent"  style="display: none;" >

```rhai
action.display_message("#S:#I #{pane_current_path}")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_copy_mode </h2>

```rust,ignore
fn enter_copy_mode(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_copy_mode" id="link-enter_copy_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_copy_mode', 'Description')">
Description
</button>
</div>

<div group="enter_copy_mode" id="enter_copy_mode-Description" class="tabcontent"  style="display: block;" >
Enter copy mode on the focused pane (tmux `copy-mode`). The view stops
following output and a cursor appears at the program's cursor; copy-mode
motions move it. Does nothing on the alternate screen.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_hints </h2>

```rust,ignore
fn enter_hints(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_hints" id="link-enter_hints-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_hints', 'Description')">
Description
</button>
<button group="enter_hints" id="link-enter_hints-Example"  class="tablinks"
    onclick="openTab(event, 'enter_hints', 'Example')">
Example
</button>
</div>

<div group="enter_hints" id="enter_hints-Description" class="tabcontent"  style="display: block;" >
Enter thumbs-style hint mode: label the matches in the visible pane and
copy the selected one to the clipboard (OSC 52).
</div>
<div group="enter_hints" id="enter_hints-Example" class="tabcontent"  style="display: none;" >

```rhai
action.enter_hints()
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_hints_with </h2>

```rust,ignore
fn enter_hints_with(_: ActionApi, action: String) -> Action
```

<div>
<div class="tab">
<button group="enter_hints_with" id="link-enter_hints_with-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_hints_with', 'Description')">
Description
</button>
<button group="enter_hints_with" id="link-enter_hints_with-Example"  class="tablinks"
    onclick="openTab(event, 'enter_hints_with', 'Example')">
Example
</button>
</div>

<div group="enter_hints_with" id="enter_hints_with-Description" class="tabcontent"  style="display: block;" >
Enter hint mode, invoking the named action with the selected text exposed
as `ctx.hint_selection()` instead of copying it.
</div>
<div group="enter_hints_with" id="enter_hints_with-Example" class="tabcontent"  style="display: none;" >

```rhai
action.enter_hints_with("open-url")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_mode </h2>

```rust,ignore
fn enter_mode(_: ActionApi, mode: String) -> Action
```

<div>
<div class="tab">
<button group="enter_mode" id="link-enter_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_mode', 'Description')">
Description
</button>
</div>

<div group="enter_mode" id="enter_mode-Description" class="tabcontent"  style="display: block;" >
Enter a specific input mode by name.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_search_backward </h2>

```rust,ignore
fn enter_search_backward(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_search_backward" id="link-enter_search_backward-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_search_backward', 'Description')">
Description
</button>
</div>

<div group="enter_search_backward" id="enter_search_backward-Description" class="tabcontent"  style="display: block;" >
Enter incremental search mode, searching backward (`?`). After a backward
search `search_next` moves to earlier matches and `search_prev` to later
ones, as in vim.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_search_mode </h2>

```rust,ignore
fn enter_search_mode(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_search_mode" id="link-enter_search_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_search_mode', 'Description')">
Description
</button>
</div>

<div group="enter_search_mode" id="enter_search_mode-Description" class="tabcontent"  style="display: block;" >
Enter incremental search mode, searching forward (`/`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_select_block </h2>

```rust,ignore
fn enter_select_block(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_select_block" id="link-enter_select_block-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_select_block', 'Description')">
Description
</button>
</div>

<div group="enter_select_block" id="enter_select_block-Description" class="tabcontent"  style="display: block;" >
Start a block (rectangle) selection at the copy-mode cursor. Toggles
like `enter_select_char`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_select_char </h2>

```rust,ignore
fn enter_select_char(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_select_char" id="link-enter_select_char-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_select_char', 'Description')">
Description
</button>
</div>

<div group="enter_select_char" id="enter_select_char-Description" class="tabcontent"  style="display: block;" >
Start a character selection at the copy-mode cursor, entering copy mode
first if needed. With a character selection already active this clears
it; with another kind active it switches to character selection.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> enter_select_line </h2>

```rust,ignore
fn enter_select_line(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="enter_select_line" id="link-enter_select_line-Description"  class="tablinks active"
    onclick="openTab(event, 'enter_select_line', 'Description')">
Description
</button>
</div>

<div group="enter_select_line" id="enter_select_line-Description" class="tabcontent"  style="display: block;" >
Start a line selection at the copy-mode cursor. Toggles like
`enter_select_char`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> exit_copy_mode </h2>

```rust,ignore
fn exit_copy_mode(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="exit_copy_mode" id="link-exit_copy_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'exit_copy_mode', 'Description')">
Description
</button>
</div>

<div group="exit_copy_mode" id="exit_copy_mode-Description" class="tabcontent"  style="display: block;" >
Leave copy mode: drop the cursor and any selection, follow output again,
and return to `normal` mode (tmux `cancel`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> focus_buffer </h2>

```rust,ignore
fn focus_buffer(_: ActionApi, buffer_id: int) -> Action
```

<div>
<div class="tab">
<button group="focus_buffer" id="link-focus_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'focus_buffer', 'Description')">
Description
</button>
</div>

<div group="focus_buffer" id="focus_buffer-Description" class="tabcontent"  style="display: block;" >
Focus a specific buffer by id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> focus_down </h2>

```rust,ignore
fn focus_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="focus_down" id="link-focus_down-Description"  class="tablinks active"
    onclick="openTab(event, 'focus_down', 'Description')">
Description
</button>
</div>

<div group="focus_down" id="focus_down-Description" class="tabcontent"  style="display: block;" >
Focus the view below the current node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> focus_left </h2>

```rust,ignore
fn focus_left(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="focus_left" id="link-focus_left-Description"  class="tablinks active"
    onclick="openTab(event, 'focus_left', 'Description')">
Description
</button>
<button group="focus_left" id="link-focus_left-Example"  class="tablinks"
    onclick="openTab(event, 'focus_left', 'Example')">
Example
</button>
</div>

<div group="focus_left" id="focus_left-Description" class="tabcontent"  style="display: block;" >
Focus the view to the left of the current node.
</div>
<div group="focus_left" id="focus_left-Example" class="tabcontent"  style="display: none;" >

```rhai
action.focus_left()
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> focus_right </h2>

```rust,ignore
fn focus_right(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="focus_right" id="link-focus_right-Description"  class="tablinks active"
    onclick="openTab(event, 'focus_right', 'Description')">
Description
</button>
</div>

<div group="focus_right" id="focus_right-Description" class="tabcontent"  style="display: block;" >
Focus the view to the right of the current node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> focus_up </h2>

```rust,ignore
fn focus_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="focus_up" id="link-focus_up-Description"  class="tablinks active"
    onclick="openTab(event, 'focus_up', 'Description')">
Description
</button>
</div>

<div group="focus_up" id="focus_up-Description" class="tabcontent"  style="display: block;" >
Focus the view above the current node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> follow_output </h2>

```rust,ignore
fn follow_output(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="follow_output" id="link-follow_output-Description"  class="tablinks active"
    onclick="openTab(event, 'follow_output', 'Description')">
Description
</button>
</div>

<div group="follow_output" id="follow_output-Description" class="tabcontent"  style="display: block;" >
Re-enable following live output.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> insert_tab_after </h2>

```rust,ignore
fn insert_tab_after(_: ActionApi, tabs_node_id: int, title: String, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="insert_tab_after" id="link-insert_tab_after-Description"  class="tablinks active"
    onclick="openTab(event, 'insert_tab_after', 'Description')">
Description
</button>
</div>

<div group="insert_tab_after" id="insert_tab_after-Description" class="tabcontent"  style="display: block;" >
Insert a tab after a specific tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> insert_tab_after_current </h2>

```rust,ignore
fn insert_tab_after_current(_: ActionApi, title: String, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="insert_tab_after_current" id="link-insert_tab_after_current-Description"  class="tablinks active"
    onclick="openTab(event, 'insert_tab_after_current', 'Description')">
Description
</button>
<button group="insert_tab_after_current" id="link-insert_tab_after_current-Example"  class="tablinks"
    onclick="openTab(event, 'insert_tab_after_current', 'Example')">
Example
</button>
</div>

<div group="insert_tab_after_current" id="insert_tab_after_current-Description" class="tabcontent"  style="display: block;" >
Insert a tab after the active tab of the innermost tabs that hold the
focus, and select it. When the focus is inside nested tabs, the new tab
is a nested tab there, not a window; use `new_window` for tmux's
`new-window`. Otherwise the new tab is a window, after the current one.
</div>
<div group="insert_tab_after_current" id="insert_tab_after_current-Example" class="tabcontent"  style="display: none;" >

```rhai
action.insert_tab_after_current("logs", tree.buffer_spawn(["tail", "-f", "app.log"]))
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> insert_tab_before </h2>

```rust,ignore
fn insert_tab_before(_: ActionApi, tabs_node_id: int, title: String, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="insert_tab_before" id="link-insert_tab_before-Description"  class="tablinks active"
    onclick="openTab(event, 'insert_tab_before', 'Description')">
Description
</button>
</div>

<div group="insert_tab_before" id="insert_tab_before-Description" class="tabcontent"  style="display: block;" >
Insert a tab before a specific tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> insert_tab_before_current </h2>

```rust,ignore
fn insert_tab_before_current(_: ActionApi, title: String, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="insert_tab_before_current" id="link-insert_tab_before_current-Description"  class="tablinks active"
    onclick="openTab(event, 'insert_tab_before_current', 'Description')">
Description
</button>
</div>

<div group="insert_tab_before_current" id="insert_tab_before_current-Description" class="tabcontent"  style="display: block;" >
Insert a tab before the active tab of the innermost tabs that hold the
focus, and select it. Like `insert_tab_after_current`, this makes a
nested tab when the focus is in nested tabs, and otherwise a window,
before the current one.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> join_buffer_here </h2>

```rust,ignore
fn join_buffer_here(_: ActionApi, buffer_id: int, placement: "tab-after" | "tab-before" | "left" | "right" | "up" | "down") -> Action
```

<div>
<div class="tab">
<button group="join_buffer_here" id="link-join_buffer_here-Description"  class="tablinks active"
    onclick="openTab(event, 'join_buffer_here', 'Description')">
Description
</button>
</div>

<div group="join_buffer_here" id="join_buffer_here-Description" class="tabcontent"  style="display: block;" >
Attach a buffer at the current node.
`placement` accepts `tab-after`, `tab-before`, `left`, `right`, `up`, or `down`.
Example: `action.join_buffer_here(12, "tab-after")`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> kill_buffer </h2>

```rust,ignore
fn kill_buffer(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="kill_buffer" id="link-kill_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'kill_buffer', 'Description')">
Description
</button>
</div>

<div group="kill_buffer" id="kill_buffer-Description" class="tabcontent"  style="display: block;" >
Kill the currently focused buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> kill_buffer_id </h2>

```rust,ignore
fn kill_buffer_id(_: ActionApi, buffer_id: int) -> Action
```

<div>
<div class="tab">
<button group="kill_buffer_id" id="link-kill_buffer_id-Description"  class="tablinks active"
    onclick="openTab(event, 'kill_buffer_id', 'Description')">
Description
</button>
</div>

<div group="kill_buffer_id" id="kill_buffer_id-Description" class="tabcontent"  style="display: block;" >
Kill a buffer by id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> kill_floating </h2>

```rust,ignore
fn kill_floating(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="kill_floating" id="link-kill_floating-Description"  class="tablinks active"
    onclick="openTab(event, 'kill_floating', 'Description')">
Description
</button>
</div>

<div group="kill_floating" id="kill_floating-Description" class="tabcontent"  style="display: block;" >
Close the currently focused floating window and end the programs of the
buffers it shows, like tmux's `kill-pane` for a popup.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> kill_floating_id </h2>

```rust,ignore
fn kill_floating_id(_: ActionApi, floating_id: int) -> Action
```

<div>
<div class="tab">
<button group="kill_floating_id" id="link-kill_floating_id-Description"  class="tablinks active"
    onclick="openTab(event, 'kill_floating_id', 'Description')">
Description
</button>
</div>

<div group="kill_floating_id" id="kill_floating_id-Description" class="tabcontent"  style="display: block;" >
Close a floating window by id and end the programs of the buffers it
shows.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> kill_node </h2>

```rust,ignore
fn kill_node(_: ActionApi, node_id: int) -> Action
```

<div>
<div class="tab">
<button group="kill_node" id="link-kill_node-Description"  class="tablinks active"
    onclick="openTab(event, 'kill_node', 'Description')">
Description
</button>
</div>

<div group="kill_node" id="kill_node-Description" class="tabcontent"  style="display: block;" >
Close a view by node id and end the programs of the buffers it shows. On
a split or tabs node this ends every pane in it, like tmux's
`kill-window`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> kill_view </h2>

```rust,ignore
fn kill_view(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="kill_view" id="link-kill_view-Description"  class="tablinks active"
    onclick="openTab(event, 'kill_view', 'Description')">
Description
</button>
</div>

<div group="kill_view" id="kill_view-Description" class="tabcontent"  style="display: block;" >
Close the currently focused view and end its buffer's program, like
tmux's `kill-pane`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> last_pane </h2>

```rust,ignore
fn last_pane(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="last_pane" id="link-last_pane-Description"  class="tablinks active"
    onclick="openTab(event, 'last_pane', 'Description')">
Description
</button>
<button group="last_pane" id="link-last_pane-Example"  class="tablinks"
    onclick="openTab(event, 'last_pane', 'Example')">
Example
</button>
</div>

<div group="last_pane" id="last_pane-Description" class="tabcontent"  style="display: block;" >
Focus the pane that was focused before the current one in the current
window, like tmux `select-pane -l`. Repeating it goes back and forth
between two panes. Panes that were closed are skipped; with no other
pane to go back to it does nothing.
</div>
<div group="last_pane" id="last_pane-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>;", action.last_pane());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> last_session </h2>

```rust,ignore
fn last_session(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="last_session" id="link-last_session-Description"  class="tablinks active"
    onclick="openTab(event, 'last_session', 'Description')">
Description
</button>
</div>

<div group="last_session" id="last_session-Description" class="tabcontent"  style="display: block;" >
Switch back to the previously active session (tmux `switch-client -l`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> leave_mode </h2>

```rust,ignore
fn leave_mode(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="leave_mode" id="link-leave_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'leave_mode', 'Description')">
Description
</button>
</div>

<div group="leave_mode" id="leave_mode-Description" class="tabcontent"  style="display: block;" >
Leave the active input mode.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> move_buffer_to_floating </h2>

```rust,ignore
fn move_buffer_to_floating(_: ActionApi, buffer_id: int, options: Map) -> Action
```

<div>
<div class="tab">
<button group="move_buffer_to_floating" id="link-move_buffer_to_floating-Description"  class="tablinks active"
    onclick="openTab(event, 'move_buffer_to_floating', 'Description')">
Description
</button>
<button group="move_buffer_to_floating" id="link-move_buffer_to_floating-Options"  class="tablinks"
    onclick="openTab(event, 'move_buffer_to_floating', 'Options')">
Options
</button>
</div>

<div group="move_buffer_to_floating" id="move_buffer_to_floating-Description" class="tabcontent"  style="display: block;" >
Move a buffer into a new floating window.
</div>
<div group="move_buffer_to_floating" id="move_buffer_to_floating-Options" class="tabcontent"  style="display: none;" >

- `x` (i16): horizontal offset from the anchor (default: 0)
- `y` (i16): vertical offset from the anchor (default: 0)
- `width` (FloatingSize): window width, as a percentage (e.g., 50%) or pixel value (default: 50%)
- `height` (FloatingSize): window height, as a percentage or pixel value (default: 50%)
- `anchor` (FloatingAnchor): anchor point for positioning, e.g., "top_left", "center" (default: center)
- `title` (Option\<String\>): window title (default: none)
- `focus` (bool): whether to focus the window after creation (default: true)
- `close_on_empty` (bool): whether to close the window when its buffer empties (default: true)
- `border` (bool): frame the window and show its title; without a frame its content fills the window, like tmux's `display-popup -B` (default: true)
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> move_buffer_to_node </h2>

```rust,ignore
fn move_buffer_to_node(_: ActionApi, buffer_id: int, node_id: int) -> Action
```

<div>
<div class="tab">
<button group="move_buffer_to_node" id="link-move_buffer_to_node-Description"  class="tablinks active"
    onclick="openTab(event, 'move_buffer_to_node', 'Description')">
Description
</button>
</div>

<div group="move_buffer_to_node" id="move_buffer_to_node-Description" class="tabcontent"  style="display: block;" >
Move a buffer into a specific node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> move_current_node_after </h2>

```rust,ignore
fn move_current_node_after(_: ActionApi, sibling_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="move_current_node_after" id="link-move_current_node_after-Description"  class="tablinks active"
    onclick="openTab(event, 'move_current_node_after', 'Description')">
Description
</button>
</div>

<div group="move_current_node_after" id="move_current_node_after-Description" class="tabcontent"  style="display: block;" >
Move the current node after a sibling.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> move_current_node_before </h2>

```rust,ignore
fn move_current_node_before(_: ActionApi, sibling_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="move_current_node_before" id="link-move_current_node_before-Description"  class="tablinks active"
    onclick="openTab(event, 'move_current_node_before', 'Description')">
Description
</button>
</div>

<div group="move_current_node_before" id="move_current_node_before-Description" class="tabcontent"  style="display: block;" >
Move the current node before a sibling.
Use this when the current node is the one being repositioned.
Example: `action.move_current_node_before(42)`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> move_node_after </h2>

```rust,ignore
fn move_node_after(_: ActionApi, node_id: int, sibling_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="move_node_after" id="link-move_node_after-Description"  class="tablinks active"
    onclick="openTab(event, 'move_node_after', 'Description')">
Description
</button>
</div>

<div group="move_node_after" id="move_node_after-Description" class="tabcontent"  style="display: block;" >
Move a node after a sibling.
Use this when you need to move a specific node id instead of the current node.
Example: `action.move_node_after(10, 42)`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> move_node_before </h2>

```rust,ignore
fn move_node_before(_: ActionApi, node_id: int, sibling_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="move_node_before" id="link-move_node_before-Description"  class="tablinks active"
    onclick="openTab(event, 'move_node_before', 'Description')">
Description
</button>
</div>

<div group="move_node_before" id="move_node_before-Description" class="tabcontent"  style="display: block;" >
Move a node before a sibling.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> new_window </h2>

```rust,ignore
fn new_window(_: ActionApi, title: String, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="new_window" id="link-new_window-Description"  class="tablinks active"
    onclick="openTab(event, 'new_window', 'Description')">
Description
</button>
<button group="new_window" id="link-new_window-Example"  class="tablinks"
    onclick="openTab(event, 'new_window', 'Example')">
Example
</button>
</div>

<div group="new_window" id="new_window-Description" class="tabcontent"  style="display: block;" >
Add a window to the current session, after its last window, and
select it (tmux `new-window`). Unlike `insert_tab_after_current`, the
new tab is always a window, even when the focus is in nested tabs.
</div>
<div group="new_window" id="new_window-Example" class="tabcontent"  style="display: none;" >

```rhai
action.new_window("logs", tree.buffer_spawn(["tail", "-f", "app.log"]))
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> next_current_tabs </h2>

```rust,ignore
fn next_current_tabs(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="next_current_tabs" id="link-next_current_tabs-Description"  class="tablinks active"
    onclick="openTab(event, 'next_current_tabs', 'Description')">
Description
</button>
</div>

<div group="next_current_tabs" id="next_current_tabs-Description" class="tabcontent"  style="display: block;" >
Select the next tab in the currently focused tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> next_session </h2>

```rust,ignore
fn next_session(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="next_session" id="link-next_session-Description"  class="tablinks active"
    onclick="openTab(event, 'next_session', 'Description')">
Description
</button>
</div>

<div group="next_session" id="next_session-Description" class="tabcontent"  style="display: block;" >
Switch to the next session in list order, wrapping around.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> next_tab </h2>

```rust,ignore
fn next_tab(_: ActionApi, tabs_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="next_tab" id="link-next_tab-Description"  class="tablinks active"
    onclick="openTab(event, 'next_tab', 'Description')">
Description
</button>
</div>

<div group="next_tab" id="next_tab-Description" class="tabcontent"  style="display: block;" >
Select the next tab in a specific tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> noop </h2>

```rust,ignore
fn noop(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="noop" id="link-noop-Description"  class="tablinks active"
    onclick="openTab(event, 'noop', 'Description')">
Description
</button>
</div>

<div group="noop" id="noop-Description" class="tabcontent"  style="display: block;" >
Build a no-op action.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> notify </h2>

```rust,ignore
fn notify(_: ActionApi, level: "info" | "warn" | "error", message: String) -> Action
```

<div>
<div class="tab">
<button group="notify" id="link-notify-Description"  class="tablinks active"
    onclick="openTab(event, 'notify', 'Description')">
Description
</button>
</div>

<div group="notify" id="notify-Description" class="tabcontent"  style="display: block;" >
Emit a client notification.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> open_buffer_history </h2>

```rust,ignore
fn open_buffer_history(_: ActionApi, buffer_id: int, scope: "visible" | "full", placement: "floating" | "tab") -> Action
```

<div>
<div class="tab">
<button group="open_buffer_history" id="link-open_buffer_history-Description"  class="tablinks active"
    onclick="openTab(event, 'open_buffer_history', 'Description')">
Description
</button>
</div>

<div group="open_buffer_history" id="open_buffer_history-Description" class="tabcontent"  style="display: block;" >
Open the history of a buffer in a new view.
`scope` accepts `visible` or `full`. `placement` accepts `floating` or `tab`.
Example: `action.open_buffer_history(12, "visible", "floating")`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> open_floating </h2>

```rust,ignore
fn open_floating(_: ActionApi, tree: TreeSpec, options: Map) -> Action
```

<div>
<div class="tab">
<button group="open_floating" id="link-open_floating-Description"  class="tablinks active"
    onclick="openTab(event, 'open_floating', 'Description')">
Description
</button>
<button group="open_floating" id="link-open_floating-Example"  class="tablinks"
    onclick="openTab(event, 'open_floating', 'Example')">
Example
</button>
</div>

<div group="open_floating" id="open_floating-Description" class="tabcontent"  style="display: block;" >
Open a floating view around the provided tree. Takes the options of
`move_buffer_to_floating`; `border: false` opens it without a frame,
its content filling the whole window, like tmux's `display-popup -B`.
</div>
<div group="open_floating" id="open_floating-Example" class="tabcontent"  style="display: none;" >

```rhai
action.open_floating(tree.buffer_spawn(["lazygit"]), #{
    width: "90%", height: "90%", border: false
})
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> prev_current_tabs </h2>

```rust,ignore
fn prev_current_tabs(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="prev_current_tabs" id="link-prev_current_tabs-Description"  class="tablinks active"
    onclick="openTab(event, 'prev_current_tabs', 'Description')">
Description
</button>
</div>

<div group="prev_current_tabs" id="prev_current_tabs-Description" class="tabcontent"  style="display: block;" >
Select the previous tab in the currently focused tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> prev_session </h2>

```rust,ignore
fn prev_session(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="prev_session" id="link-prev_session-Description"  class="tablinks active"
    onclick="openTab(event, 'prev_session', 'Description')">
Description
</button>
</div>

<div group="prev_session" id="prev_session-Description" class="tabcontent"  style="display: block;" >
Switch to the previous session in list order, wrapping around.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> prev_tab </h2>

```rust,ignore
fn prev_tab(_: ActionApi, tabs_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="prev_tab" id="link-prev_tab-Description"  class="tablinks active"
    onclick="openTab(event, 'prev_tab', 'Description')">
Description
</button>
</div>

<div group="prev_tab" id="prev_tab-Description" class="tabcontent"  style="display: block;" >
Select the previous tab in a specific tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> rename_session_prompt </h2>

```rust,ignore
fn rename_session_prompt(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="rename_session_prompt" id="link-rename_session_prompt-Description"  class="tablinks active"
    onclick="openTab(event, 'rename_session_prompt', 'Description')">
Description
</button>
<button group="rename_session_prompt" id="link-rename_session_prompt-Example"  class="tablinks"
    onclick="openTab(event, 'rename_session_prompt', 'Example')">
Example
</button>
</div>

<div group="rename_session_prompt" id="rename_session_prompt-Description" class="tabcontent"  style="display: block;" >
Prompt for a new name for the current session, starting from its
current name (tmux `prefix $`).
</div>
<div group="rename_session_prompt" id="rename_session_prompt-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>$", action.rename_session_prompt());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> rename_window_prompt </h2>

```rust,ignore
fn rename_window_prompt(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="rename_window_prompt" id="link-rename_window_prompt-Description"  class="tablinks active"
    onclick="openTab(event, 'rename_window_prompt', 'Description')">
Description
</button>
<button group="rename_window_prompt" id="link-rename_window_prompt-Example"  class="tablinks"
    onclick="openTab(event, 'rename_window_prompt', 'Example')">
Example
</button>
</div>

<div group="rename_window_prompt" id="rename_window_prompt-Description" class="tabcontent"  style="display: block;" >
Prompt for a new name for the current window, starting from its current
name (tmux `prefix ,`).
</div>
<div group="rename_window_prompt" id="rename_window_prompt-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>,", action.rename_window_prompt());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> replace_current_with </h2>

```rust,ignore
fn replace_current_with(_: ActionApi, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="replace_current_with" id="link-replace_current_with-Description"  class="tablinks active"
    onclick="openTab(event, 'replace_current_with', 'Description')">
Description
</button>
</div>

<div group="replace_current_with" id="replace_current_with-Description" class="tabcontent"  style="display: block;" >
Replace the focused node with a new tree.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> replace_node </h2>

```rust,ignore
fn replace_node(_: ActionApi, node_id: int, tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="replace_node" id="link-replace_node-Description"  class="tablinks active"
    onclick="openTab(event, 'replace_node', 'Description')">
Description
</button>
</div>

<div group="replace_node" id="replace_node-Description" class="tabcontent"  style="display: block;" >
Replace a specific node by id with a new tree.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> resize_down </h2>

```rust,ignore
fn resize_down(_: ActionApi, cells: int) -> Action
```

<div>
<div class="tab">
<button group="resize_down" id="link-resize_down-Description"  class="tablinks active"
    onclick="openTab(event, 'resize_down', 'Description')">
Description
</button>
</div>

<div group="resize_down" id="resize_down-Description" class="tabcontent"  style="display: block;" >
Move the focused pane's border `cells` rows down (tmux `resize-pane -D`).
See `resize_left` for which divider moves.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> resize_left </h2>

```rust,ignore
fn resize_left(_: ActionApi, cells: int) -> Action
```

<div>
<div class="tab">
<button group="resize_left" id="link-resize_left-Description"  class="tablinks active"
    onclick="openTab(event, 'resize_left', 'Description')">
Description
</button>
<button group="resize_left" id="link-resize_left-Example"  class="tablinks"
    onclick="openTab(event, 'resize_left', 'Example')">
Example
</button>
</div>

<div group="resize_left" id="resize_left-Description" class="tabcontent"  style="display: block;" >
Move the focused pane's border `cells` cells left (tmux `resize-pane -L`).

The nearest split with panes side by side is resized: the divider to the right of the
pane moves left, shrinking the pane, or, when the pane is the split's rightmost child,
the divider to its left moves left, growing it. Panes keep at least one column of
content, and the action does nothing when there is no such split or the divider cannot
move. While a pane is zoomed only splits inside the zoomed view can be resized.
</div>
<div group="resize_left" id="resize_left-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<M-h>", action.resize_left(3));
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> resize_right </h2>

```rust,ignore
fn resize_right(_: ActionApi, cells: int) -> Action
```

<div>
<div class="tab">
<button group="resize_right" id="link-resize_right-Description"  class="tablinks active"
    onclick="openTab(event, 'resize_right', 'Description')">
Description
</button>
</div>

<div group="resize_right" id="resize_right-Description" class="tabcontent"  style="display: block;" >
Move the focused pane's border `cells` cells right (tmux `resize-pane -R`).
See `resize_left` for which divider moves.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> resize_up </h2>

```rust,ignore
fn resize_up(_: ActionApi, cells: int) -> Action
```

<div>
<div class="tab">
<button group="resize_up" id="link-resize_up-Description"  class="tablinks active"
    onclick="openTab(event, 'resize_up', 'Description')">
Description
</button>
</div>

<div group="resize_up" id="resize_up-Description" class="tabcontent"  style="display: block;" >
Move the focused pane's border `cells` rows up (tmux `resize-pane -U`).
See `resize_left` for which divider moves; panes keep at least one row of content,
and their title row if they have one.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> reveal_buffer </h2>

```rust,ignore
fn reveal_buffer(_: ActionApi, buffer_id: int) -> Action
```

<div>
<div class="tab">
<button group="reveal_buffer" id="link-reveal_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'reveal_buffer', 'Description')">
Description
</button>
</div>

<div group="reveal_buffer" id="reveal_buffer-Description" class="tabcontent"  style="display: block;" >
Reveal a specific buffer by id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> run_command </h2>

```rust,ignore
fn run_command(_: ActionApi, command: String) -> Action
```

<div>
<div class="tab">
<button group="run_command" id="link-run_command-Description"  class="tablinks active"
    onclick="openTab(event, 'run_command', 'Description')">
Description
</button>
<button group="run_command" id="link-run_command-Example"  class="tablinks"
    onclick="openTab(event, 'run_command', 'Example')">
Example
</button>
</div>

<div group="run_command" id="run_command-Description" class="tabcontent"  style="display: block;" >
Run a command line as if it was typed at the command prompt, like
binding a tmux command. Output and errors are shown in the status line.
</div>
<div group="run_command" id="run_command-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>E", action.run_command("set-environment -g EDITOR nvim"));
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> run_named_action </h2>

```rust,ignore
fn run_named_action(_: ActionApi, name: String) -> Action
```

<div>
<div class="tab">
<button group="run_named_action" id="link-run_named_action-Description"  class="tablinks active"
    onclick="openTab(event, 'run_named_action', 'Description')">
Description
</button>
</div>

<div group="run_named_action" id="run_named_action-Description" class="tabcontent"  style="display: block;" >
Run another named action by name.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> run_shell </h2>

```rust,ignore
fn run_shell(_: ActionApi, command: String) -> Action
```

<div>
<div class="tab">
<button group="run_shell" id="link-run_shell-Description"  class="tablinks active"
    onclick="openTab(event, 'run_shell', 'Description')">
Description
</button>
<button group="run_shell" id="link-run_shell-Example"  class="tablinks"
    onclick="openTab(event, 'run_shell', 'Example')">
Example
</button>
</div>

<div group="run_shell" id="run_shell-Description" class="tabcontent"  style="display: block;" >
Run a shell command line in the client's login context (tmux `run-shell`).

The string is executed as `/bin/sh -lc "<command>"`. Output is discarded;
the spawned tool can drive Embers via `$EMBERS_SOCKET` and the `embers` CLI.
`$EMBERS_CLIENT` and `$EMBERS_BUFFER` name the client the binding was
pressed in and its focused buffer.
</div>
<div group="run_shell" id="run_shell-Example" class="tabcontent"  style="display: none;" >

```rhai
action.run_shell("wisp popup")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> run_shell_argv </h2>

```rust,ignore
fn run_shell_argv(_: ActionApi, argv: Array) -> Action
```

<div>
<div class="tab">
<button group="run_shell_argv" id="link-run_shell_argv-Description"  class="tablinks active"
    onclick="openTab(event, 'run_shell_argv', 'Description')">
Description
</button>
<button group="run_shell_argv" id="link-run_shell_argv-Example"  class="tablinks"
    onclick="openTab(event, 'run_shell_argv', 'Example')">
Example
</button>
</div>

<div group="run_shell_argv" id="run_shell_argv-Description" class="tabcontent"  style="display: block;" >
Run a command given as an explicit argv array, without a shell.
</div>
<div group="run_shell_argv" id="run_shell_argv-Example" class="tabcontent"  style="display: none;" >

```rhai
action.run_shell_argv(["wisp", "popup"])
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> scroll_line_down </h2>

```rust,ignore
fn scroll_line_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="scroll_line_down" id="link-scroll_line_down-Description"  class="tablinks active"
    onclick="openTab(event, 'scroll_line_down', 'Description')">
Description
</button>
</div>

<div group="scroll_line_down" id="scroll_line_down-Description" class="tabcontent"  style="display: block;" >
Scroll one line downward in local scrollback.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> scroll_line_up </h2>

```rust,ignore
fn scroll_line_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="scroll_line_up" id="link-scroll_line_up-Description"  class="tablinks active"
    onclick="openTab(event, 'scroll_line_up', 'Description')">
Description
</button>
</div>

<div group="scroll_line_up" id="scroll_line_up-Description" class="tabcontent"  style="display: block;" >
Scroll one line upward in local scrollback.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> scroll_page_down </h2>

```rust,ignore
fn scroll_page_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="scroll_page_down" id="link-scroll_page_down-Description"  class="tablinks active"
    onclick="openTab(event, 'scroll_page_down', 'Description')">
Description
</button>
</div>

<div group="scroll_page_down" id="scroll_page_down-Description" class="tabcontent"  style="display: block;" >
Scroll one page downward in local scrollback.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> scroll_page_up </h2>

```rust,ignore
fn scroll_page_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="scroll_page_up" id="link-scroll_page_up-Description"  class="tablinks active"
    onclick="openTab(event, 'scroll_page_up', 'Description')">
Description
</button>
</div>

<div group="scroll_page_up" id="scroll_page_up-Description" class="tabcontent"  style="display: block;" >
Scroll one page upward in local scrollback.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> scroll_to_bottom </h2>

```rust,ignore
fn scroll_to_bottom(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="scroll_to_bottom" id="link-scroll_to_bottom-Description"  class="tablinks active"
    onclick="openTab(event, 'scroll_to_bottom', 'Description')">
Description
</button>
</div>

<div group="scroll_to_bottom" id="scroll_to_bottom-Description" class="tabcontent"  style="display: block;" >
Scroll to the bottom of local scrollback.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> scroll_to_top </h2>

```rust,ignore
fn scroll_to_top(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="scroll_to_top" id="link-scroll_to_top-Description"  class="tablinks active"
    onclick="openTab(event, 'scroll_to_top', 'Description')">
Description
</button>
</div>

<div group="scroll_to_top" id="scroll_to_top-Description" class="tabcontent"  style="display: block;" >
Scroll to the top of local scrollback.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> search_next </h2>

```rust,ignore
fn search_next(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="search_next" id="link-search_next-Description"  class="tablinks active"
    onclick="openTab(event, 'search_next', 'Description')">
Description
</button>
</div>

<div group="search_next" id="search_next-Description" class="tabcontent"  style="display: block;" >
Jump to the next search match.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> search_prev </h2>

```rust,ignore
fn search_prev(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="search_prev" id="link-search_prev-Description"  class="tablinks active"
    onclick="openTab(event, 'search_prev', 'Description')">
Description
</button>
</div>

<div group="search_prev" id="search_prev-Description" class="tabcontent"  style="display: block;" >
Jump to the previous search match.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_current_tabs </h2>

```rust,ignore
fn select_current_tabs(_: ActionApi, index: int) -> Action
```

<div>
<div class="tab">
<button group="select_current_tabs" id="link-select_current_tabs-Description"  class="tablinks active"
    onclick="openTab(event, 'select_current_tabs', 'Description')">
Description
</button>
</div>

<div group="select_current_tabs" id="select_current_tabs-Description" class="tabcontent"  style="display: block;" >
Select a tab by index in the currently focused tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_move_down </h2>

```rust,ignore
fn select_move_down(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="select_move_down" id="link-select_move_down-Description"  class="tablinks active"
    onclick="openTab(event, 'select_move_down', 'Description')">
Description
</button>
</div>

<div group="select_move_down" id="select_move_down-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor down. Same as `copy_cursor_down`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_move_left </h2>

```rust,ignore
fn select_move_left(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="select_move_left" id="link-select_move_left-Description"  class="tablinks active"
    onclick="openTab(event, 'select_move_left', 'Description')">
Description
</button>
</div>

<div group="select_move_left" id="select_move_left-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor left. Same as `copy_cursor_left`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_move_right </h2>

```rust,ignore
fn select_move_right(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="select_move_right" id="link-select_move_right-Description"  class="tablinks active"
    onclick="openTab(event, 'select_move_right', 'Description')">
Description
</button>
</div>

<div group="select_move_right" id="select_move_right-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor right. Same as `copy_cursor_right`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_move_up </h2>

```rust,ignore
fn select_move_up(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="select_move_up" id="link-select_move_up-Description"  class="tablinks active"
    onclick="openTab(event, 'select_move_up', 'Description')">
Description
</button>
</div>

<div group="select_move_up" id="select_move_up-Description" class="tabcontent"  style="display: block;" >
Move the copy-mode cursor up. Same as `copy_cursor_up`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_other_end </h2>

```rust,ignore
fn select_other_end(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="select_other_end" id="link-select_other_end-Description"  class="tablinks active"
    onclick="openTab(event, 'select_other_end', 'Description')">
Description
</button>
</div>

<div group="select_other_end" id="select_other_end-Description" class="tabcontent"  style="display: block;" >
Swap the selection's ends, so motions move the other end (`o`).
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_tab </h2>

```rust,ignore
fn select_tab(_: ActionApi, tabs_node_id: int, index: int) -> Action
```

<div>
<div class="tab">
<button group="select_tab" id="link-select_tab-Description"  class="tablinks active"
    onclick="openTab(event, 'select_tab', 'Description')">
Description
</button>
</div>

<div group="select_tab" id="select_tab-Description" class="tabcontent"  style="display: block;" >
Select a tab by index in a specific tabs node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> select_window </h2>

```rust,ignore
fn select_window(_: ActionApi, number: int) -> Action
```

<div>
<div class="tab">
<button group="select_window" id="link-select_window-Description"  class="tablinks active"
    onclick="openTab(event, 'select_window', 'Description')">
Description
</button>
<button group="select_window" id="link-select_window-Example"  class="tablinks"
    onclick="openTab(event, 'select_window', 'Example')">
Example
</button>
</div>

<div group="select_window" id="select_window-Description" class="tabcontent"  style="display: block;" >
Select a window (a root tab of the current session) by its number, which
counts from the base index set with `set_base_index` (0 when unset), like
tmux `select-window -t :N`.
</div>
<div group="select_window" id="select_window-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>1", action.select_window(1));
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> send_bytes </h2>

```rust,ignore
fn send_bytes(_: ActionApi, buffer_id: int, bytes: String) -> Action
fn send_bytes(_: ActionApi, buffer_id: int, bytes: Array) -> Action
```

<div>
<div class="tab">
<button group="send_bytes" id="link-send_bytes-Description"  class="tablinks active"
    onclick="openTab(event, 'send_bytes', 'Description')">
Description
</button>
</div>

<div group="send_bytes" id="send_bytes-Description" class="tabcontent"  style="display: block;" >
Send a string of bytes to a specific buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> send_bytes_current </h2>

```rust,ignore
fn send_bytes_current(_: ActionApi, bytes: String) -> Action
fn send_bytes_current(_: ActionApi, bytes: Array) -> Action
```

<div>
<div class="tab">
<button group="send_bytes_current" id="link-send_bytes_current-Description"  class="tablinks active"
    onclick="openTab(event, 'send_bytes_current', 'Description')">
Description
</button>
</div>

<div group="send_bytes_current" id="send_bytes_current-Description" class="tabcontent"  style="display: block;" >
Send a string of bytes to the focused buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> send_keys </h2>

```rust,ignore
fn send_keys(_: ActionApi, buffer_id: int, notation: String) -> Action
```

<div>
<div class="tab">
<button group="send_keys" id="link-send_keys-Description"  class="tablinks active"
    onclick="openTab(event, 'send_keys', 'Description')">
Description
</button>
</div>

<div group="send_keys" id="send_keys-Description" class="tabcontent"  style="display: block;" >
Send a key notation sequence to a specific buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> send_keys_current </h2>

```rust,ignore
fn send_keys_current(_: ActionApi, notation: String) -> Action
```

<div>
<div class="tab">
<button group="send_keys_current" id="link-send_keys_current-Description"  class="tablinks active"
    onclick="openTab(event, 'send_keys_current', 'Description')">
Description
</button>
</div>

<div group="send_keys_current" id="send_keys_current-Description" class="tabcontent"  style="display: block;" >
Send a key notation sequence to the focused buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_environment </h2>

```rust,ignore
fn set_environment(_: ActionApi, name: String, value: ?) -> Action
```

<div>
<div class="tab">
<button group="set_environment" id="link-set_environment-Description"  class="tablinks active"
    onclick="openTab(event, 'set_environment', 'Description')">
Description
</button>
<button group="set_environment" id="link-set_environment-Example"  class="tablinks"
    onclick="openTab(event, 'set_environment', 'Example')">
Example
</button>
</div>

<div group="set_environment" id="set_environment-Description" class="tabcontent"  style="display: block;" >
Set an environment variable for panes created later in the current
session (tmux `set-environment`). Pass `()` as the value to unset it.
Panes that already run keep their environment.
</div>
<div group="set_environment" id="set_environment-Example" class="tabcontent"  style="display: none;" >

```rhai
action.set_environment("AWS_PROFILE", "staging")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_global_environment </h2>

```rust,ignore
fn set_global_environment(_: ActionApi, name: String, value: ?) -> Action
```

<div>
<div class="tab">
<button group="set_global_environment" id="link-set_global_environment-Description"  class="tablinks active"
    onclick="openTab(event, 'set_global_environment', 'Description')">
Description
</button>
<button group="set_global_environment" id="link-set_global_environment-Example"  class="tablinks"
    onclick="openTab(event, 'set_global_environment', 'Example')">
Example
</button>
</div>

<div group="set_global_environment" id="set_global_environment-Description" class="tabcontent"  style="display: block;" >
Set an environment variable for panes created later in every session
(tmux `set-environment -g`). Pass `()` as the value to unset it. A
session's own value wins over the global one.
</div>
<div group="set_global_environment" id="set_global_environment-Example" class="tabcontent"  style="display: none;" >

```rhai
action.set_global_environment("EDITOR", "nvim")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_synchronize_panes </h2>

```rust,ignore
fn set_synchronize_panes(_: ActionApi, enabled: bool) -> Action
```

<div>
<div class="tab">
<button group="set_synchronize_panes" id="link-set_synchronize_panes-Description"  class="tablinks active"
    onclick="openTab(event, 'set_synchronize_panes', 'Description')">
Description
</button>
</div>

<div group="set_synchronize_panes" id="set_synchronize_panes-Description" class="tabcontent"  style="display: block;" >
Turn tmux-style `synchronize-panes` on or off for the active window.

The setting belongs to the window (a root tab) and is shared by every client. While it
is on, keys, pastes and `send_keys_current`/`send_bytes_current` input aimed at the
focused pane go to every pane the window shows, each encoded for that pane. Panes in
hidden nested tabs or hidden by zoom, panes in other windows, and popups are left out,
and input typed into a focused popup is never copied. Mouse and focus events, input
sent to a specific buffer id, and `embers send-keys` are not synchronized. Tab bars can
show the state through `TabInfo.synchronize_panes()`.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> split_with </h2>

```rust,ignore
fn split_with(_: ActionApi, direction: "h" | "horizontal" | "v" | "vertical", tree: TreeSpec) -> Action
```

<div>
<div class="tab">
<button group="split_with" id="link-split_with-Description"  class="tablinks active"
    onclick="openTab(event, 'split_with', 'Description')">
Description
</button>
</div>

<div group="split_with" id="split_with-Description" class="tabcontent"  style="display: block;" >
Split the current node and attach the provided tree as the new sibling.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> swap_current_node </h2>

```rust,ignore
fn swap_current_node(_: ActionApi, sibling_node_id: int) -> Action
```

<div>
<div class="tab">
<button group="swap_current_node" id="link-swap_current_node-Description"  class="tablinks active"
    onclick="openTab(event, 'swap_current_node', 'Description')">
Description
</button>
</div>

<div group="swap_current_node" id="swap_current_node-Description" class="tabcontent"  style="display: block;" >
Swap the current node with a sibling.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> switch_session </h2>

```rust,ignore
fn switch_session(_: ActionApi, name: String) -> Action
```

<div>
<div class="tab">
<button group="switch_session" id="link-switch_session-Description"  class="tablinks active"
    onclick="openTab(event, 'switch_session', 'Description')">
Description
</button>
<button group="switch_session" id="link-switch_session-Example"  class="tablinks"
    onclick="openTab(event, 'switch_session', 'Example')">
Example
</button>
</div>

<div group="switch_session" id="switch_session-Description" class="tabcontent"  style="display: block;" >
Switch the client to the session with the given name (tmux `switch-client -t`).
</div>
<div group="switch_session" id="switch_session-Example" class="tabcontent"  style="display: none;" >

```rhai
action.switch_session("work")
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> toggle_mode </h2>

```rust,ignore
fn toggle_mode(_: ActionApi, mode: String) -> Action
```

<div>
<div class="tab">
<button group="toggle_mode" id="link-toggle_mode-Description"  class="tablinks active"
    onclick="openTab(event, 'toggle_mode', 'Description')">
Description
</button>
</div>

<div group="toggle_mode" id="toggle_mode-Description" class="tabcontent"  style="display: block;" >
Toggle a named input mode.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> toggle_synchronize_panes </h2>

```rust,ignore
fn toggle_synchronize_panes(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="toggle_synchronize_panes" id="link-toggle_synchronize_panes-Description"  class="tablinks active"
    onclick="openTab(event, 'toggle_synchronize_panes', 'Description')">
Description
</button>
<button group="toggle_synchronize_panes" id="link-toggle_synchronize_panes-Example"  class="tablinks"
    onclick="openTab(event, 'toggle_synchronize_panes', 'Example')">
Example
</button>
</div>

<div group="toggle_synchronize_panes" id="toggle_synchronize_panes-Description" class="tabcontent"  style="display: block;" >
Toggle tmux-style `synchronize-panes` for the active window.

While it is on, input typed into a pane of the window is also sent to every other pane
the window currently shows. Popups and other windows never take part. See
`set_synchronize_panes` for details.
</div>
<div group="toggle_synchronize_panes" id="toggle_synchronize_panes-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>S", action.toggle_synchronize_panes());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> toggle_zoom_current_node </h2>

```rust,ignore
fn toggle_zoom_current_node(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="toggle_zoom_current_node" id="link-toggle_zoom_current_node-Description"  class="tablinks active"
    onclick="openTab(event, 'toggle_zoom_current_node', 'Description')">
Description
</button>
<button group="toggle_zoom_current_node" id="link-toggle_zoom_current_node-Example"  class="tablinks"
    onclick="openTab(event, 'toggle_zoom_current_node', 'Example')">
Example
</button>
</div>

<div group="toggle_zoom_current_node" id="toggle_zoom_current_node-Description" class="tabcontent"  style="display: block;" >
Toggle zoom for the focused node (tmux `resize-pane -Z`).
</div>
<div group="toggle_zoom_current_node" id="toggle_zoom_current_node-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>z", action.toggle_zoom_current_node());
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> toggle_zoom_node </h2>

```rust,ignore
fn toggle_zoom_node(_: ActionApi, node_id: int) -> Action
```

<div>
<div class="tab">
<button group="toggle_zoom_node" id="link-toggle_zoom_node-Description"  class="tablinks active"
    onclick="openTab(event, 'toggle_zoom_node', 'Description')">
Description
</button>
</div>

<div group="toggle_zoom_node" id="toggle_zoom_node-Description" class="tabcontent"  style="display: block;" >
Toggle zoom for the specified node id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> unzoom_current_session </h2>

```rust,ignore
fn unzoom_current_session(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="unzoom_current_session" id="link-unzoom_current_session-Description"  class="tablinks active"
    onclick="openTab(event, 'unzoom_current_session', 'Description')">
Description
</button>
</div>

<div group="unzoom_current_session" id="unzoom_current_session-Description" class="tabcontent"  style="display: block;" >
Clear the current session's active zoom state.
This removes the current session zoom rather than unwinding a stack of prior zooms.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> yank_selection </h2>

```rust,ignore
fn yank_selection(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="yank_selection" id="link-yank_selection-Description"  class="tablinks active"
    onclick="openTab(event, 'yank_selection', 'Description')">
Description
</button>
</div>

<div group="yank_selection" id="yank_selection-Description" class="tabcontent"  style="display: block;" >
Copy the current selection into the clipboard (OSC 52) and leave copy
mode. Without a selection it only leaves copy mode.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> zoom_current_node </h2>

```rust,ignore
fn zoom_current_node(_: ActionApi) -> Action
```

<div>
<div class="tab">
<button group="zoom_current_node" id="link-zoom_current_node-Description"  class="tablinks active"
    onclick="openTab(event, 'zoom_current_node', 'Description')">
Description
</button>
</div>

<div group="zoom_current_node" id="zoom_current_node-Description" class="tabcontent"  style="display: block;" >
Zoom the session's currently focused node.
There is intentionally no separate `zoom_node(node_id)` helper in this API surface.
</div>

</div>
</div>
</br>
