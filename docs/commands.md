# Commands

This page covers the tmux-style commands Embers offers beyond the basics in the
[README](../README.md): how windows are numbered, and the commands and client
actions that go with them.

## Window numbering

Windows are a session's root tabs. Each has a number: its position plus the
*base index*, like tmux's `base-index`. Numbers are what `list-windows` and
`new-window` print, what window targets such as `-t work:2` mean, and what tab
bar formatters get from `TabInfo.number()`.

Windows are always numbered without gaps. Closing one renumbers the windows
after it, which is how tmux behaves with `renumber-windows on`; Embers has no
setting for leaving gaps.

### Setting the base index

| Where | Setting | Applies to |
| --- | --- | --- |
| Rhai config | `set_base_index(1);` | the interactive client: tab bar numbers, `action.select_window`, commands run from the command prompt |
| Environment | `EMBERS_BASE_INDEX=1` | the `embers` CLI, and the client when its config does not call `set_base_index` |
| Command line | `embers --base-index 1 ...` | one CLI invocation; overrides `EMBERS_BASE_INDEX` |

The CLI does not read the Rhai config, so to number windows from 1 everywhere,
export `EMBERS_BASE_INDEX=1` where your shells and the server start. Panes
inherit the server's environment, so `embers` commands run inside panes then
agree with the client. The client also passes its base index to `run_shell`
children.

### Without a base index

When no base index is set, numbers start at 0, which is what earlier versions
printed. A numeric target is then accepted as either 0- or 1-based: in a
session with two windows, `-t work:0` is the first window and `-t work:2` the
second, but `-t work:1` is rejected as ambiguous. Setting any base index,
including `0`, makes every number mean exactly one window.

### Examples

```sh
export EMBERS_BASE_INDEX=1
embers new-window -t work --title logs   # prints "2	logs" in a session with one window
embers list-windows -t work              # number, active flag, title
embers select-window -t work:1
```

```rhai
set_base_index(1);
bind("normal", "<leader>1", action.select_window(1));
bind("normal", "<leader>2", action.select_window(2));

fn format_tabs(ctx) {
    let left = [];
    for tab in ctx.tabs() {
        left.push(ui.segment(` ${tab.number()}:${tab.title()} `));
    }
    ui.bar(left, [], [])
}
tabbar.set_formatter(format_tabs);
```

`TabInfo.index()` stays zero-based, because `action.select_tab` takes a
zero-based index.

### Panes

Panes are addressed by id (`list-panes` prints them, and `-t <id>` selects
one), not by a per-window index, so there is no `pane-base-index`.
