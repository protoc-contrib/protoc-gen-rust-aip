//! Reads a `CodeGeneratorRequest` on stdin, writes a `CodeGeneratorResponse`
//! on stdout. The protoc plugin protocol, and nothing else.

use std::io::{self, Read, Write};

use anyhow::Result;
use buffa::Message;
use buffa_codegen::generated::{
    compiler::{CodeGeneratorRequest, CodeGeneratorResponse, code_generator_response::File},
    descriptor::Edition,
};
use protoc_gen_rust_aip::{emit, messages, scan};

/// `PROTO3_OPTIONAL | SUPPORTS_EDITIONS`, from
/// `CodeGeneratorResponse.Feature`.
const SUPPORTED_FEATURES: u64 = 1 | 2;

fn main() -> Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let request = CodeGeneratorRequest::decode_from_slice(&input)?;

    // A failed generation still exits 0 with the error in the response: that is
    // how protoc reports a plugin's own diagnostics rather than "plugin failed
    // with status code 1".
    let response = match run(&request) {
        Ok(file) => CodeGeneratorResponse {
            supported_features: Some(SUPPORTED_FEATURES),
            minimum_edition: Some(Edition::EDITION_PROTO2 as i32),
            maximum_edition: Some(Edition::EDITION_2024 as i32),
            file,
            error: None,
            ..Default::default()
        },
        Err(error) => CodeGeneratorResponse {
            error: Some(format!("{error:#}")),
            ..Default::default()
        },
    };

    let mut output = Vec::new();
    response.encode(&mut output);
    io::stdout().write_all(&output)?;
    Ok(())
}

fn run(request: &CodeGeneratorRequest) -> Result<Vec<File>> {
    let options = parse_options(request.parameter.as_deref().unwrap_or_default());
    let registry = scan::gather(request)?;
    let index = messages::gather(request);
    emit::render(request, &registry, &index, &options)
}

/// Parses the comma-separated `key=value` list protoc and buf pass as the
/// plugin parameter.
///
/// An unknown key is ignored rather than rejected, so a `buf.gen.yaml` written
/// against a later version of this plugin still works against this one. A
/// boolean whose value is neither `true` nor `false` is ignored the same way,
/// and keeps the default.
fn parse_options(parameter: &str) -> emit::Options {
    let mut options = emit::Options::default();
    for part in parameter.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key.trim() {
            "proto_module" => value.trim().clone_into(&mut options.proto_module),
            "packaging" => {
                if let Ok(value) = value.trim().parse() {
                    options.packaging = value;
                }
            }
            _ => {}
        }
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_the_proto_module() {
        assert_eq!(parse_options("").proto_module, "crate::proto");
    }

    #[test]
    fn reads_the_proto_module_past_unknown_keys() {
        let options = parse_options("later_option=on,proto_module=crate::buffa,bare_flag");
        assert_eq!(options.proto_module, "crate::buffa");
    }

    #[test]
    fn emits_the_packaging_tree_unless_asked_not_to() {
        assert!(parse_options("").packaging);
        assert!(parse_options("packaging=true").packaging);
        assert!(!parse_options("proto_module=crate::buffa,packaging=false").packaging);
    }

    #[test]
    fn a_packaging_value_that_is_not_a_bool_keeps_the_default() {
        // The same forgiving reading unknown keys get: a `buf.gen.yaml` is not
        // worth failing a build over, and the default is the safe direction --
        // an unused mod.rs, rather than a missing one.
        assert!(parse_options("packaging=no").packaging);
    }
}
