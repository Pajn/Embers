# Commands

This page covers the tmux-style commands Embers offers beyond the basics in the
[README](../README.md): how windows are numbered, session environments,
`display-message`, and the commands and client actions that go with them.

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

## Environment

Like tmux, Embers keeps a global environment and one per session. A pane
created for a session starts with the server's environment, then the global
variables, then the session's, then any variables its creator passed
(`tree.buffer_spawn` env, for example); later layers win. Changes only affect
panes created afterwards. The environments are kept in memory, not saved with
the workspace, so set them from your config or a startup script.

```sh
embers set-environment -t work AWS_PROFILE staging   # one session
embers set-environment -g EDITOR nvim                # every session
embers set-environment -u -t work AWS_PROFILE        # unset
embers show-environment -t work                      # NAME=VALUE lines
embers show-environment -g EDITOR                    # one variable
```

`-t` names a session and, as with other commands, may be left out when only
one session exists. `-g` and `-t` cannot be combined. `setenv` and `showenv`
are accepted as short names. Unsetting a session variable removes it from the
session; the global value, if any, applies again.

From the Rhai config, `action.set_environment(name, value)` sets a variable in
the current session and `action.set_global_environment(name, value)` in the
global environment; a value of `()` unsets it.

```rhai
bind("normal", "<leader>P", action.set_environment("AWS_PROFILE", "prod"));
```

## display-message

`display-message` shows a message in the status line of the clients showing a
session, for three seconds unless `-d` gives a time in milliseconds. With `-p`
it prints the message instead.

```sh
embers display-message "build finished"           # every client on the session
embers display-message -d 10000 -t 12 "#S: done"   # format for pane 12, 10 s
embers display-message -c 3 "hello"                # client 3 only (see list-clients)
embers display-message -p "#{pane_current_path}"   # print, show nothing
```

The message is expanded for the target pane (`-t`, a pane id), or the focused
pane of the session or client. Words after the options are joined with
spaces. Without a message it shows `[#S] #I:#W, current pane #D -
(#{pane_current_command})`. If no client shows the session, the command fails,
as it does in tmux.

### Formats

A small part of tmux's format language is supported:

| Short | Long | Value |
| --- | --- | --- |
| `#S` | `#{session_name}` | session name |
| `#I` | `#{window_index}` | window number, counting from the base index |
| `#W` | `#{window_name}` | window title |
| `#D` | `#{pane_id}` | pane id, as `list-panes` prints it and `-t` takes it |
| `#T` | `#{pane_title}` | pane title |
| | `#{pane_current_path}` | the pane's working directory |
| | `#{pane_current_command}` | the program in the pane's foreground, or the one it started with when that is unknown |
| `##` | | a literal `#` |

An unknown `#{name}` expands to nothing. Conditionals, modifiers and styles
are not supported. A pane in a popup reports the session's active window.

From the Rhai config, `action.display_message(text)` and
`action.display_message(text, milliseconds)` expand the same formats for the
focused pane and show the result in this client's status line.

```rhai
bind("normal", "<leader>i", action.display_message("#S:#I #{pane_current_path}"));
```
