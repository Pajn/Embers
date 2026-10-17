# Copy mode

Copy mode lets you move a cursor through a pane's scrollback, select text, and copy it to the
clipboard. The built-in bindings follow tmux's `copy-mode-vi` table, which in turn follows vim.
Copies go to the host terminal's clipboard as OSC 52, so the terminal must allow OSC 52 writes.

## Entering and leaving

- Bind `action.enter_copy_mode()` to a key. The built-in config does not bind one, because every
  normal-mode key belongs to the program in the pane. A tmux-style setup:

  ```rhai
  set_leader("<C-b>");
  bind("normal", "<leader>[", action.enter_copy_mode());
  ```

- Drag with the mouse (see [Mouse](#mouse)).
- Scroll the focused pane back: `PageUp`, the mouse wheel, or any `scroll_line_up()`,
  `scroll_page_up()` or `scroll_to_top()` binding in `normal` mode. Scrolling down to the bottom
  again leaves copy mode, unless a selection is active. This is tmux's `copy-mode -e`, which its
  wheel and `PageUp` bindings use.

Entering copy mode freezes the pane's view, so new output does not move it, and puts the cursor
where the program's cursor is. When copy mode starts by scrolling, the scroll then moves the cursor
to stay on screen, so it lands on the bottom row. `q`, `<C-c>` or `<Esc>` (with no selection) leaves
copy mode. This drops the cursor, the selection and any search highlights, and the view follows
output again.

Copy mode does not start on the alternate screen (full-screen programs such as vim or less), which
has no scrollback.

## Default keys

These keys work in `copy` mode and in `select` mode. `select` is copy mode with a selection active.

| Keys | Action | Rhai action |
| --- | --- | --- |
| `h` `j` `k` `l`, arrows | move the cursor | `copy_cursor_left()` … `copy_cursor_down()` |
| `w` `b` `e` | next word start, previous word start, next word end | `copy_next_word()`, `copy_previous_word()`, `copy_next_word_end()` |
| `W` `B` `E` | the same for whitespace-separated words | `copy_next_space()`, `copy_previous_space()`, `copy_next_space_end()` |
| `0` / `<Home>`, `^`, `$` / `<End>` | line start, first non-blank, last character | `copy_start_of_line()`, `copy_back_to_indentation()`, `copy_end_of_line()` |
| `gg`, `G` | first line of history, last line | `copy_history_top()`, `copy_history_bottom()` |
| `H` `M` `L` | top, middle and bottom row of the view | `copy_top_line()`, `copy_middle_line()`, `copy_bottom_line()` |
| `{` `}` | previous or next paragraph | `copy_previous_paragraph()`, `copy_next_paragraph()` |
| `<C-u>` `<C-d>` | half a page up or down | `copy_halfpage_up()`, `copy_halfpage_down()` |
| `<C-b>` `<C-f>`, `<PageUp>` `<PageDown>` | a page up or down | `copy_page_up()`, `copy_page_down()` |
| `<C-y>` `<C-e>` | scroll a line, keeping the cursor on screen | `scroll_line_up()`, `scroll_line_down()` |
| `f` `F` `t` `T` + character | jump onto, or just before, a character on the line | `copy_jump_forward()`, `copy_jump_backward()`, `copy_jump_to_forward()`, `copy_jump_to_backward()` |
| `;` `,` | repeat the last jump, or repeat it reversed | `copy_jump_again()`, `copy_jump_reverse()` |
| `/` `?` | search forward or backward | `enter_search_mode()`, `enter_search_backward()` |
| `n` `N` | next or previous match | `search_next()`, `search_prev()` |
| `v` `V` `<C-v>` | character, line or block selection | `enter_select_char()`, `enter_select_line()`, `enter_select_block()` |
| `<Enter>` | copy the selection and leave copy mode | `yank_selection()` |
| `q` `<C-c>` | leave copy mode | `exit_copy_mode()` |

Only in `copy` mode: `<Space>` starts a character selection and `<Esc>` leaves copy mode.

Only in `select` mode: `y` copies the selection and leaves copy mode, `o` swaps the ends of the
selection (`select_other_end()`), and `<Esc>` clears the selection and stays in copy mode.

Pressing the selection key that started the current selection clears it. Pressing another one
switches to that kind of selection, as in vim's visual mode.

### Counts

A count before a motion repeats it: `5j`, `3w`, `2fa`. `0` continues a count that has started and
is the start-of-line motion otherwise. A count also repeats `n`/`N`. Motions to a fixed place (`0`,
`^`, `$`, `gg`, `G`, `H`, `M`, `L`) ignore the count. A digit that is itself bound in the mode runs
its binding instead of starting a count.

### Words

Word motions use vim's three classes: whitespace, separators, and word characters. A run of
separators is a word of its own, so `w` stops at `foo`, `.`, `bar` in `foo.bar`. The separators are
ASCII punctuation other than `_`. This is tmux's default `word-separators` and, for ASCII text, the
same as vim's default `iskeyword`. Letters, digits, `_` and all non-ASCII characters are word
characters. An empty line counts as a word for `w` and `b`, as in vim. `W`, `B` and `E` split on
whitespace only.

### Paragraphs

`{` and `}` move to the blank line before or after a paragraph, as in vim: from a blank line they
first skip the blank lines next to it. Without another blank line, `{` stops at the start of history
and `}` at the last character of the last line. A line of only spaces counts as blank, because
terminal rows are often padded with them.

### Search

`/` and `?` open a prompt in the status line, which starts empty. `<Enter>` runs the search and
moves the cursor to the next match after it in the search direction, wrapping around. An empty
search clears the highlights. After a `?` search, `n` moves backward and `N` forward. With a
selection active, a search extends it.

## Mouse

With the default `mouse.set_select(true)`, in a pane whose program has not asked for mouse events:

- Dragging with the left button enters copy mode with a selection from the press point, extends it
  while you drag, and scrolls a line at a time while the pointer is above or below the pane.
  Releasing copies the selection and leaves copy mode. This is tmux's default with `mouse on`
  (`copy-pipe-and-cancel` on `MouseDragEnd1Pane`).
- A double click copies the word under the pointer and a triple click copies the line. Unlike tmux,
  the selection is not shown for a moment before copy mode closes.
- A click without a drag only focuses the pane.

If the program has requested mouse reporting (vim with `mouse=a`, for example), presses, drags and
releases go to the program as before. The mouse wheel scrolls as before. In copy mode, scrolling
with the wheel moves the cursor to stay on screen. Scrolling the focused pane back with the wheel
enters copy mode (see [Entering and leaving](#entering-and-leaving)); the wheel over another pane
only scrolls its view.

## Leader bindings

`normal` mode bindings that start with the leader also work in `copy` and `select` mode, as tmux's
prefix table does in copy mode. Each leaves copy mode before it runs, so `<leader>'` switching
session, say, leaves you typing into the program as usual. A binding of the same keys in `copy` or
`select` mode takes precedence, and so does a binding of the leader key itself there: with
`set_leader("<C-b>")`, copy mode's `<C-b>` stays a page up. `unbind` in either mode removes an
inherited binding, or all of them when given the leader key.

## Customising

`copy` and `select` are ordinary binding tables. Bind them from Rhai the way tmux uses
`bind -T copy-mode-vi`:

```rhai
// Move between panes without leaving copy mode.
for mode in ["copy", "select"] {
    bind(mode, "<C-h>", action.focus_left());
    bind(mode, "<C-j>", action.focus_down());
    bind(mode, "<C-k>", action.focus_up());
    bind(mode, "<C-l>", action.focus_right());
}

// tmux's `v` = begin-selection and `y` = copy-selection-and-cancel.
bind("copy", "y", action.yank_selection());
```

The input mode belongs to the client, not to a pane. When focus moves to another pane in copy mode,
the next motion starts copy mode there at the program's cursor. The pane you left keeps its cursor
until copy mode ends.

`action.enter_mode("copy")` and `action.leave_mode()` behave like `enter_copy_mode()` and
`exit_copy_mode()`.

`mouse.set_select(false)` turns off mouse selection.
