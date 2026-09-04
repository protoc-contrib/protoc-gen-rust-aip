# protoc-gen-rust-aip

A protoc plugin that generates Rust helpers for
[Google AIP](https://google.aip.dev)-shaped APIs.

The Rust counterpart of
[protoc-gen-go-aip](https://github.com/protoc-contrib/protoc-gen-go-aip); its
generated code depends on
[aip-rs](https://github.com/protoc-contrib/aip-rs).

## Status

Nothing implemented yet. The design is settled — this README is the spec.

## What it generates

### Query helpers, per List request

A request is a List request when a service method takes it and returns a
message with a single repeated message field. **That field's type is the
resource**, and its fields are what get exposed. Nothing needs annotating:

```proto
service Library {
  rpc ListBooks(ListBooksRequest) returns (ListBooksResponse);
}

message ListBooksResponse {
  repeated Book books = 1;   // <- Book is the resource
  string next_page_token = 2;
}
```

From that, emit a CEL environment declaring every field of `Book` that has a
CEL type, plus a parser for each dimension the request carries (`filter`,
`order_by`, `page_token`).

Fields with no total order — nested messages other than `Timestamp` and
`Duration`, repeated fields, maps — are **skipped, not rejected**. A field
that is not declared is simply undeclared to the CEL compiler.

Proto enums declare as CEL **int**, which is how a database column stores
them: `genre == 1`, not `genre == "GENRE_FICTION"`.

There is no allow-list in the `.proto` marking which fields are queryable.
That policy lives at the query layer, in the AIP-path to database-column map,
which is fail-closed. A second copy in the schema was tried in the Go
predecessor and removed — it could only drift out of agreement with the one
that is actually enforced.

### Field behavior and field masks — generated, not reflective

This is where the Rust plugin does **more** than the Go one.

`buffa`'s reflection is bridge mode: it encodes the message and decodes it
into a `DynamicMessage`, a full serialize/deserialize round trip per access.
Go's `protoreflect` offers direct field access, so aip-go could afford to walk
messages at runtime. Here, emit the walk instead:

```rust
impl Collection {
    /// Clears every field annotated OUTPUT_ONLY.
    fn clear_output_only(&mut self) { self.create_time = None; /* ... */ }

    /// Errors if a field annotated REQUIRED has no value.
    fn validate_required(&self) -> Result<(), Error> { /* explicit checks */ }
}

impl UpdateCollectionRequest {
    /// Errors if update_mask names a path that is not a field of Collection.
    fn validate_field_mask(&self) -> Result<(), Error> { /* static path match */ }
}
```

Direct field access, no reflection, no round trips. Field-mask validation
becomes a match against a path list the plugin already knows at codegen time,
so it needs no descriptor lookup at runtime either.

Two semantics worth carrying over from aip-go, because both were bugs there
first:

- **Respect explicit presence.** A field with presence — a message, an
  `optional` scalar, a oneof member — is judged present or absent, so an
  explicit `false` satisfies a REQUIRED `optional bool`. A presence-less
  proto3 scalar has no way to distinguish unset from zero, so a REQUIRED one
  must be non-zero.
- **Field-mask coverage matches by path prefix.** A mask of `["carrier"]`
  covers `carrier.name`: replacing a subtree means the whole subtree has to be
  valid.

### Resource names

Emit a concrete type per resource — `BookName { publisher_id, book_id }` —
with a typed `parent()` and parent-to-child builders. Delegate the segment
walking to `aip-rs`'s compiled pattern rather than inlining it, so a fix to
the walk ships as a dependency bump rather than a regeneration of every
consumer.

Keep parsing generated and typed. A runtime parser taking a pattern *string*
gives up the compile-time link between the pattern and its variables, and the
pattern is always known at codegen time.

## Wiring it up

Run it as a **local** plugin. The Rust template already does this for
`protoc-gen-protovalidate-buffa`, which likewise publishes no BSR entry:

```yaml
plugins:
  - local: protoc-gen-rust-aip
    out: src/aip
    opt:
      - proto_module=crate::buffa
```

## License

MIT. See [LICENSE](LICENSE).
