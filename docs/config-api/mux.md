# Mux

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> current_buffer </h2>

```rust,ignore
fn current_buffer(mux: MuxApi) -> ?
```

<div>
<div class="tab">
<button group="current_buffer" id="link-current_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'current_buffer', 'Description')">
Description
</button>
</div>

<div group="current_buffer" id="current_buffer-Description" class="tabcontent"  style="display: block;" >
Return the currently focused buffer, if any.

ReturnType: `BufferRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> current_floating </h2>

```rust,ignore
fn current_floating(mux: MuxApi) -> ?
```

<div>
<div class="tab">
<button group="current_floating" id="link-current_floating-Description"  class="tablinks active"
    onclick="openTab(event, 'current_floating', 'Description')">
Description
</button>
</div>

<div group="current_floating" id="current_floating-Description" class="tabcontent"  style="display: block;" >
Return the currently focused floating window, if any.

ReturnType: `FloatingRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> current_node </h2>

```rust,ignore
fn current_node(mux: MuxApi) -> ?
```

<div>
<div class="tab">
<button group="current_node" id="link-current_node-Description"  class="tablinks active"
    onclick="openTab(event, 'current_node', 'Description')">
Description
</button>
</div>

<div group="current_node" id="current_node-Description" class="tabcontent"  style="display: block;" >
Return the currently focused node, if any.

ReturnType: `NodeRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> current_session </h2>

```rust,ignore
fn current_session(mux: MuxApi) -> ?
```

<div>
<div class="tab">
<button group="current_session" id="link-current_session-Description"  class="tablinks active"
    onclick="openTab(event, 'current_session', 'Description')">
Description
</button>
</div>

<div group="current_session" id="current_session-Description" class="tabcontent"  style="display: block;" >
Return the current session reference, if any.

ReturnType: `SessionRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> detached_buffers </h2>

```rust,ignore
fn detached_buffers(mux: MuxApi) -> Array
```

<div>
<div class="tab">
<button group="detached_buffers" id="link-detached_buffers-Description"  class="tablinks active"
    onclick="openTab(event, 'detached_buffers', 'Description')">
Description
</button>
</div>

<div group="detached_buffers" id="detached_buffers-Description" class="tabcontent"  style="display: block;" >
Return detached buffers in the current model snapshot.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> find_buffer </h2>

```rust,ignore
fn find_buffer(mux: MuxApi, buffer_id: int) -> ?
```

<div>
<div class="tab">
<button group="find_buffer" id="link-find_buffer-Description"  class="tablinks active"
    onclick="openTab(event, 'find_buffer', 'Description')">
Description
</button>
</div>

<div group="find_buffer" id="find_buffer-Description" class="tabcontent"  style="display: block;" >
Find a buffer by numeric id. Returns `()` when it does not exist.

ReturnType: `BufferRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> find_floating </h2>

```rust,ignore
fn find_floating(mux: MuxApi, floating_id: int) -> ?
```

<div>
<div class="tab">
<button group="find_floating" id="link-find_floating-Description"  class="tablinks active"
    onclick="openTab(event, 'find_floating', 'Description')">
Description
</button>
</div>

<div group="find_floating" id="find_floating-Description" class="tabcontent"  style="display: block;" >
Find a floating window by numeric id. Returns `()` when it does not exist.

ReturnType: `FloatingRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> find_node </h2>

```rust,ignore
fn find_node(mux: MuxApi, node_id: int) -> ?
```

<div>
<div class="tab">
<button group="find_node" id="link-find_node-Description"  class="tablinks active"
    onclick="openTab(event, 'find_node', 'Description')">
Description
</button>
</div>

<div group="find_node" id="find_node-Description" class="tabcontent"  style="display: block;" >
Find a node by numeric id. Returns `()` when it does not exist.

ReturnType: `NodeRef | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> sessions </h2>

```rust,ignore
fn sessions(mux: MuxApi) -> Array
```

<div>
<div class="tab">
<button group="sessions" id="link-sessions-Description"  class="tablinks active"
    onclick="openTab(event, 'sessions', 'Description')">
Description
</button>
</div>

<div group="sessions" id="sessions-Description" class="tabcontent"  style="display: block;" >
Return every visible session.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> visible_buffers </h2>

```rust,ignore
fn visible_buffers(mux: MuxApi) -> Array
```

<div>
<div class="tab">
<button group="visible_buffers" id="link-visible_buffers-Description"  class="tablinks active"
    onclick="openTab(event, 'visible_buffers', 'Description')">
Description
</button>
</div>

<div group="visible_buffers" id="visible_buffers-Description" class="tabcontent"  style="display: block;" >
Return visible buffers in the current model snapshot.
</div>

</div>
</div>
</br>
