//! The test schema, compiled by `buffa` and extended by this plugin.
//!
//! Nothing here is hand-written apart from the two mount points below. The
//! crate exists so the plugin's output is type-checked against real
//! buffa-generated messages and exercised by [`tests/resource.rs`], which is
//! the only way to catch output that is well-formed Rust but wrong.

/// The buffa-generated message types.
pub mod proto {
    #![allow(
        clippy::all,
        clippy::pedantic,
        warnings,
        reason = "buffa-build generated code -- upstream codegen style, not ours to police"
    )]

    include!(concat!(env!("OUT_DIR"), "/proto/_include.rs"));
}

/// The AIP helpers this plugin emits for that schema.
pub mod aip_gen {
    include!(concat!(env!("OUT_DIR"), "/aip/mod.rs"));
}
