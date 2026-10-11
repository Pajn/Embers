# Commands

This page covers the tmux-style commands Embers offers beyond the basics in the
[README](../README.md): the session/window/pane picker, the command prompt,
`display-message`, session environments, how windows are numbered, and the
client actions that go with them.

## Picker (choose-tree)

The picker is tmux's `choose-tree`: a box over the client listing every
session, its windows and their panes as a collapsible tree.

| Action | Opens with | tmux |
| --- | --- | --- |
| `action.choose_tree()` | sessions, windows and panes all expanded | `choose-tree` |
| `action.choose_window()` | sessions expanded to their windows | `choose-tree -w` (`prefix w`) |
| `action.choose_session()` | sessions only | `choose-tree -s` (`prefix s`) |

The same three are available at the command prompt as `choose-tree` (with
`-s` or `-w`), `choose-window` and `choose-session`. The picker opens on the
current window or pane, marked with `*`. Sessions show their window count,
windows their number (from the base index) and pane count, and panes their id,
title and the program in their foreground.

| Key | Effect |
| --- | --- |
| `j`/`k`, Down/Up, C-n/C-p | move |
| PageDown/PageUp, C-d/C-u | move a page |
| `g`/`G`, Home/End | first/last row |
| `l`, Right, `+` | expand, or step into an expanded row |
| `h`, Left, `-` | collapse, or step out to the parent |
| Enter | switch to the session, window or pane |
| `x` | kill the selected item, after `y` to confirm |
| `/`, or any other text | filter |
| Esc | clear the filter, or close |
| `q`, C-c | close |

While filtering, typed text goes to the filter and only rows whose text
contains it (ignoring case) are shown, with the sessions and windows above
them; Up/Down still move, Enter or Tab ends filtering and Backspace on an empty
filter does too.

`x` runs `kill-session`, `kill-window` or `kill-pane` exactly as the CLI does,
so the picker kills the way those commands do. Choosing a pane in a hidden
nested tab reveals it. There is no preview.

The tree follows changes while the picker is open, and the selection stays on
the same session, window or pane, even when windows before it close. If the
selected item itself closes, Enter and `x` do nothing and the status line says
so, rather than acting on another row.

```rhai
bind("normal", "<leader>w", action.choose_window());
bind("normal", "<leader>s", action.choose_session());
```

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
| Rhai config | `set_base_index(1);` | the interactive client: tab bar numbers, `action.select_window`, commands run from the command prompt; and through the server, the `embers` CLI |
| Environment | `EMBERS_BASE_INDEX=1` | the client when its config does not call `set_base_index`, and the CLI, overriding what the server reports |
| Command line | `embers --base-index 1 ...` | one CLI invocation; overrides `EMBERS_BASE_INDEX` |

The server is the source of truth for the CLI. An attached client reports its
base index (from its config, else `EMBERS_BASE_INDEX`) to the server for the
session it shows, and the server keeps it with the session, saved with the
workspace. Sessions created afterwards start with the last base index
reported. The CLI numbers a session's windows from that base index unless
`--base-index` or `EMBERS_BASE_INDEX` gives one, so `set_base_index(1)` in the
config is enough for `embers select-window -t 1` in a pane, or in any shell, to
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
# With set_base_index(1) in the config of the client showing "work", or:
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

## Last pane

`select-pane -l` focuses the pane that was focused before the current one in
the current window, like tmux; running it again goes back. Each window has its
own last pane, so switching windows and back does not change it. When the last
pane has been closed, the one focused before it takes its place; with none left
the command fails (`no last pane`). The server keeps this history in memory, so
it starts empty after a server restart. A focused popup has no last pane.

From the Rhai config, `action.last_pane()` does the same and does nothing when
there is no last pane:

```rhai
bind("normal", "<leader>;", action.last_pane());
```

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

## Command prompt

The command prompt is tmux's `prefix :`. It opens in the status line, the last
row of the screen, and on Enter runs the typed line as an `embers` command.
The session's tab bar is at the top of the screen unless the config moves it
to the bottom with `tabbar.set_position("bottom")` (tmux `status-position
bottom`), where it sits just above the status line:

```text
:split-window --horizontal
:new-window --title logs -- tail -f app.log
:rename-window build
:resize-pane -L 5
:select-window -t 2
:kill-pane
:set-environment -g EDITOR nvim
:display-message "#S:#I #{pane_current_path}"
```

### How commands are run

The prompt accepts exactly the commands, options and arguments of the CLI, so
there is one command language to learn. The line is split into words like a
shell would (quotes group words; there are no variables or `;` separators), and
the words are parsed with the same definitions the `embers` binary uses for its
command line. The parsed command then runs the same code as `embers <command>`,
over a short connection of its own to the server the client is attached to.

Two things differ from running the CLI in a shell:

- Without `-t`, a command applies to the client's current session, and through
  it to that session's current window and focused pane, even when several
  sessions exist. The CLI would ask for `-t` then.
- Client commands default to this client: `display-message` shows on it, and
  `switch-client -t other` and `detach-client` need no client id.

Window numbers follow the client's base index (`set_base_index`). Output, such
as from `list-windows`, and errors are shown in the status line for a few
seconds; lines of output are joined with `; `. `attach` and `automation` cannot
be run from the prompt. Of the CLI's global options only `--base-index` applies:
the prompt always talks to the attached server, so a line with `--socket`,
`--config`, `--log` or `-v` is refused with an error rather than run with the
option ignored.

The alternative, mapping prompt commands onto client actions, would have meant
a second command language whose options drift from the CLI's. Reusing the CLI
definitions keeps the two in step, and kill and rename commands from the prompt
behave exactly as from the shell.

### Keys

| Key | Effect |
| --- | --- |
| Enter | run the line |
| Esc, C-c, C-g | cancel |
| Left/Right, C-b/C-f | move the cursor |
| Home/End, C-a/C-e | start/end of the line |
| Backspace, Delete (C-h, C-d) | delete a character |
| C-u, C-k | delete to the start/end of the line |
| C-w | delete the previous word |
| Up/Down, C-p/C-n | earlier/later lines from this client's history |

The history holds the last 100 lines run from the plain prompt, for as long as
the client runs. Lines from template prompts, like the rename prompts, are not
recorded.

#### vi keys

`set_status_keys("vi")` in the config is tmux's `status-keys vi`. The command
prompt and the rename and other template prompts then open in insert mode,
where the keys above work as usual, and Esc switches to normal mode:

| Key | Effect |
| --- | --- |
| `h`/`l` | move the cursor |
| `w`/`b`/`e` | next word, start of the word, end of the word |
| `0`/`$` | start/end of the line |
| `x` | delete the character under the cursor |
| `dw`, `D` | delete to the next word, to the end of the line |
| `cw`, `C` | change to the end of the word, to the end of the line |
| `i`/`a`/`I`/`A` | insert before/after the cursor, at the start/end of the line |
| `k`/`j` | earlier/later lines from the history |
| Esc, C-c, C-g | cancel |
| Enter | run the line |

As in vi, a word is a run of letters, digits and underscores, or a run of
other non-blank characters. Other keys do nothing in normal mode. The copy-mode
search prompt (`/`, `?`) keeps its own keys.

### Actions

| Action | tmux |
| --- | --- |
| `action.command_prompt()` | `command-prompt` |
| `action.command_prompt_with(initial)` | `command-prompt -I initial` |
| `action.command_prompt_template(prompt, initial, template)` | `command-prompt -p prompt -I initial template` |
| `action.rename_window_prompt()` | `prefix ,` |
| `action.rename_session_prompt()` | `prefix $` |
| `action.run_command(line)` | binding a command, such as `bind x kill-pane` |
| `action.confirm(prompt, action)` | `confirm-before -p prompt command` |

The prompt label and initial text may use formats, so `#W` starts the window
rename prompt from the current name. In a template, `%%` is replaced by the
typed text as a single word, quoted as needed, so templates need no quotes of
their own: `rename-window -- %%`.

```rhai
bind("normal", "<leader>:", action.command_prompt());
bind("normal", "<leader>,", action.rename_window_prompt());
bind("normal", "<leader>$", action.rename_session_prompt());
bind("normal", "<leader>x", action.run_command("kill-pane"));
bind("normal", "<leader>N", action.command_prompt_template("(new session) ", "", "new-session %%"));
```

### Confirmation

`action.confirm(prompt, action)` is tmux's `confirm-before`: it shows `prompt`
in the status line, expanded as a format, and runs `action` (an action or an
array of actions) only if the next key is `y`. Any other key cancels. tmux's
default `x` and `&` bindings ask this way:

```rhai
bind("normal", "<leader>x", action.confirm("kill-pane #D? (y/n)", action.run_command("kill-pane")));
bind("normal", "<leader>&", action.confirm("kill-window #W? (y/n)", action.run_command("kill-window")));
```
