# TabInfo

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> buffer_count </h2>

```rust,ignore
fn buffer_count(tab: TabInfo) -> int
```

<div>
<div class="tab">
<button group="buffer_count" id="link-buffer_count-Description"  class="tablinks active"
    onclick="openTab(event, 'buffer_count', 'Description')">
Description
</button>
</div>

<div group="buffer_count" id="buffer_count-Description" class="tabcontent"  style="display: block;" >
Return how many buffers are attached to the tab.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> cwd </h2>

```rust,ignore
fn cwd(tab: TabInfo) -> ?
```

<div>
<div class="tab">
<button group="cwd" id="link-cwd-Description"  class="tablinks active"
    onclick="openTab(event, 'cwd', 'Description')">
Description
</button>
</div>

<div group="cwd" id="cwd-Description" class="tabcontent"  style="display: block;" >
Return the working directory of the pane the tab shows (see
`pane_title`), like tmux's `#{pane_current_path}`.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> has_activity </h2>

```rust,ignore
fn has_activity(tab: TabInfo) -> bool
```

<div>
<div class="tab">
<button group="has_activity" id="link-has_activity-Description"  class="tablinks active"
    onclick="openTab(event, 'has_activity', 'Description')">
Description
</button>
</div>

<div group="has_activity" id="has_activity-Description" class="tabcontent"  style="display: block;" >
Return whether the tab has activity.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> has_bell </h2>

```rust,ignore
fn has_bell(tab: TabInfo) -> bool
```

<div>
<div class="tab">
<button group="has_bell" id="link-has_bell-Description"  class="tablinks active"
    onclick="openTab(event, 'has_bell', 'Description')">
Description
</button>
</div>

<div group="has_bell" id="has_bell-Description" class="tabcontent"  style="display: block;" >
Return whether the tab has a bell marker.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> index </h2>

```rust,ignore
fn index(tab: TabInfo) -> int
```

<div>
<div class="tab">
<button group="index" id="link-index-Description"  class="tablinks active"
    onclick="openTab(event, 'index', 'Description')">
Description
</button>
</div>

<div group="index" id="index-Description" class="tabcontent"  style="display: block;" >
Return the zero-based tab index, as `action.select_tab` takes it.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> is_active </h2>

```rust,ignore
fn is_active(tab: TabInfo) -> bool
```

<div>
<div class="tab">
<button group="is_active" id="link-is_active-Description"  class="tablinks active"
    onclick="openTab(event, 'is_active', 'Description')">
Description
</button>
</div>

<div group="is_active" id="is_active-Description" class="tabcontent"  style="display: block;" >
Return whether the tab is active.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> number </h2>

```rust,ignore
fn number(tab: TabInfo) -> int
```

<div>
<div class="tab">
<button group="number" id="link-number-Description"  class="tablinks active"
    onclick="openTab(event, 'number', 'Description')">
Description
</button>
<button group="number" id="link-number-Example"  class="tablinks"
    onclick="openTab(event, 'number', 'Example')">
Example
</button>
</div>

<div group="number" id="number-Description" class="tabcontent"  style="display: block;" >
Return the tab's window number: its index plus the base index set with
`set_base_index` (0 when unset), like tmux `#I`. Show this in tab bars so
the numbers match CLI targets such as `select-window -t work:2` and
`action.select_window`.
</div>
<div group="number" id="number-Example" class="tabcontent"  style="display: none;" >

```rhai
let label = `${tab.number()}:${tab.title()}`;
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> pane_title </h2>

```rust,ignore
fn pane_title(tab: TabInfo) -> ?
```

<div>
<div class="tab">
<button group="pane_title" id="link-pane_title-Description"  class="tablinks active"
    onclick="openTab(event, 'pane_title', 'Description')">
Description
</button>
</div>

<div group="pane_title" id="pane_title-Description" class="tabcontent"  style="display: block;" >
Return the title of the pane the tab shows: the focused pane when the
tab holds the focus, otherwise the pane its active nested tabs lead to
(the first pane of a split).

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> process_name </h2>

```rust,ignore
fn process_name(tab: TabInfo) -> ?
```

<div>
<div class="tab">
<button group="process_name" id="link-process_name-Description"  class="tablinks active"
    onclick="openTab(event, 'process_name', 'Description')">
Description
</button>
<button group="process_name" id="link-process_name-Example"  class="tablinks"
    onclick="openTab(event, 'process_name', 'Example')">
Example
</button>
</div>

<div group="process_name" id="process_name-Description" class="tabcontent"  style="display: block;" >
Return the name of the program in the foreground of the pane the tab
shows (see `pane_title`), like tmux's `#{pane_current_command}`.

ReturnType: `string | ()`
</div>
<div group="process_name" id="process_name-Example" class="tabcontent"  style="display: none;" >

```rhai
let label = if tab.process_name() == "nvim" { "editor" } else { tab.title() };
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> synchronize_panes </h2>

```rust,ignore
fn synchronize_panes(tab: TabInfo) -> bool
```

<div>
<div class="tab">
<button group="synchronize_panes" id="link-synchronize_panes-Description"  class="tablinks active"
    onclick="openTab(event, 'synchronize_panes', 'Description')">
Description
</button>
<button group="synchronize_panes" id="link-synchronize_panes-Example"  class="tablinks"
    onclick="openTab(event, 'synchronize_panes', 'Example')">
Example
</button>
</div>

<div group="synchronize_panes" id="synchronize_panes-Description" class="tabcontent"  style="display: block;" >
Return whether `synchronize-panes` is on for the window this tab holds, so a formatter
can flag it. Always `false` for nested tabs.
</div>
<div group="synchronize_panes" id="synchronize_panes-Example" class="tabcontent"  style="display: none;" >

```rhai
let label = if tab.synchronize_panes() { tab.title() + " [sync]" } else { tab.title() };
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> title </h2>

```rust,ignore
fn title(tab: TabInfo) -> String
```

<div>
<div class="tab">
<button group="title" id="link-title-Description"  class="tablinks active"
    onclick="openTab(event, 'title', 'Description')">
Description
</button>
</div>

<div group="title" id="title-Description" class="tabcontent"  style="display: block;" >
Return the tab title.
</div>

</div>
</div>
</br>
