//! Raw bindings to libghostty-vt, Ghostty's terminal emulation library.
//!
//! The bindings in `bindings.rs` are generated from the headers of the Ghostty
//! commit `ghostty_source.rs` pins; regenerate them with `cargo gen-ghostty`
//! after changing it.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(clippy::all)]

mod bindings;

pub use bindings::*;
