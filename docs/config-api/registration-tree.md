# Tree (Registration)

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> buffer_attach </h2>

```rust,ignore
fn buffer_attach(_: TreeApi, buffer_id: int) -> TreeSpec
```

<div>
<div class="tab">
<button group="buffer_attach" id="link-buffer_attach-Description"  class="tablinks active"
    onclick="openTab(event, 'buffer_attach', 'Description')">
Description
</button>
</div>

<div group="buffer_attach" id="buffer_attach-Description" class="tabcontent"  style="display: block;" >
Attach an existing buffer by id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> buffer_current </h2>

```rust,ignore
fn buffer_current(_: TreeApi) -> TreeSpec
```

<div>
<div class="tab">
<button group="buffer_current" id="link-buffer_current-Description"  class="tablinks active"
    onclick="openTab(event, 'buffer_current', 'Description')">
Description
</button>
</div>

<div group="buffer_current" id="buffer_current-Description" class="tabcontent"  style="display: block;" >
Build a tree reference to the currently focused buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> buffer_empty </h2>

```rust,ignore
fn buffer_empty(_: TreeApi) -> TreeSpec
```

<div>
<div class="tab">
<button group="buffer_empty" id="link-buffer_empty-Description"  class="tablinks active"
    onclick="openTab(event, 'buffer_empty', 'Description')">
Description
</button>
</div>

<div group="buffer_empty" id="buffer_empty-Description" class="tabcontent"  style="display: block;" >
Build an empty buffer tree node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> buffer_spawn </h2>

```rust,ignore
fn buffer_spawn(_: TreeApi, command: Array) -> TreeSpec
fn buffer_spawn(_: TreeApi, command: Array, options: Map) -> TreeSpec
```

<div>
<div class="tab">
<button group="buffer_spawn" id="link-buffer_spawn-Description"  class="tablinks active"
    onclick="openTab(event, 'buffer_spawn', 'Description')">
Description
</button>
<button group="buffer_spawn" id="link-buffer_spawn-Example"  class="tablinks"
    onclick="openTab(event, 'buffer_spawn', 'Example')">
Example
</button>
</div>

<div group="buffer_spawn" id="buffer_spawn-Description" class="tabcontent"  style="display: block;" >
Spawn a new buffer from a command array.

Supported `options` keys are `title` (`string`), `cwd` (`string`), and `env`
(`map<string, string>`). Unknown keys are rejected.
</div>
<div group="buffer_spawn" id="buffer_spawn-Example" class="tabcontent"  style="display: none;" >

```rhai
tree.buffer_spawn(["/bin/zsh"], #{ title: "shell" })
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> current_buffer </h2>

```rust,ignore
fn current_buffer(_: TreeApi) -> TreeSpec
```

<div>
<div class="tab">
<button group="current_buffer" id="link-current_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'current_buffer', 'Description')">
Description
</button>
</div>

<div group="current_buffer" id="current_buffer-Description" class="tabcontent"  style="display: block;" >
Build a tree reference to the currently focused buffer.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> current_node </h2>

```rust,ignore
fn current_node(_: TreeApi) -> TreeSpec
```

<div>
<div class="tab">
<button group="current_node" id="link-current_node-Description"  class="tablinks active"
    onclick="openTab(event, 'current_node', 'Description')">
Description
</button>
</div>

<div group="current_node" id="current_node-Description" class="tabcontent"  style="display: block;" >
Build a tree reference to the currently focused node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> split </h2>

```rust,ignore
fn split(_: TreeApi, direction: String, children: Array) -> TreeSpec
fn split(_: TreeApi, direction: String, children: Array, sizes: Array) -> TreeSpec
```

<div>
<div class="tab">
<button group="split" id="link-split-Description"  class="tablinks active"
    onclick="openTab(event, 'split', 'Description')">
Description
</button>
</div>

<div group="split" id="split-Description" class="tabcontent"  style="display: block;" >
Build a split with an explicit direction string.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> split_h </h2>

```rust,ignore
fn split_h(_: TreeApi, children: Array) -> TreeSpec
```

<div>
<div class="tab">
<button group="split_h" id="link-split_h-Description"  class="tablinks active"
    onclick="openTab(event, 'split_h', 'Description')">
Description
</button>
</div>

<div group="split_h" id="split_h-Description" class="tabcontent"  style="display: block;" >
Build a horizontal split.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> split_v </h2>

```rust,ignore
fn split_v(_: TreeApi, children: Array) -> TreeSpec
```

<div>
<div class="tab">
<button group="split_v" id="link-split_v-Description"  class="tablinks active"
    onclick="openTab(event, 'split_v', 'Description')">
Description
</button>
</div>

<div group="split_v" id="split_v-Description" class="tabcontent"  style="display: block;" >
Build a vertical split.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> tab </h2>

```rust,ignore
fn tab(_: TreeApi, title: String, tree: TreeSpec) -> TabSpec
```

<div>
<div class="tab">
<button group="tab" id="link-tab-Description"  class="tablinks active"
    onclick="openTab(event, 'tab', 'Description')">
Description
</button>
</div>

<div group="tab" id="tab-Description" class="tabcontent"  style="display: block;" >
Build a single tab specification.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> tabs </h2>

```rust,ignore
fn tabs(_: TreeApi, tabs: Array) -> TreeSpec
```

<div>
<div class="tab">
<button group="tabs" id="link-tabs-Description"  class="tablinks active"
    onclick="openTab(event, 'tabs', 'Description')">
Description
</button>
</div>

<div group="tabs" id="tabs-Description" class="tabcontent"  style="display: block;" >
Build a tabs container with the first tab active.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> tabs_with_active </h2>

```rust,ignore
fn tabs_with_active(_: TreeApi, tabs: Array, active: int) -> TreeSpec
```

<div>
<div class="tab">
<button group="tabs_with_active" id="link-tabs_with_active-Description"  class="tablinks active"
    onclick="openTab(event, 'tabs_with_active', 'Description')">
Description
</button>
</div>

<div group="tabs_with_active" id="tabs_with_active-Description" class="tabcontent"  style="display: block;" >
Build a tabs container with an explicit active tab.
</div>

</div>
</div>
</br>
