# Embers Config API

This reference is generated from the Rust-backed Rhai exports used by Embers.

There are two execution phases:

- registration time: the top-level config file where you declare modes, bindings, named actions, and visual settings
- runtime: named actions, event handlers, and tab bar formatters that run against live client state

Definition files live in [`defs/`](defs/).

## Pages

- [action](action.md)
- [buffer-ref](buffer-ref.md)
- [context](context.md)
- [event-info](event-info.md)
- [floating-ref](floating-ref.md)
- [mouse](mouse.md)
- [mux](mux.md)
- [node-ref](node-ref.md)
- [registration-action](registration-action.md)
- [registration-globals](registration-globals.md)
- [registration-system](registration-system.md)
- [registration-tree](registration-tree.md)
- [registration-ui](registration-ui.md)
- [runtime-theme](runtime-theme.md)
- [session-ref](session-ref.md)
- [system-runtime](system-runtime.md)
- [tab-bar-context](tab-bar-context.md)
- [tab-info](tab-info.md)
- [tabbar](tabbar.md)
- [theme](theme.md)
- [tree](tree.md)
- [ui](ui.md)

## Definitions

- [`registration.rhai`](defs/registration.rhai)
- [`runtime.rhai`](defs/runtime.rhai)

## Example

- [`example.md`](example.md)
