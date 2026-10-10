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
  nearest first. A pane keeps at least one column, or its title row plus one
  row of content, and a nested split keeps room for all of its panes.
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

Embers has no foreground process detection: `buffer.process_name()` is the
name of the command the pane was started with. Neovim instead marks its own
pane with the `is-vim` pane option, the counterpart of tmux's `@pane-is-vim`,
and the config reads it with `buffer.user_option("is-vim")`. The detection
used by the repository config treats `"1"` as Neovim and `"0"` as not, and
otherwise falls back to the command name, which catches panes started
directly into Neovim:

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
        action.send_bytes_current([8])
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

smart-splits.nvim only sets `@pane-is-vim` under tmux, so Neovim needs its own
autocommands to set the Embers option. Panes inherit `EMBERS_SOCKET`, so the
`embers` command reaches the right server, and `EMBERS_BUFFER`, the id of the
pane's buffer, so without `-t` the command sets the option on Neovim's own pane,
whichever pane is focused and however many sessions exist (see
[Set in panes](configuration.md#set-in-panes)):

```lua
local function set_is_vim(value)
  if vim.env.EMBERS_BUFFER then
    vim.system({ "embers", "buffer", "set-option", "is-vim", value })
  end
end
vim.api.nvim_create_autocmd({ "VimEnter", "VimResume" }, {
  callback = function() set_is_vim("1") end,
})
vim.api.nvim_create_autocmd({ "VimLeavePre", "VimSuspend" }, {
  callback = function() set_is_vim("0") end,
})
```

The full example lives in
[`crates/embers-client/tests/fixtures/repository_config.rhai`](../crates/embers-client/tests/fixtures/repository_config.rhai).
