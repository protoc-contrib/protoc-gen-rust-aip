//! Generates Rust helpers for [Google AIP](https://google.aip.dev)-shaped APIs
//! from a protobuf schema: typed resource names, `List` request query parsing,
//! and the `OUTPUT_ONLY` clearing walk.
//!
//! The Rust counterpart of
//! [protoc-gen-go-aip](https://github.com/protoc-contrib/protoc-gen-go-aip).
//! Generated code depends on [aip-rs](https://github.com/protoc-contrib/aip-rs)
//! for the parts that are pure data manipulation, and extends the message types
//! emitted by `protoc-gen-buffa`.
//!
//! The binary is the entry point; this library exists so the passes can be
//! driven from a test without going through stdin.

pub mod annotations;
pub mod emit;
mod idents;
pub mod messages;
pub mod scan;
