//! `src/generated` is buffa's output for the vendored `google/api` annotation
//! protos under `proto/`, committed so `cargo install` needs no `protoc`. This
//! regenerates it and fails if the committed copy has drifted -- from the
//! protos, or from the buffa it targets.
//!
//! Regenerate with `UPDATE_GENERATED=1 cargo test --test generated` and review
//! the diff.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Annotation protos the plugin reads. All three live in `google.api` and
/// extend `descriptor.proto`'s options messages.
const ANNOTATIONS: &[&str] = &[
    "google/api/resource.proto",
    "google/api/field_behavior.proto",
    "google/api/field_info.proto",
];

#[test]
fn committed_code_matches_a_fresh_generation() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let proto_root = &manifest.join("proto");
    let committed = manifest.join("src/generated");
    let fresh = Path::new(env!("CARGO_TARGET_TMPDIR")).join("annotations");
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(&fresh).unwrap();

    // The annotations import `google/protobuf/descriptor.proto`, which ships
    // beside protoc rather than with them.
    let include = protoc_include()
        .expect("google/protobuf/descriptor.proto not found; install protoc or set PROTOC");
    buffa_build::Config::new()
        .files(
            &ANNOTATIONS
                .iter()
                .map(|proto| path(&proto_root.join(proto)))
                .collect::<Vec<_>>(),
        )
        .includes(&[path(proto_root), path(&include)])
        .out_dir(&fresh)
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
        .expect("buffa codegen");

    if std::env::var_os("UPDATE_GENERATED").is_some() {
        let _ = std::fs::remove_dir_all(&committed);
        std::fs::create_dir_all(&committed).unwrap();
        for name in files(&fresh) {
            std::fs::copy(fresh.join(&name), committed.join(&name)).unwrap();
        }
        return;
    }

    let names: BTreeSet<_> = files(&fresh).union(&files(&committed)).cloned().collect();
    let stale: Vec<_> = names
        .into_iter()
        .filter(|name| {
            std::fs::read_to_string(fresh.join(name)).ok()
                != std::fs::read_to_string(committed.join(name)).ok()
        })
        .collect();
    assert!(
        stale.is_empty(),
        "src/generated is out of date: {stale:?}\n\
         rerun with UPDATE_GENERATED=1 and review the diff"
    );
}

fn files(directory: &Path) -> BTreeSet<PathBuf> {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| PathBuf::from(entry.file_name()))
        .collect()
}

/// The directory holding protoc's bundled well-known types, found by stepping
/// from the `protoc` binary up to its prefix and back down into `include/`.
/// A symlinked protoc -- a Homebrew shim, say -- has its includes beside the
/// real binary rather than beside the link.
fn protoc_include() -> Option<PathBuf> {
    let protoc = std::env::var_os("PROTOC").map(PathBuf::from).or_else(|| {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|directory| directory.join("protoc"))
            .find(|candidate| candidate.is_file())
    })?;
    let protoc = std::fs::canonicalize(&protoc).unwrap_or(protoc);
    protoc
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join("include"))
        .filter(|include| include.join("google/protobuf/descriptor.proto").exists())
}

fn path(path: &Path) -> String {
    path.to_str().expect("utf-8 path").to_owned()
}
