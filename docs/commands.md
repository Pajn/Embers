# Commands

This page covers the tmux-style commands Embers offers beyond the basics in the
[README](../README.md): the session/window/pane picker, the command prompt,
`display-message`, session environments, how windows are numbered, the client
actions that go with them, and [how closely the CLI follows
tmux](#tmux-compatibility).

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

Panes have ids (`list-panes` prints them, and `-t <id>` selects one). Like
tmux with its default `pane-base-index` of 0, `-t work:2.1` is the second pane
of window 2, counting in `list-panes` order from 0; there is no setting to
count from another number. See [Targets](#targets).

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

The message is expanded for the target pane (`-t`, any [target](#targets)), or the focused
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
| `#D` | `#{pane_id}` | pane id, as `%N` like tmux (the pane showing buffer `N`); `-t` takes it |
| | `#{node_id}` | the pane's layout node, as `list-panes` prints it; `-t` takes it as a plain number |
| `#T` | `#{pane_title}` | pane title |
| | `#{pane_current_path}` | the pane's working directory |
| | `#{pane_current_command}` | the program in the pane's foreground, or the one it started with when that is unknown |
| | `#{session_id}` | session id, as `$N` (a target) |
| | `#{session_windows}` | number of windows in the session |
| | `#{session_attached}` | number of clients showing the session (CLI only) |
| | `#{client_name}`, `#{client_id}`, `#{client_session}` | the client's id and the name of the session it shows (`list-clients -F` only) |
| | `#{window_id}` | window id, as `@N` (a target): the node the window shows |
| | `#{window_active}` | 1 for the session's active window, else 0 |
| | `#{window_panes}` | number of panes shown in the window |
| | `#{buffer_id}` | the pane's buffer, the value of `EMBERS_BUFFER`; `-t %N` takes it |
| | `#{pane_index}` | the pane's position in its window, from 0, as `-t :window.N` takes it |
| | `#{pane_active}` | 1 for the session's focused pane, else 0 |
| | `#{pane_pid}` | process id of the pane's program |
| | `#{pane_width}`, `#{pane_height}` | the pane's size in cells |
| | `#{pane_dead}`, `#{pane_dead_status}` | 1 when the pane's program has exited (kept by `remain-on-exit`), and its exit status |
| | `#{pane_at_left}`, `#{pane_at_right}`, `#{pane_at_top}`, `#{pane_at_bottom}` | 1 when no pane is drawn on that side, as `select-pane -L/-R/-U/-D` sees it (CLI only) |
| | `#{@name}` | the pane's option `@name` (see [Options](#options)), or nothing when it is not set |
| `##` | | a literal `#` |

An unknown `#{name}` expands to nothing. Conditionals, modifiers and styles
are not supported. A pane in a popup reports the session's active window.
Variables marked CLI only need the session's layout or client list, which the
`embers` command looks up; the client's own formats leave them empty.

From the Rhai config, `action.display_message(text)` and
`action.display_message(text, milliseconds)` expand the same formats for the
focused pane and show the result in this client's status line.

```rhai
bind("normal", "<leader>i", action.display_message("#S:#I #{pane_current_path}"));
```

## Command prompt

The command prompt is tmux's `prefix :`. It opens in the status line and on
Enter runs the typed line as an `embers` command. Like tmux's, the status line
is drawn over the session's tab bar while it has something to show, so the
prompt opens at the top of the screen, or at the bottom when the config moves
the bar there with `tabbar.set_position("bottom")` (tmux `status-position
bottom`; see [Screen layout](configuration.md#screen-layout)). Some lines it
runs:

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

## proxy

`embers proxy` connects its standard input and output to the server, starting
the server if it is not running, so a program that speaks the Embers protocol
can reach the server through a pipe instead of the socket. Over SSH, this
reaches the server on another machine:

```sh
ssh devbox embers proxy
```

The proxy passes bytes through without reading them, so the client at the
other end does the version check as it would on the socket: a client and a
server built from releases that speak different protocol versions refuse each
other, with an error naming both releases. The proxy exits once the server
closes the connection, which it does when the client's side of the pipe ends.

`--socket` and `EMBERS_SOCKET` choose the server as for any other command. SSH
runs the command without a login shell, so `embers` must be on the `PATH` that
non-interactive SSH sessions get on the remote machine, or the command must
name its path.

## tmux compatibility

The commands a script or tool typically sends to tmux take tmux's flags and
targets, so the same code can drive either. Inside a pane, `EMBERS_SOCKET` and
`EMBERS_BUFFER` play the part of `TMUX` and `TMUX_PANE`; `-t
%$EMBERS_BUFFER` is the counterpart of `-t $TMUX_PANE`.

```sh
embers new-session -d -s review -c ~/src/app
embers switch-client -t =review
embers split-window -h -c '#{pane_current_path}' -P -F '#{pane_id}'
embers display-popup -E -w 90% -h 90% -d ~/src/app -- "$EDITOR" draft.md
embers select-pane -L
embers capture-pane -p -S -100 -J
```

Commands still take a command to run only after `--`, as they always have:
`new-window -- vim`, not tmux's `new-window vim`.

### Targets

Every `-t` takes tmux's forms:

| Target | Means |
| --- | --- |
| `work` | session `work`, or the one session whose name starts with `work` |
| `=work` | session `work` exactly |
| `$3` | session id 3 (`#{session_id}`) |
| `work:2`, `work:logs`, `work:=logs` | window 2 (counting from the base index), the window named or starting with `logs`, or named exactly `logs` |
| `work:^`, `work:$`, `work:+`, `work:-2` | the first, last, next or second previous window |
| `@7` | the window showing node 7 (`#{window_id}`) |
| `work:2.1`, `:2.1`, `.1` | pane 1 (from 0) of that window, of window 2 of the current session, of the current window |
| `:`, `work:` | the current window and pane, or those of `work` |
| `%12` | the pane showing buffer 12 (`#{buffer_id}`, `EMBERS_BUFFER`) |
| `12` | as a pane target, pane id 12 (`#D`, `list-panes`); as a window target, window 12 |

A word without `:` names a window of the current session and, failing that, a
session, as in tmux. A session target may name a window or pane, meaning its
session. "Current" means the pane the command runs in, else the command
prompt's session, else the only session. tmux's glob patterns, `{last}`/`!`
and the pane tokens such as `{left-of}` are not supported; `kill-popup -t`
still takes popup ids. Unlike earlier versions, `work:12` given to a pane
command is window 12, not pane 12.

### Commands

| Command | Supported | Differences |
| --- | --- | --- |
| `new-session` | `-d`, `-A`, `-s`, `-n`, `-c`, `-x`, `-y`, `-P`, `-F`, `-- command` | See below. `new-session NAME` (no flags) is the older Embers form: an empty session, printing its id and name. |
| `new-window` | `-t`, `-n`, `-c`, `-d`, `-P`, `-F`, `-- command` | `-t` names the session; the window is always added last. Without `-P` it prints the window's number and title. |
| `split-window` | `-t`, `-h`, `-v`, `-c`, `-d`, `-P`, `-F`, `-- command` | Without `-h` or `-v` the new pane goes to the right (tmux: below). Without `-P` it prints the new pane's id. No `-l`, `-p`, `-b` or `-f`. |
| `display-popup` | `-t`, `-w`, `-h`, `-x`, `-y`, `-E`, `-EE`, `-d`, `-T`, `-P`, `-F`, `-- command` | See below. |
| `switch-client` | `-c`, `-t`, `-n`, `-p` | No `-l`: the server does not record each client's previous session. See below. |
| `detach-client` | `-c` | Picks the client like `switch-client`. |
| `select-pane` | `-t`, `-L`, `-R`, `-U`, `-D`, `-l` | Directional moves do not wrap around at the edge. No `-T`: pane titles come from the program. See below. |
| `set-option`, `show-options` | `-p` only; `-t`, `-u`, `-o`, `-q`; `show-options -v` | See [Options](#options). |
| `capture-pane` | `-t`, `-p`, `-S`, `-E`, `-J`, `-e` | See below. |
| `list-sessions`, `list-windows`, `list-panes` | `-F`; `list-windows -a`, `list-panes -s`/`-a` | Without `-F`, Embers' tab-separated columns. |
| `list-clients` | `-F`, `-t session` | The format expands for each client's session and focused pane, plus `#{client_name}`/`#{client_id}` (the client's id; clients have no tty name) and `#{client_session}`. Without `-F`, Embers' tab-separated columns. |
| `has-session`, `kill-session`, `kill-window`, `kill-pane`, `rename-session`, `rename-window`, `select-window`, `send-keys`, `resize-pane`, `display-message` | `-t` with any target | `send-keys` sends its words as text, joined by spaces (`--enter` presses Enter); tmux key names such as `C-c` are not translated. |

`new`, `ls`, `neww`, `lsw`, `splitw`, `lsp`, `selectp`, `capturep`, `popup`,
`switchc`, `detach`, `set` and `show` are accepted as short names. `-h` is not help for
`split-window` and `display-popup`; use `--help`. The global `-v` (verbose)
goes before the command, which leaves `-v` to `split-window`.

#### new-session

`new-session` creates the session with a first window running the command, or
`$SHELL`, in `-c DIR`. Like tmux it attaches unless `-d` is given, but only
from a terminal: run from a script, it creates the session and leaves it
detached. From inside a pane it refuses to attach (tmux: "sessions should be
nested with care"); use `-d` and `switch-client`. From the command prompt it
switches the client to the new session instead. `-A` attaches to an existing
session of that name instead of failing with `duplicate session`. Without
`-s`, sessions are named `0`, `1`, ... as in tmux. `-x` and `-y` set the size
the session is laid out at until a client shows it.

#### Working directories

`-c` (and `display-popup -d`) may use formats, expanded for the pane being
split, the pane the command runs in, or the session's focused pane, so
`split-window -c '#{pane_current_path}'` works as in tmux. Relative
directories are taken from where the command runs. Without `-c`, a pane
created by a command run from a shell starts in that shell's directory, which
is what tmux does for commands run from a shell; from the command prompt it
starts where the server runs, as panes created by key bindings do. Embers has
no per-session start directory, which is what tmux uses for key bindings.

#### display-popup

Run from a shell, `display-popup` waits until the popup closes and exits with
its command's status, like tmux, so a tool can open an editor and read the
file back afterwards. `--no-wait` returns at once and prints the popup id,
which is how the command behaved before; from the command prompt and from
`embers automation` it never waits.

- The popup closes when its command exits, with or without `-E`; `-EE` keeps
  it open when the command fails, until it is closed. tmux keeps a popup
  without `-E` open.
- Closing the popup (`kill-popup`, a binding) ends the wait and the command;
  the status is then 0. A popup belongs to its session, not to a client, so a
  client detaching does not close it and the wait goes on. If the server goes
  away, the command fails. Interrupting the waiting command (`C-c`) leaves the
  popup to close by itself when its command exits.
- `-w` and `-h` are cells or a percentage of the session's view; the default is
  half the view each way, centred, as in tmux. `-x` and `-y` place the top-left
  corner in cells or percent, or take `C` (centre) and, for `-x`, `R` (right
  edge). The view is the size the client showing the session last reported;
  before any client has shown it, the size its panes add up to, else 80x24.
- The working directory is `-d DIR`, tmux's letter. `-c DIR` is accepted too,
  although in tmux `-c` names the client. Popups belong to sessions, so `-t`
  picks the session through any target and there is no client option.
- `-T` is a format. `-P` prints the popup with `-F` (default `#{popup_id}`) as
  soon as it is shown.
- To wait for the status, the popup's buffer is created with the
  `remain-on-exit` option on, and the command closes the popup once it has the
  status.

#### capture-pane

With any of `-p`, `-S`, `-E`, `-J` or `-e`, `capture-pane` prints like tmux
`capture-pane -p`: the screen, or lines `-S` to `-E`, where 0 is the screen's
first row, negative numbers go into the history and `-` is the start of the
history or the end of the screen. `-e` keeps colours and attributes as SGR
escape sequences. Captured lines carry no wrap flag, so `-J` joins a line to
the next when it fills the pane's width; a line that exactly fills the width
without wrapping is joined too. Bare `capture-pane` prints the whole history
and screen, as it always has (tmux would store it in a paste buffer).

#### switch-client, select-pane and select-window

As in tmux, `switch-client -t` with a window or pane shows that window with
that pane focused: `switch-client -t %3` moves the client to the session of
the pane showing buffer 3, makes its window current and focuses it. A target
with `:`, `.` or `%` is read as a pane, so `work:2` shows window 2 with the
pane it last focused; a plain word is a session, shown as it was. The server
changes the window and focus before it switches the client, so the client
never shows the session's old window.

`select-pane -t` and `select-window -t` never move a client. `select-pane -t`
with a pane in a window that is not its session's current one makes it the
pane that window focuses, but leaves the current window as it is, as tmux
does; selecting the window then focuses the pane. `#{pane_active}` is 1 only
for the session's focused pane, so it reads 0 for such a pane until its window
is selected (tmux reports each window's active pane). `select-window -t
work:2` makes window 2 current in `work`, even when the client shows another
session.

#### Options

Embers keeps options on panes only, as the options of the buffer a pane
shows. `set-option -p` and `show-options -p` work on those, so tmux scripts
that keep their state in pane user options run unchanged:

```sh
embers set-option -p -t %3 @tria-open-socket /tmp/tria.sock
embers list-panes -a -F '#{pane_id} #{@tria-open-socket}'
embers show-options -p -t %3              # @tria-open-socket /tmp/tria.sock
embers show-options -pv -t %3 @tria-open-socket
embers set-option -pu -t %3 @tria-open-socket
```

A user option `@name` is the buffer option `name`: the one `embers buffer
set-option name` sets, the config reads with `buffer.user_option("name")`, and
`#{@name}` expands to. So `set-option -p @is-vim 1` and `buffer set-option
is-vim 1` set the same option. The only other option `set-option -p` takes is
`remain-on-exit`; any other name is an invalid option, as an unknown one is in
tmux. Options are saved with the workspace.

Windows, sessions and the server have no options: `set-option` and
`show-options` without `-p`, or with `-w`, `-s` or `-g`, fail. `#{@name}` reads
the pane's option in every format; `list-windows -F` and `list-sessions -F`
read it from the pane they expand for (the focused pane, else the window's
first), where tmux would look up a window or session option.
`set-option -a` (append) and `-F` are not supported.

#### Choosing the client

`switch-client` and `detach-client` without `-c` act on the command prompt's
client; else on `EMBERS_CLIENT`, which `run_shell` children get; else on the
client showing the session of the pane the command runs in, the one that
attached last when several do (tmux picks the most recently active, which
Embers does not track); else on the only attached client. Only when none of
these decides, because several clients are attached, does the command ask for
`-c`.
