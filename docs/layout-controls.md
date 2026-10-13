# Layout controls

Resizing panes, zooming, dragging split dividers with the mouse, and tmux-style
`synchronize-panes`. The config functions mentioned here are documented in
[`config-api`](config-api/index.md).

## Resizing panes

`action.resize_left(n)`, `resize_right(n)`, `resize_up(n)` and `resize_down(n)`
follow tmux's `resize-pane -L/-R/-U/-D n`:

- The nearest ancestor split along the direction's axis is resized: a
  side-by-side split for left and right, a stacked split for up and down.
  Splits along the other axis, and tab containers, are skipped on the way up.
- The divider after the focused pane moves, or the divider before it when the
  pane is the split's last child. Left and up move the divider toward the
  start of the split, right and down toward the end. In a two-pane split,
  `resize_left` shrinks the left pane and grows the right one, whichever of
  them is focused.
- The space comes from the panes on the shrinking side of the divider,
  nearest first. A pane keeps at least one column, or one row of content plus
  its title row if it has one (`set_pane_titles`), and a nested split keeps
  room for all of its panes.
- When there is no such split, or the divider is already as far as it can go,
  the action does nothing.
- A popup's panes only resize splits inside the popup; the popup itself keeps
  its geometry.
- While a pane is zoomed, only splits inside the zoomed view can be resized.

Split sizes are stored as relative weights. A resize sets them to the cell
counts the panes end up with, so the layout is exact at the current size and
stays proportional when the terminal is resized.

`action.toggle_zoom_current_node()` zooms or unzooms the focused pane (tmux
`resize-pane -Z`).

From the command line:

```sh
embers resize-pane -L 3            # focused pane
embers resize-pane -t 12 -D        # pane 12, by one row
embers resize-pane -t 12 -Z        # toggle zoom
embers resize-pane -t 12 --sizes 3,1
```

`--sizes` sets the weights of the pane's parent split directly. The CLI has no
terminal of its own, so `-L/-R/-U/-D` measure the layout at the size the
session was last drawn at, which the attached client records in the size of
each pane's PTY.

## Dragging dividers

With mouse capture on, pressing the left button on a split divider and
dragging moves the divider to the pointer, within the same limits as the
resize actions. Once a drag has started on a divider it keeps resizing that
split wherever the pointer goes, until the button is released. Presses and
drags that start inside a pane are handled as before, including being
forwarded to programs that asked for mouse reporting. Popups hide the
dividers underneath them.

The built-in config turns this on; `mouse.set_drag_resize(false)` turns it off.

## Divider colors

Dividers between panes are drawn as continuous lines, joined where one meets
another, dim by default. Like tmux's `pane-border-style` and
`pane-active-border-style`, the theme can color them, with the cells along the
focused pane's edges in a color of their own:

```rhai
theme.set_pane_border_color("brightblack");
theme.set_pane_active_border_color("magenta");
```

Colors are `"#RRGGBB"` or one of the sixteen ANSI names, `"black"` to
`"white"` and `"brightblack"` to `"brightwhite"`, which follow the host
terminal's color scheme. Without an active color the cells next to the focused
pane take the other color. Popup frames are not affected.

When a divider is the only one, between just two panes, it borders the focused
pane all along, so like tmux only the half on that pane's side takes the active
color: the top or left half for the first pane, the bottom or right half for
the second.

## Popup frames

Popups (floating windows) are framed with rounded box-drawing lines, `╭─╮`,
with the popup's title on the top edge. The focused popup's frame is bold, the
others dim. Like tmux's `popup-border-lines` and `popup-border-style`, the
theme can pick other lines and colors:

```rhai
theme.set_popup_border_lines("single");
theme.set_popup_border_color("brightblack");
theme.set_popup_active_border_color("magenta");
```

The lines are `"rounded"`, `"single"` (`┌─┐`), `"double"` (`╔═╗`), `"heavy"`
(`┏━┓`), `"ascii"` (`+-+`) or `"none"`. With `"none"` the frame cells stay
blank rather than giving the popup's panes the space: the title still has its
row, and a popup's panes are the same size in every client, whatever each
client's config says. Colors take the same values as the divider colors and
also color the title.

A popup can also be opened without a frame, like tmux's `display-popup -B`:
`embers display-popup -B -- cmd`, or `border: false` in the options of
`action.open_floating`. It has no frame and no title, and its panes fill the
whole popup, less a title row each if `set_pane_titles` gives panes one (see
[Pane titles](configuration.md#pane-titles)).

## Synchronize panes

While `synchronize-panes` is on for a window (a root tab), input typed into
the focused pane is also sent to every other pane the window shows.

- It applies to typed keys, pastes, and `send_keys_current` and
  `send_bytes_current` actions. Each pane gets the input encoded for its own
  program, so key encodings and bracketed paste follow what each program
  asked for.
- Only panes the window currently shows take part. Panes in inactive nested
  tabs or hidden by zoom do not.
- Popups are excluded: input typed into a focused popup goes only to the
  popup, and popup panes never receive copies.
- Panes in other windows and other sessions are never affected.
- Mouse and focus events, actions that name a specific buffer id, and
  `embers send-keys` are not synchronized.
- If a copy cannot be delivered, for example because that pane's program has
  exited, the other panes still receive their input.

The setting is shared by every client attached to the session and lasts as
long as the server runs; sessions restored from disk start with it off. When
a session's last two windows are reduced to one, the remaining window is no
longer tabbed and its setting is cleared.

Turn it on and off with `action.toggle_synchronize_panes()` or
`action.set_synchronize_panes(enabled)`, or from the command line:

```sh
embers synchronize-panes            # toggle the active window, print on/off
embers synchronize-panes -t work:logs on
embers synchronize-panes off
```

The default tab bar appends `(sync)` to a synchronized window's title. Custom
tab bar formatters can check `tab.synchronize_panes()`:

```rhai
fn format_tabs(ctx) {
    let left = [];
    for tab in ctx.tabs() {
        left.push(ui.segment(" " + tab.title() + " "));
        if tab.synchronize_panes() {
            left.push(ui.segment("sync ", #{ fg: "#e78284" }));
        }
    }
    ui.bar(left, [], [])
}
tabbar.set_formatter(format_tabs);
```

## smart-splits.nvim

[smart-splits.nvim](https://github.com/mrjones2014/smart-splits.nvim) expects
Alt-h/j/k/l to resize the multiplexer pane by 3 cells unless the pane runs
Neovim, which then gets the key and resizes its own windows. Ctrl-h/j/k/l move
between panes the same way.

### Neovim detection

`buffer.process_name()` is the name of the program in the pane's foreground,
like tmux's `pane_current_command`: the job the pane's shell runs, so `nvim`
once Neovim is started from a shell, and the pane's own command (`zsh`) while
the shell is at its prompt. `buffer.foreground_command()` gives the whole
command line, and `buffer.command()` the command the pane was started with.
The runtime keeper reads the foreground job from the PTY's foreground process
group, and the client sees a change within about 50 ms. `embers buffer show
<id>` prints the same as its `current_command` row, and the job's command line
as `foreground_command` while a job runs.

Detection cannot see through every layer: Neovim run over `ssh`, or inside a
nested multiplexer, shows up as `ssh` or `tmux`. For those, Neovim can mark its
own pane with the `is-vim` pane option, the counterpart of tmux's
`@pane-is-vim`, which the config reads with `buffer.user_option("is-vim")`.
The detection used by the repository config treats `"1"` as Neovim and `"0"`
as not, and otherwise goes by the foreground program's name:

```rhai
fn is_nvim_buffer(ctx) {
    let buffer = ctx.current_buffer();
    let node = ctx.current_node();
    if buffer == () || node == () || node.kind() != "buffer_view" {
        return false;
    }
    let is_vim = buffer.user_option("is-vim");
    if is_vim == "1" {
        return true;
    }
    if is_vim == "0" {
        return false;
    }
    let name = buffer.process_name();
    name == "nvim" || name == "vim"
}

fn smart_resize_left(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_keys_current("<M-h>")
    } else {
        action.resize_left(3)
    }
}

fn smart_nav_left(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_keys_current("<C-h>")
    } else {
        action.focus_left()
    }
}

define_action("smart-resize-left", smart_resize_left);
define_action("smart-nav-left", smart_nav_left);
bind("normal", "<M-h>", "smart-resize-left");
bind("normal", "<C-h>", "smart-nav-left");
// ...and likewise for j/k/l with resize_down/up/right and focus_down/up/right.
```

smart-splits.nvim only sets `@pane-is-vim` under tmux, so a Neovim that
detection misses needs its own autocommands to set the Embers option. Panes
inherit `EMBERS_SOCKET`, so the `embers` command reaches the right server, and
`EMBERS_BUFFER`, the id of the pane's buffer, so without `-t` the command sets
the option on Neovim's own pane, whichever pane is focused and however many
sessions exist (see [Set in panes](configuration.md#set-in-panes)). Leaving or
suspending Neovim unsets the option rather than setting `"0"`, so detection
takes over again for whatever runs in the pane next:

```lua
local function set_is_vim(args)
  if vim.env.EMBERS_BUFFER then
    vim.system(vim.list_extend({ "embers", "buffer", "set-option" }, args))
  end
end
vim.api.nvim_create_autocmd({ "VimEnter", "VimResume" }, {
  callback = function() set_is_vim({ "is-vim", "1" }) end,
})
vim.api.nvim_create_autocmd({ "VimLeavePre", "VimSuspend" }, {
  callback = function() set_is_vim({ "--unset", "is-vim" }) end,
})
```

Where the pane's environment does not reach Neovim, as on the far side of
`ssh`, set the option from the local side instead: a wrapper that starts the
remote editor can run `embers buffer set-option is-vim 1` before it and
`embers buffer set-option --unset is-vim` after it.

The full example lives in the repository's [`config.rhai`](../config.rhai).
