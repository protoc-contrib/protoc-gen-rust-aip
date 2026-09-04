//! Compiles the vendored `google/api` annotation protos into Rust types, so
//! the plugin can read `(google.api.resource)` and friends off the descriptor
//! options with `buffa`'s typed extension accessors rather than by hand-decoding
//! unknown fields.
//!
//! The protos are vendored under this crate's own `proto/` directory: a protoc
//! plugin is installed with `cargo install`, which has no `buf` and no
//! googleapis checkout to point at.

use std::path::{Path, PathBuf};

/// Annotation protos the plugin reads. All three live in `google.api` and
/// extend `descriptor.proto`'s options messages.
const ANNOTATIONS: &[&str] = &[
    "google/api/resource.proto",
    "google/api/field_behavior.proto",
    "google/api/field_info.proto",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let proto_root = manifest.join("proto");

    for annotation in ANNOTATIONS {
        println!(
            "cargo:rerun-if-changed={}",
            proto_root.join(annotation).display()
        );
    }
    println!("cargo:rerun-if-env-changed=PROTOC");

    // The annotations import `google/protobuf/descriptor.proto`, which ships
    // beside protoc rather than with them. Locate it via protoc's own prefix.
    let include = protoc_include()
        .expect("google/protobuf/descriptor.proto not found; install protoc or set PROTOC");

    let files: Vec<String> = ANNOTATIONS
        .iter()
        .map(|a| path_str(&proto_root.join(a)))
        .collect();
    let includes = [path_str(&proto_root), path_str(&include)];

    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let includes: Vec<&str> = includes.iter().map(String::as_str).collect();

    buffa_build::Config::new()
        .files(&files)
        .includes(&includes)
        // `descriptor.proto` is imported only to be extended -- an extendee is
        // named by string, so nothing here refers to a `google.protobuf` type.
        // Generating it would fork the descriptor types the plugin itself reads
        // from `buffa-codegen` into a second, incompatible copy.
        .exclude_package("google.protobuf")
        // Nothing reads these annotations off the wire without owning them
        // first, so the zero-copy view types are dead weight.
        .generate_views(false)
        .include_file("_include.rs")
        .compile()
        .expect("google/api annotation compilation failed");
}

/// The directory holding protoc's bundled well-known types, found by stepping
/// from the `protoc` binary up to its prefix and back down into `include/`.
fn protoc_include() -> Option<PathBuf> {
    let protoc = std::env::var("PROTOC").ok().or_else(which_protoc)?;
    PathBuf::from(protoc)
        .parent() // bin/
        .and_then(Path::parent) // prefix/
        .map(|prefix| prefix.join("include"))
        .filter(|include| include.join("google/protobuf/descriptor.proto").exists())
}

/// Resolves `protoc` on `PATH`. Unix-only; Windows is not a supported build
/// host for this crate.
fn which_protoc() -> Option<String> {
    let output = std::process::Command::new("which")
        .arg("protoc")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    Some(
        std::fs::canonicalize(&path)
            .unwrap_or_else(|_| PathBuf::from(&path))
            .to_string_lossy()
            .into_owned(),
    )
}

fn path_str(path: &Path) -> String {
    path.to_str().expect("utf-8 path").to_owned()
}
