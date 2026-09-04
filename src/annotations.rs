//! The vendored `google.api` annotation types, compiled by `build.rs`.
//!
//! This is `buffa-build` output verbatim; the crate's lints are switched off
//! for it rather than fought with.

#![allow(
    clippy::all,
    clippy::pedantic,
    reason = "buffa-build generated code -- upstream codegen style, not ours to police"
)]
#![allow(
    warnings,
    reason = "generated code emits unused-import and dead-code warnings for the parts of the annotations this plugin does not read"
)]

include!(concat!(env!("OUT_DIR"), "/_include.rs"));
