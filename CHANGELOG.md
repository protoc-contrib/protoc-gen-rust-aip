# Changelog

## [0.1.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.1.0...v0.1.0) (2026-09-25)


### ⚠ BREAKING CHANGES

* the generated List request helpers -- QUERY_FIELDS, parse_filter, parse_order_by, parse_page_token, parse_query and the List<Resource>Query struct -- are gone; parse those fields in the query layer.

### Features

* name the resource-name error ParseError in generated code ([#12](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/12)) ([d107b2a](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/d107b2ace9bc890da400e0664116de3f8cc8fc76))
* parse a create request's proposed ID, and leave minting to the server ([#11](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/11)) ([37f30ed](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/37f30ed9dbc1b94c6e0b0a70e2d59dd865dd7710))
* per-package output, optional packaging and views, AIP-133 and MUTABLE_PATHS ([a31a907](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/a31a90721a56d26f1638e9e0b7c62c1735943a20))
* stop generating List query parsers ([#10](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/10)) ([a95bef3](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/a95bef3c196d3e2bba5c037b61fe7551126112be)), closes [#9](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/9)
* validate resource patterns with the aip-rs runtime ([#13](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/13)) ([bfcdef0](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/bfcdef0e2ccd459a6d1c3c2af5253b62a00da30f))
