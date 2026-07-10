# Hints

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> set_patterns </h2>

```rust,ignore
fn set_patterns(hints: HintsApi, patterns: Array)
```

<div>
<div class="tab">
<button group="set_patterns" id="link-set_patterns-Description"  class="tablinks active"
    onclick="openTab(event, 'set_patterns', 'Description')">
Description
</button>
<button group="set_patterns" id="link-set_patterns-Example"  class="tablinks"
    onclick="openTab(event, 'set_patterns', 'Example')">
Example
</button>
</div>

<div group="set_patterns" id="set_patterns-Description" class="tabcontent"  style="display: block;" >
Replace the hint patterns used by `action.enter_hints()` with the given
regex strings. A non-string element or an invalid regex is an error. An
empty list restores the built-in default set (URLs, paths, SHAs, UUIDs,
IPs, numbers).
</div>
<div group="set_patterns" id="set_patterns-Example" class="tabcontent"  style="display: none;" >

```rhai
hints.set_patterns(["https?://\\S+", "[0-9a-f]{7,40}"]);
```

</div>

</div>
</div>
</br>
