# Tabbar

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_formatter </h2>

```rust,ignore
fn set_formatter(tabbar: TabbarApi, callback: FnPtr)
```

<div>
<div class="tab">
<button group="set_formatter" id="link-set_formatter-Description"  class="tablinks active"
    onclick="openTab(event, 'set_formatter', 'Description')">
Description
</button>
</div>

<div group="set_formatter" id="set_formatter-Description" class="tabcontent"  style="display: block;" >
Register the function used to format the tab bar.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_position </h2>

```rust,ignore
fn set_position(tabbar: TabbarApi, value: String)
```

<div>
<div class="tab">
<button group="set_position" id="link-set_position-Description"  class="tablinks active"
    onclick="openTab(event, 'set_position', 'Description')">
Description
</button>
<button group="set_position" id="link-set_position-Example"  class="tablinks"
    onclick="openTab(event, 'set_position', 'Example')">
Example
</button>
</div>

<div group="set_position" id="set_position-Description" class="tabcontent"  style="display: block;" >
Put the session's tab bar, its list of windows, at the `"top"` (the
default) or `"bottom"` of the screen, like tmux's `status-position`.
The command prompt and messages are drawn over it while they show, so
they move with it. Tab bars of nested tabs stay at the top of their
own area.
</div>
<div group="set_position" id="set_position-Example" class="tabcontent"  style="display: none;" >

```rhai
tabbar.set_position("bottom");
```

</div>

</div>
</div>
</br>
