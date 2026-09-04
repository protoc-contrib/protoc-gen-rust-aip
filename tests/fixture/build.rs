//! Generates the fixture crate twice over: once with `buffa-build`, for the
//! message types, and once with this plugin, for the AIP helpers that extend
//! them.
//!
//! Running the plugin as a library rather than as a `protoc` plugin binary
//! keeps this to one `cargo build` — a build script cannot depend on a binary
//! produced by the same build — and exercises the same two passes the binary
//! runs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use buffa::Message;
use buffa_codegen::generated::{
    compiler::CodeGeneratorRequest,
    descriptor::{FileDescriptorProto, FileDescriptorSet},
};
use protoc_gen_rust_aip::{emit, messages, scan};

/// The schema under test, relative to the sibling `tests/proto`.
const SCHEMA: &[&str] = &[
    "example/v1/library.proto",
    "example/v1/uuid.proto",
    "example/v1/behavior.proto",
    "example/v1/query.proto",
    "other/v1/catalog.proto",
];

fn main() -> Result<()> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    // This crate lives at `<plugin>/tests/fixture`, so the schema is its
    // sibling and the plugin root is two levels up.
    let tests = manifest.parent().context("fixture sits under tests/")?;
    let plugin = tests
        .parent()
        .context("tests/ sits under the plugin root")?;
    let schema_root = tests.join("proto");
    // The plugin vendors the `google/api` annotations the schema imports.
    let annotations = plugin.join("proto");

    for file in SCHEMA {
        println!(
            "cargo:rerun-if-changed={}",
            schema_root.join(file).display()
        );
    }

    let files: Vec<String> = SCHEMA
        .iter()
        .map(|file| path(&schema_root.join(file)))
        .collect();
    let includes = [path(&schema_root), path(&annotations)];
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let includes: Vec<&str> = includes.iter().map(String::as_str).collect();

    buffa_build::Config::new()
        .files(&files)
        .includes(&includes)
        .out_dir(out.join("proto"))
        // The annotations are read by the plugin, not used as field types.
        .exclude_package("google.api")
        .exclude_package("google.protobuf")
        .generate_views(false)
        .include_file("_include.rs")
        .compile()
        .map_err(|error| anyhow::anyhow!("buffa codegen: {error}"))?;

    let request = request(&schema_root, &includes, &out)?;
    let registry = scan::gather(&request)?;
    let index = messages::gather(&request);
    let generated = emit::render(
        &request,
        &registry,
        &index,
        &emit::Options {
            proto_module: "crate::proto".to_owned(),
        },
    )?;

    let aip = out.join("aip");
    std::fs::create_dir_all(&aip)?;
    for file in generated {
        let name = file.name.context("generated file has a name")?;
        let content = file.content.unwrap_or_default();
        std::fs::write(aip.join(&name), content).with_context(|| format!("write {name}"))?;
    }
    Ok(())
}

/// Builds the `CodeGeneratorRequest` the plugin would have received on stdin,
/// out of a descriptor set `protoc` writes for the schema.
fn request(schema_root: &Path, includes: &[&str], out: &Path) -> Result<CodeGeneratorRequest> {
    let descriptors = out.join("schema.bin");
    let protoc = std::env::var("PROTOC").unwrap_or_else(|_| "protoc".to_owned());
    let mut command = std::process::Command::new(protoc);
    command
        .arg("--include_imports")
        .arg(format!("--descriptor_set_out={}", descriptors.display()));
    for include in includes {
        command.arg(format!("-I{include}"));
    }
    for file in SCHEMA {
        command.arg(schema_root.join(file));
    }
    let output = command.output().context("run protoc")?;
    if !output.status.success() {
        bail!("protoc: {}", String::from_utf8_lossy(&output.stderr));
    }

    let set = FileDescriptorSet::decode_from_slice(&std::fs::read(&descriptors)?)?;
    Ok(CodeGeneratorRequest {
        file_to_generate: SCHEMA.iter().map(|file| (*file).to_owned()).collect(),
        proto_file: set.file.into_iter().collect::<Vec<FileDescriptorProto>>(),
        ..Default::default()
    })
}

fn path(path: &Path) -> String {
    path.to_str().expect("utf-8 path").to_owned()
}
