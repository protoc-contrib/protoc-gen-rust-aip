//! Reads a `CodeGeneratorRequest` on stdin, writes a `CodeGeneratorResponse`
//! on stdout. The protoc plugin protocol, and nothing else.

use std::io::{self, Read, Write};

use anyhow::{Result, anyhow, bail};
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
    let options = parse_options(request.parameter.as_deref().unwrap_or_default())?;
    let registry = scan::gather(request, options.allow_unresolved_refs)?;
    let index = messages::gather(request);
    emit::render(request, &registry, &index, &options)
}

/// Parses the comma-separated `key=value` list protoc and buf pass as the
/// plugin parameter.
///
/// Strict, as protoc-gen-go-aip's flags are: a key this plugin does not know
/// -- a typo such as `view=true` -- and a boolean that is neither `true` nor
/// `false` fail generation, naming the option, rather than leaving a default
/// in place that the `buf.gen.yaml` meant to change.
///
/// # Errors
///
/// For an option with no `=`, an unknown key, or a boolean option whose
/// value is not `true` or `false`.
fn parse_options(parameter: &str) -> Result<emit::Options> {
    let mut options = emit::Options::default();
    for part in parameter
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let Some((key, value)) = part.split_once('=') else {
            bail!("option {part:?} has no value: write {part}=<value>");
        };
        let (key, value) = (key.trim(), value.trim());
        let flag = match key {
            "proto_module" => {
                value.clone_into(&mut options.proto_module);
                continue;
            }
            "wildcard_requests" => {
                if value.is_empty() {
                    bail!("option wildcard_requests: no request name");
                }
                options.wildcard_requests.insert(value.to_owned());
                continue;
            }
            "packaging" => &mut options.packaging,
            "views" => &mut options.views,
            "allow_unresolved_refs" => &mut options.allow_unresolved_refs,
            "connect" => &mut options.connect,
            _ => bail!(
                "unknown option {key:?}: want proto_module, packaging, views, \
                 allow_unresolved_refs, connect or wildcard_requests"
            ),
        };
        *flag = value
            .parse()
            .map_err(|_| anyhow!("option {key}: {value:?} is not a boolean, want true or false"))?;
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(parameter: &str) -> emit::Options {
        parse_options(parameter).expect("valid options")
    }

    fn refused(parameter: &str) -> String {
        format!(
            "{:#}",
            parse_options(parameter).expect_err("invalid options")
        )
    }

    #[test]
    fn defaults_the_proto_module() {
        assert_eq!(options("").proto_module, "crate::proto");
    }

    #[test]
    fn reads_the_proto_module() {
        assert_eq!(
            options("proto_module=crate::buffa").proto_module,
            "crate::buffa"
        );
    }

    #[test]
    fn skips_empty_entries_and_whitespace() {
        let options = options(" packaging = false ,, views=true,");
        assert!(!options.packaging);
        assert!(options.views);
    }

    #[test]
    fn emits_the_packaging_tree_unless_asked_not_to() {
        assert!(options("").packaging);
        assert!(options("packaging=true").packaging);
        assert!(!options("proto_module=crate::buffa,packaging=false").packaging);
    }

    #[test]
    fn view_accessors_are_off_unless_asked_for() {
        // Off by default: a view type this schema did not generate is a
        // compile error in the consumer, not a missing method.
        assert!(!options("").views);
        assert!(options("views=true").views);
        assert!(!options("views=false").views);
    }

    #[test]
    fn unresolved_references_fail_unless_allowed() {
        assert!(!options("").allow_unresolved_refs);
        assert!(options("allow_unresolved_refs=true").allow_unresolved_refs);
        assert!(!options("allow_unresolved_refs=false").allow_unresolved_refs);
    }

    #[test]
    fn emits_the_interceptor_impls_unless_asked_not_to() {
        assert!(options("").connect);
        assert!(options("connect=true").connect);
        assert!(!options("connect=false").connect);
    }

    #[test]
    fn every_request_is_strict_unless_listed() {
        assert!(options("").wildcard_requests.is_empty());
    }

    #[test]
    fn wildcard_requests_repeats_once_per_request() {
        let options = options(
            "wildcard_requests=library.v1.ListBooksRequest,views=true,\
             wildcard_requests=library.v1.GetBookRequest",
        );
        assert_eq!(
            options.wildcard_requests.iter().collect::<Vec<_>>(),
            ["library.v1.GetBookRequest", "library.v1.ListBooksRequest"]
        );
    }

    #[test]
    fn a_wildcard_request_needs_a_name() {
        assert_eq!(
            refused("wildcard_requests="),
            "option wildcard_requests: no request name"
        );
    }

    #[test]
    fn an_unknown_option_is_refused_by_name() {
        let error = refused("packaging=false,view=true");
        assert!(error.starts_with(r#"unknown option "view""#), "{error}");
    }

    #[test]
    fn a_boolean_option_that_is_not_a_bool_is_refused() {
        assert_eq!(
            refused("views=yes"),
            r#"option views: "yes" is not a boolean, want true or false"#
        );
        assert_eq!(
            refused("packaging=no"),
            r#"option packaging: "no" is not a boolean, want true or false"#
        );
    }

    #[test]
    fn an_option_without_a_value_is_refused() {
        assert_eq!(
            refused("views"),
            r#"option "views" has no value: write views=<value>"#
        );
    }
}
