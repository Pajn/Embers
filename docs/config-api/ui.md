# UI

```Namespace: global```

<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> bar </h2>

```rust,ignore
fn bar(_: UiApi, left: Array, center: Array, right: Array) -> BarSpec
```

<div>
<div class="tab">
<button group="bar" id="link-bar-Description"  class="tablinks active" 
    onclick="openTab(event, 'bar', 'Description')">
Description
</button>
</div>

<div group="bar" id="bar-Description" class="tabcontent"  style="display: block;" >
Build a full bar specification from left, center, and right segments.
</div>

</div>
</div>
</br>
<div style='box-shadow: 0 4px 8px 0 rgba(0,0,0,0.2); padding: 15px; border-radius: 5px; border: 1px solid var(--theme-hover)'>
    <h2 class="func-name"> <code>fn</code> segment </h2>

```rust,ignore
fn segment(_: UiApi, text: String) -> BarSegment
fn segment(_: UiApi, text: String, options: Map) -> BarSegment
```

<div>
<div class="tab">
<button group="segment" id="link-segment-Description"  class="tablinks active" 
    onclick="openTab(event, 'segment', 'Description')">
Description
</button>
</div>

<div group="segment" id="segment-Description" class="tabcontent"  style="display: block;" >
Create a [`BarSegment`] from a [`UiApi`] receiver and text using default styling.

`segment(_: UiApi, text: String) -> BarSegment` produces plain text with default
[`StyleSpec`] values and no click target.
</div>

</div>
</div>
</br>
