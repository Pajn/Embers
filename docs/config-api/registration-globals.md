# Registration Globals

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> bind </h2>

```rust,ignore
fn bind(mode: String, notation: String, action: Action)
fn bind(mode: String, notation: String, action_name: String)
fn bind(mode: String, notation: String, actions: Array)
```

<div>
<div class="tab">
<button group="bind" id="link-bind-Description"  class="tablinks active" 
    onclick="openTab(event, 'bind', 'Description')">
Description
</button>
<button group="bind" id="link-bind-Example"  class="tablinks" 
    onclick="openTab(event, 'bind', 'Example')">
Example
</button>
</div>

<div group="bind" id="bind-Description" class="tabcontent"  style="display: block;" >
Bind a key notation to an [`Action`], a string action name, or an array of actions.

Use the `Action` overload for inline builders such as `action.focus_left()`, the string
overload for a named action registered with `define_action`, or an array to chain multiple
actions in sequence.
</div>
<div group="bind" id="bind-Example" class="tabcontent"  style="display: none;" >

```rhai
bind("normal", "<leader>ws", "workspace-split");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> define_action </h2>

```rust,ignore
fn define_action(name: String, callback: FnPtr)
```

<div>
<div class="tab">
<button group="define_action" id="link-define_action-Description"  class="tablinks active" 
    onclick="openTab(event, 'define_action', 'Description')">
Description
</button>
</div>

<div group="define_action" id="define_action-Description" class="tabcontent"  style="display: block;" >
Register a function pointer as a named action callable from bindings.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> define_mode </h2>

```rust,ignore
fn define_mode(mode_name: String)
fn define_mode(mode_name: String, options: Map)
```

<div>
<div class="tab">
<button group="define_mode" id="link-define_mode-Description"  class="tablinks active" 
    onclick="openTab(event, 'define_mode', 'Description')">
Description
</button>
</div>

<div group="define_mode" id="define_mode-Description" class="tabcontent"  style="display: block;" >
Define a custom input mode with hooks and fallback options.

Supported options are `fallback`, `on_enter`, and `on_leave`.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> on </h2>

```rust,ignore
fn on(event_name: String, callback: FnPtr)
```

<div>
<div class="tab">
<button group="on" id="link-on-Description"  class="tablinks active" 
    onclick="openTab(event, 'on', 'Description')">
Description
</button>
</div>

<div group="on" id="on-Description" class="tabcontent"  style="display: block;" >
Attach a callback to an emitted event such as `buffer_bell`.

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_leader </h2>

```rust,ignore
fn set_leader(notation: String)
```

<div>
<div class="tab">
<button group="set_leader" id="link-set_leader-Description"  class="tablinks active" 
    onclick="openTab(event, 'set_leader', 'Description')">
Description
</button>
<button group="set_leader" id="link-set_leader-Example"  class="tablinks" 
    onclick="openTab(event, 'set_leader', 'Example')">
Example
</button>
</div>

<div group="set_leader" id="set_leader-Description" class="tabcontent"  style="display: block;" >
Set the leader sequence used in binding notations.
</div>
<div group="set_leader" id="set_leader-Example" class="tabcontent"  style="display: none;" >

```rhai
set_leader("<C-a>");
```

</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> unbind </h2>

```rust,ignore
fn unbind(mode: String, notation: String)
```

<div>
<div class="tab">
<button group="unbind" id="link-unbind-Description"  class="tablinks active" 
    onclick="openTab(event, 'unbind', 'Description')">
Description
</button>
</div>

<div group="unbind" id="unbind-Description" class="tabcontent"  style="display: block;" >
Remove a previously bound key sequence.

</div>

</div>
</div>
</br>
