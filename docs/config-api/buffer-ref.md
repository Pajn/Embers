# BufferRef

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> activity </h2>

```rust,ignore
fn activity(buffer: BufferRef) -> String
```

<div>
<div class="tab">
<button group="activity" id="link-activity-Description"  class="tablinks active"
    onclick="openTab(event, 'activity', 'Description')">
Description
</button>
</div>

<div group="activity" id="activity-Description" class="tabcontent"  style="display: block;" >
Return the current activity state name.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> command </h2>

```rust,ignore
fn command(buffer: BufferRef) -> Array
```

<div>
<div class="tab">
<button group="command" id="link-command-Description"  class="tablinks active"
    onclick="openTab(event, 'command', 'Description')">
Description
</button>
</div>

<div group="command" id="command-Description" class="tabcontent"  style="display: block;" >
Return the original command vector.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> cwd </h2>

```rust,ignore
fn cwd(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="cwd" id="link-cwd-Description"  class="tablinks active"
    onclick="openTab(event, 'cwd', 'Description')">
Description
</button>
</div>

<div group="cwd" id="cwd-Description" class="tabcontent"  style="display: block;" >
Return the working directory, if any.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> env_hint </h2>

```rust,ignore
fn env_hint(buffer: BufferRef, key: String) -> ?
```

<div>
<div class="tab">
<button group="env_hint" id="link-env_hint-Description"  class="tablinks active"
    onclick="openTab(event, 'env_hint', 'Description')">
Description
</button>
</div>

<div group="env_hint" id="env_hint-Description" class="tabcontent"  style="display: block;" >
Look up a single environment hint captured on the buffer.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> exit_code </h2>

```rust,ignore
fn exit_code(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="exit_code" id="link-exit_code-Description"  class="tablinks active"
    onclick="openTab(event, 'exit_code', 'Description')">
Description
</button>
</div>

<div group="exit_code" id="exit_code-Description" class="tabcontent"  style="display: block;" >
Return the process exit code, if any.

ReturnType: `int | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> history_text </h2>

```rust,ignore
fn history_text(buffer: BufferRef) -> String
```

<div>
<div class="tab">
<button group="history_text" id="link-history_text-Description"  class="tablinks active"
    onclick="openTab(event, 'history_text', 'Description')">
Description
</button>
<button group="history_text" id="link-history_text-Example"  class="tablinks"
    onclick="openTab(event, 'history_text', 'Example')">
Example
</button>
</div>

<div group="history_text" id="history_text-Description" class="tabcontent"  style="display: block;" >
Return the full captured history text for the buffer.
</div>
<div group="history_text" id="history_text-Example" class="tabcontent"  style="display: none;" >

```rhai
let buffer = ctx.current_buffer();
if buffer != () {
    let history = buffer.history_text();
}
```
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> id </h2>

```rust,ignore
fn id(buffer: BufferRef) -> int
```

<div>
<div class="tab">
<button group="id" id="link-id-Description"  class="tablinks active"
    onclick="openTab(event, 'id', 'Description')">
Description
</button>
</div>

<div group="id" id="id-Description" class="tabcontent"  style="display: block;" >
Return the numeric buffer id.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> is_attached </h2>

```rust,ignore
fn is_attached(buffer: BufferRef) -> bool
```

<div>
<div class="tab">
<button group="is_attached" id="link-is_attached-Description"  class="tablinks active"
    onclick="openTab(event, 'is_attached', 'Description')">
Description
</button>
</div>

<div group="is_attached" id="is_attached-Description" class="tabcontent"  style="display: block;" >
Return whether the buffer is currently attached to a node.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> is_detached </h2>

```rust,ignore
fn is_detached(buffer: BufferRef) -> bool
```

<div>
<div class="tab">
<button group="is_detached" id="link-is_detached-Description"  class="tablinks active"
    onclick="openTab(event, 'is_detached', 'Description')">
Description
</button>
</div>

<div group="is_detached" id="is_detached-Description" class="tabcontent"  style="display: block;" >
Return whether the buffer has been detached.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> is_running </h2>

```rust,ignore
fn is_running(buffer: BufferRef) -> bool
```

<div>
<div class="tab">
<button group="is_running" id="link-is_running-Description"  class="tablinks active"
    onclick="openTab(event, 'is_running', 'Description')">
Description
</button>
</div>

<div group="is_running" id="is_running-Description" class="tabcontent"  style="display: block;" >
Return whether the buffer process is still running.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> is_visible </h2>

```rust,ignore
fn is_visible(buffer: BufferRef) -> bool
```

<div>
<div class="tab">
<button group="is_visible" id="link-is_visible-Description"  class="tablinks active"
    onclick="openTab(event, 'is_visible', 'Description')">
Description
</button>
</div>

<div group="is_visible" id="is_visible-Description" class="tabcontent"  style="display: block;" >
Return whether the buffer is visible in the current presentation.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> node_id </h2>

```rust,ignore
fn node_id(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="node_id" id="link-node_id-Description"  class="tablinks active"
    onclick="openTab(event, 'node_id', 'Description')">
Description
</button>
</div>

<div group="node_id" id="node_id-Description" class="tabcontent"  style="display: block;" >
Return the attached node id, if any.

ReturnType: `int | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> pid </h2>

```rust,ignore
fn pid(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="pid" id="link-pid-Description"  class="tablinks active"
    onclick="openTab(event, 'pid', 'Description')">
Description
</button>
</div>

<div group="pid" id="pid-Description" class="tabcontent"  style="display: block;" >
Return the process id, if any.

ReturnType: `int | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> process_name </h2>

```rust,ignore
fn process_name(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="process_name" id="link-process_name-Description"  class="tablinks active"
    onclick="openTab(event, 'process_name', 'Description')">
Description
</button>
</div>

<div group="process_name" id="process_name-Description" class="tabcontent"  style="display: block;" >
Return the detected process name, if any.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> session_id </h2>

```rust,ignore
fn session_id(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="session_id" id="link-session_id-Description"  class="tablinks active"
    onclick="openTab(event, 'session_id', 'Description')">
Description
</button>
</div>

<div group="session_id" id="session_id-Description" class="tabcontent"  style="display: block;" >
Return the attached session id, if any.

ReturnType: `int | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> snapshot_text </h2>

```rust,ignore
fn snapshot_text(buffer: BufferRef, limit: int) -> String
```

<div>
<div class="tab">
<button group="snapshot_text" id="link-snapshot_text-Description"  class="tablinks active"
    onclick="openTab(event, 'snapshot_text', 'Description')">
Description
</button>
</div>

<div group="snapshot_text" id="snapshot_text-Description" class="tabcontent"  style="display: block;" >
Return a text snapshot limited to the requested line count.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> title </h2>

```rust,ignore
fn title(buffer: BufferRef) -> String
```

<div>
<div class="tab">
<button group="title" id="link-title-Description"  class="tablinks active"
    onclick="openTab(event, 'title', 'Description')">
Description
</button>
</div>

<div group="title" id="title-Description" class="tabcontent"  style="display: block;" >
Return the buffer title.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> tty_path </h2>

```rust,ignore
fn tty_path(buffer: BufferRef) -> ?
```

<div>
<div class="tab">
<button group="tty_path" id="link-tty_path-Description"  class="tablinks active"
    onclick="openTab(event, 'tty_path', 'Description')">
Description
</button>
</div>

<div group="tty_path" id="tty_path-Description" class="tabcontent"  style="display: block;" >
Return the controlling TTY path, if any.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
