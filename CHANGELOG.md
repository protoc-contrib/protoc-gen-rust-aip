# Changelog

## [0.12.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.11.0...v0.12.0) (2026-10-19)


### Features

* implement aip::Fields for every message, so a field is named by its name ([#72](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/72)) ([d6997ef](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/d6997ef277a897c9e859b134c751d8d843b3f1e5))

## [0.11.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.10.0...v0.11.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* parse_filter names the filter on its error, as Go's ParseFilter does ([#67](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/67))

### Features

* parse_filter names the filter on its error, as Go's ParseFilter does ([#67](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/67)) ([ba4b92d](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/ba4b92d1aa1ee89298920828388be48b8ff840a0))

## [0.10.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.9.0...v0.10.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* parse a field into its name with the field named on the error ([#64](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/64))

### Features

* parse a field into its name with the field named on the error ([#64](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/64)) ([c16a26d](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/c16a26d17781a5330e43eea27b47938142458849))

## [0.9.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.8.0...v0.9.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* generated code needs aip-rs with Procedure::new and Procedure::clearing (0.9.0), and every request but a well-known type, streaming ones included, must implement protovalidate-buffa's Validate.

### Features

* name every procedure Procedure::new, a well-known request Procedure::clearing ([#61](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/61)) ([1655a05](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/1655a0577a2e2f177eb87b26347e1f5a2bdd0b43))

## [0.8.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.7.0...v0.8.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* strict plugin options, canonical lowercase UUIDs, and create-id errors that name the field ([#58](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/58))

### Bug Fixes

* strict plugin options, canonical lowercase UUIDs, and create-id errors that name the field ([#58](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/58)) ([8fecebc](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/8fecebcd0a4e7b71c07d1642d078657d7da8134b))

## [0.7.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.6.1...v0.7.0) (2026-10-14)


### Features

* emit parse_filter on request views too ([#54](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/54)) ([f6aa93e](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/f6aa93ec09a5999d27c6ac3e1ab5a1e5d566b6e5))

## [0.6.1](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.6.0...v0.6.1) (2026-10-14)


### Bug Fixes

* keep generated code clippy-clean on current stable ([#50](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/50)) ([edc8cd0](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/edc8cd027d07ead6150a214292678395f7913abb))

## [0.6.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.5.0...v0.6.0) (2026-10-14)


### ⚠ BREAKING CHANGES

* the generated FieldBehavior impls no longer have MUTABLE_PATHS, implied_update_mask, validate_required_fields, validate_immutable, validate_required, imply_resource_name, imply_update_mask, clear_output_only or clear_input_only. Use clear_fields(kind), validate_fields(kind, scope) and Resource::implied_field_mask.

### Features

* generate clear_fields and validate_fields by kind, and aip::Resource ([#46](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/46)) ([26c15b6](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/26c15b6f49d0158c5baddd8be02c7f7d69cf0865))
* generate ResourceWrites and PROCEDURES for the interceptor ([#47](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/47)) ([263befd](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/263befd0ecef0a548a2f91497c5958ba7be260a8))

## [0.5.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.4.0...v0.5.0) (2026-10-12)


### ⚠ BREAKING CHANGES

* the generated missing_required_paths, changed_immutable_paths and missing_required_request_paths are replaced by validate_required_fields, validate_immutable and validate_required, which return Results; requires aip-rs with those trait items.

### Features

* generate validate_* and imply_resource_name, and an impl for every message ([#43](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/43)) ([05ebf37](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/05ebf37abe18e8baaaed495253903519be8bf7e9))

## [0.4.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.3.0...v0.4.0) (2026-10-12)


### ⚠ BREAKING CHANGES

* the field-behavior methods and MUTABLE_PATHS move from inherent items to the aip::FieldBehavior trait (use aip::FieldBehavior at call sites), and immutable_changes is renamed to changed_immutable_paths.

### Features

* generate missing_required_request_paths and imply_update_mask on requests ([#41](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/41)) ([3e01383](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/3e01383f0d56e4c3faff5889e290925dd64a88ee))
* implement aip::FieldBehavior with REQUIRED, INPUT_ONLY and UNORDERED_LIST ([#39](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/39)) ([74fc680](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/74fc680a703a4fdc762013d72ab5d398563c9f9c))

## [0.3.0](https://github.com/protoc-contrib/protoc-gen-rust-aip/compare/v0.2.1...v0.3.0) (2026-10-12)


### Features

* build without protoc by committing the google.api types ([#31](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/31)) ([eef4164](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/eef416413cdbf65917764cc97d5b7d8715d558ed))


### Bug Fixes

* refuse a reference whose accessor collides with the resource name's ([#32](https://github.com/protoc-contrib/protoc-gen-rust-aip/issues/32)) ([c084241](https://github.com/protoc-contrib/protoc-gen-rust-aip/commit/c084241ffe98c12f3621fa23e73fc14edf3dc690))

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
