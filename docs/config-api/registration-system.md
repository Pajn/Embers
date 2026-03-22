# System (Registration)

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> env </h2>

```rust,ignore
fn env(_: SystemApi, name: String) -> ?
```

<div>
<div class="tab">
<button group="env" id="link-env-Description"  class="tablinks active" 
    onclick="openTab(event, 'env', 'Description')">
Description
</button>
</div>

<div group="env" id="env-Description" class="tabcontent"  style="display: block;" >
Read an environment variable, if it is set.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> now </h2>

```rust,ignore
fn now(_: SystemApi) -> int
```

<div>
<div class="tab">
<button group="now" id="link-now-Description"  class="tablinks active" 
    onclick="openTab(event, 'now', 'Description')">
Description
</button>
</div>

<div group="now" id="now-Description" class="tabcontent"  style="display: block;" >
Return the current Unix timestamp in seconds.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> which </h2>

```rust,ignore
fn which(_: SystemApi, name: String) -> ?
```

<div>
<div class="tab">
<button group="which" id="link-which-Description"  class="tablinks active" 
    onclick="openTab(event, 'which', 'Description')">
Description
</button>
</div>

<div group="which" id="which-Description" class="tabcontent"  style="display: block;" >
Resolve an executable from `PATH`, if it is found.

ReturnType: `string | ()`
</div>

</div>
</div>
</br>
