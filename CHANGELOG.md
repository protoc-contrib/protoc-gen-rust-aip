# Changelog

## [0.2.1](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.2.0...v0.2.1) (2026-10-10)


### Bug Fixes

* read proto2 and editions explicit presence as buffa does ([#26](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/26)) ([40d0f4b](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/40d0f4b1aba1f71801151f2e0bba190c8086649a))
* resolve resources per package, and close the review's other gaps ([#28](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/28)) ([6c66574](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/6c66574b616d6f94b84eea432be6dc2ba0fcf01f))

## [0.2.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.1.0...v0.2.0) (2026-10-10)


### ⚠ BREAKING CHANGES

* a schema with a resource_reference to a type the request does not declare now fails to generate; set allow_unresolved_refs=true to keep skipping it.

### Features

* fail on a resource_reference to an unknown type ([#23](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/23)) ([e585787](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/e58578722f20d80d2d6001af1183a10869ed85ef))
* generate a CEL filter environment for List requests ([#21](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/21)) ([2a0d6f8](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/2a0d6f8ae3f612e16bc8935a7568d81f66b3038d))
* generate implied_update_mask on each resource ([#25](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/25)) ([6136577](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/61365776c45bbe057af9f168e85a6dcb16b2016c))


### Bug Fixes

* refuse a proposed nil UUID in the create-ID accessor ([#24](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/24)) ([0ebb3fc](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/0ebb3fcf9eceeac4c16ea67f4193f99bf2717d66))

## [0.1.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.1.0...v0.1.0) (2026-09-25)


### ⚠ BREAKING CHANGES

* the generated List request helpers -- QUERY_FIELDS, parse_filter, parse_order_by, parse_page_token, parse_query and the List<Resource>Query struct -- are gone; parse those fields in the query layer.

### Features

* name the resource-name error ParseError in generated code ([#12](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/12)) ([d107b2a](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/d107b2ace9bc890da400e0664116de3f8cc8fc76))
* parse a create request's proposed ID, and leave minting to the server ([#11](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/11)) ([37f30ed](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/37f30ed9dbc1b94c6e0b0a70e2d59dd865dd7710))
* per-package output, optional packaging and views, AIP-133 and MUTABLE_PATHS ([a31a907](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/a31a90721a56d26f1638e9e0b7c62c1735943a20))
* stop generating List query parsers ([#10](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/10)) ([a95bef3](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/a95bef3c196d3e2bba5c037b61fe7551126112be)), closes [#9](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/9)
* validate resource patterns with the aip-rs runtime ([#13](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/13)) ([bfcdef0](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/bfcdef0e2ccd459a6d1c3c2af5253b62a00da30f))
