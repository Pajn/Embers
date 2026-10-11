# Theme

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_palette </h2>

```rust,ignore
fn set_palette(theme: ThemeApi, palette: Map)
```

<div>
<div class="tab">
<button group="set_palette" id="link-set_palette-Description"  class="tablinks active"
    onclick="openTab(event, 'set_palette', 'Description')">
Description
</button>
</div>

<div group="set_palette" id="set_palette-Description" class="tabcontent"  style="display: block;" >
Add named colors to the theme palette.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_pane_active_border_color </h2>

```rust,ignore
fn set_pane_active_border_color(theme: ThemeApi, color: String)
```

<div>
<div class="tab">
<button group="set_pane_active_border_color" id="link-set_pane_active_border_color-Description"  class="tablinks active"
    onclick="openTab(event, 'set_pane_active_border_color', 'Description')">
Description
</button>
<button group="set_pane_active_border_color" id="link-set_pane_active_border_color-Example"  class="tablinks"
    onclick="openTab(event, 'set_pane_active_border_color', 'Example')">
Example
</button>
</div>

<div group="set_pane_active_border_color" id="set_pane_active_border_color-Description" class="tabcontent"  style="display: block;" >
Color the divider cells next to the focused pane, like tmux's
`pane-active-border-style fg=...`. Takes the same colors as
`set_pane_border_color`. Without it they are drawn like the others.
</div>
<div group="set_pane_active_border_color" id="set_pane_active_border_color-Example" class="tabcontent"  style="display: none;" >

```rhai
theme.set_pane_active_border_color("magenta");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_pane_border_color </h2>

```rust,ignore
fn set_pane_border_color(theme: ThemeApi, color: String)
```

<div>
<div class="tab">
<button group="set_pane_border_color" id="link-set_pane_border_color-Description"  class="tablinks active"
    onclick="openTab(event, 'set_pane_border_color', 'Description')">
Description
</button>
<button group="set_pane_border_color" id="link-set_pane_border_color-Example"  class="tablinks"
    onclick="openTab(event, 'set_pane_border_color', 'Example')">
Example
</button>
</div>

<div group="set_pane_border_color" id="set_pane_border_color-Description" class="tabcontent"  style="display: block;" >
Color the dividers between panes, except next to the focused pane,
like tmux's `pane-border-style fg=...`. Takes `"#RRGGBB"` or one of
the sixteen ANSI color names (`"black"` to `"white"` and
`"brightblack"` to `"brightwhite"`), which follow the terminal's color
scheme. Without it dividers are drawn dim.
</div>
<div group="set_pane_border_color" id="set_pane_border_color-Example" class="tabcontent"  style="display: none;" >

```rhai
theme.set_pane_border_color("brightblack");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_popup_active_border_color </h2>

```rust,ignore
fn set_popup_active_border_color(theme: ThemeApi, color: String)
```

<div>
<div class="tab">
<button group="set_popup_active_border_color" id="link-set_popup_active_border_color-Description"  class="tablinks active"
    onclick="openTab(event, 'set_popup_active_border_color', 'Description')">
Description
</button>
<button group="set_popup_active_border_color" id="link-set_popup_active_border_color-Example"  class="tablinks"
    onclick="openTab(event, 'set_popup_active_border_color', 'Example')">
Example
</button>
</div>

<div group="set_popup_active_border_color" id="set_popup_active_border_color-Description" class="tabcontent"  style="display: block;" >
Color the frame and title of the focused popup. Takes the same colors
as `set_pane_border_color`. Without it the frame is drawn bold in the
terminal's default color.
</div>
<div group="set_popup_active_border_color" id="set_popup_active_border_color-Example" class="tabcontent"  style="display: none;" >

```rhai
theme.set_popup_active_border_color("magenta");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_popup_border_color </h2>

```rust,ignore
fn set_popup_border_color(theme: ThemeApi, color: String)
```

<div>
<div class="tab">
<button group="set_popup_border_color" id="link-set_popup_border_color-Description"  class="tablinks active"
    onclick="openTab(event, 'set_popup_border_color', 'Description')">
Description
</button>
<button group="set_popup_border_color" id="link-set_popup_border_color-Example"  class="tablinks"
    onclick="openTab(event, 'set_popup_border_color', 'Example')">
Example
</button>
</div>

<div group="set_popup_border_color" id="set_popup_border_color-Description" class="tabcontent"  style="display: block;" >
Color the frames and titles of popups that do not have focus, like
tmux's `popup-border-style fg=...`. Takes the same colors as
`set_pane_border_color`. Without it they are drawn dim.
</div>
<div group="set_popup_border_color" id="set_popup_border_color-Example" class="tabcontent"  style="display: none;" >

```rhai
theme.set_popup_border_color("brightblack");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_popup_border_lines </h2>

```rust,ignore
fn set_popup_border_lines(theme: ThemeApi, lines: String)
```

<div>
<div class="tab">
<button group="set_popup_border_lines" id="link-set_popup_border_lines-Description"  class="tablinks active"
    onclick="openTab(event, 'set_popup_border_lines', 'Description')">
Description
</button>
<button group="set_popup_border_lines" id="link-set_popup_border_lines-Example"  class="tablinks"
    onclick="openTab(event, 'set_popup_border_lines', 'Example')">
Example
</button>
</div>

<div group="set_popup_border_lines" id="set_popup_border_lines-Description" class="tabcontent"  style="display: block;" >
Draw popup (floating window) frames with `"rounded"` (the default,
`╭─╮`), `"single"` (`┌─┐`), `"double"` (`╔═╗`), `"heavy"` (`┏━┓`),
`"ascii"` (`+-+`) or `"none"`, like tmux's `popup-border-lines`. With
`"none"` the frame cells are left blank and the title stays on the top
one: the popup's panes keep their size.
</div>
<div group="set_popup_border_lines" id="set_popup_border_lines-Example" class="tabcontent"  style="display: none;" >

```rhai
theme.set_popup_border_lines("single");
```

</div>

</div>
</div>
</br>
