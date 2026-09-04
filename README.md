# protoc-gen-rust-aip

A protoc plugin that generates Rust helpers for
[Google AIP](https://google.aip.dev)-shaped APIs.

The Rust counterpart of
[protoc-gen-go-aip](https://github.com/protoc-contrib/protoc-gen-go-aip); its
generated code depends on
[aip-rs](https://github.com/protoc-contrib/aip-rs).

## Status

All three passes are implemented.

| Pass | Covers |
| --- | --- |
| Resource names | single and multi-pattern, `name_field`, file-scope `resource_definition`, `resource_reference`, UUID-typed segments, typed `parent()` and parent-to-child builders, across packages |
| Query helpers | `filter`, `order_by` and `page_token` per List request, plus the combined `parse_query` |
| `OUTPUT_ONLY` clearing walk | recursive through singular, repeated, map and oneof fields |

Still missing, and tracked in [Field
behavior](#field-behavior-clearing-is-generated-validating-need-not-be): the
**read-only** halves of AIP-203 and AIP-134 — validating `REQUIRED` fields and
checking `update_mask` paths. Those need no mutation, so they belong in
`aip-rs` rather than here, and `aip-rs` does not have them yet.

**One deviation from the Go implementation, forced by the ecosystem:**
`cel-rust` has no type checker, so a `filter` is checked by *reference* — every
name it uses must be a declared field — rather than type-checked the way
`cel-go` does for `protoc-gen-go-aip`. `title == 5` compiles here and fails at
the query layer. See [Query helpers](#query-helpers-per-list-request).

`tests/fixture/` compiles the schema in `tests/proto` with **both** `buffa` and this
plugin and exercises the result, so the generated code is type-checked against
real message types rather than only diffed against a golden file.

### Dependencies of the generated code

Only what a schema actually uses:

| Crate | Needed when |
| --- | --- |
| [`aip-rs`](https://github.com/protoc-contrib/aip-rs) (as `aip`) | always |
| `buffa` | a List request has a `page_token` — the checksum marshals the request |
| `cel` | a List request has a `filter` |
| `uuid` | a resource ID is annotated `UUID4` |


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

From that it emits, on the request:

```rust
impl ListBooksRequest {
    pub const QUERY_FIELDS: &'static [&'static str];

    pub fn parse_filter(&self) -> Result<Option<cel::Program>, aip::query::FilterError>;
    pub fn parse_order_by(&self) -> Result<aip::OrderBy, aip::QueryError>;
    pub fn parse_page_token(&self) -> Result<aip::PageToken, aip::pagination::ParseError>;
    pub fn checksum(&self) -> u32;

    pub fn parse_query(&self) -> Result<ListBooksQuery, aip::QueryError>;
}
```

Only the dimensions the request actually declares get a parser, and
`ListBooksQuery` has one field per dimension — a request with just `filter`
gets just `filter`.

`QUERY_FIELDS` is every field of `Book` with a CEL type. Fields with no total
order — nested messages other than `Timestamp` and `Duration`, repeated
fields, maps — are **skipped, not rejected**. A field that is not declared is
simply undeclared.

Proto enums declare as CEL **int**, which is how a database column stores
them: `genre == 1`, not `genre == "GENRE_FICTION"`.

There is no allow-list in the `.proto` marking which fields are queryable.
That policy lives at the query layer, in the AIP-path to database-column map,
which is fail-closed. A second copy in the schema was tried in the Go
predecessor and removed — it could only drift out of agreement with the one
that is actually enforced.

#### Filters are reference-checked, not type-checked

The Go implementation compiles a filter against a `cel.Env` declaring each
field's type, so `cel-go`'s checker rejects `title == 5` at the boundary.
**`cel-rust` has no checker** — `Program::compile` parses. So a filter is
parsed, and then every name it references is checked against `QUERY_FIELDS`:

```rust
list.filter = r#"shoe_size == 9"#.into();   // FilterError::Undeclared
list.filter = r#"title = "x" AND y"#.into(); // FilterError::Syntax — AIP-160, not CEL
list.filter = r#"title == 5"#.into();        // parses; fails at the query layer
```

Undeclared names and the old AIP-160 grammar are caught. Type errors are not,
and reach whatever builds the `WHERE` clause. Revisit if `cel-rust` grows a
checker.

#### The page-token checksum

`checksum` clones the request, clears `page_token`, `page_size` and
`skip`, and marshals it — the AIP-158 rule, which is why the generated code
depends on `buffa`. A mismatch means the client changed `filter` or `order_by`
mid-page.

One caveat carried from the specification: marshalling must be
**deterministic**. buffa encodes a map field in `HashMap` order, so a List
request carrying a map produces an unstable checksum and rejects every token.
The generated doc comment says so on any request where it applies; configure
that field as a `BTreeMap` in the buffa codegen.

### Field behavior: clearing is generated, validating need not be

The OUTPUT_ONLY walk is generated, not reflective — and not as an
optimisation. buffa 0.9 has **no reflective path to mutation in any mode**:
`Reflectable` exposes only `reflect(&self)`, and the source marks
`reflect_mut` as designed but deferred to the MergeSink work.
`ReflectMessage::clear()` exists, but reaching it needs a
`&mut dyn ReflectMessage` that nothing produces.

```rust
impl Shipment {
    /// Clears every field annotated `OUTPUT_ONLY`, at any depth.
    pub fn clear_output_only(&mut self) {
        self.tracking_id = Default::default();
        for item in &mut self.parcels { item.clear_output_only(); }
        // ...
    }
}
```

The walk is recursive, through singular, repeated and map-valued messages. A
message with nothing output-only beneath it gets no walk at all and is never
descended into. A `oneof` is cleared entirely when the member that is set is
the output-only one, since there is no individual field to assign to.

Reads are a genuine choice, and are **not built yet on either side**.
Validating REQUIRED fields and checking update_mask paths are read-only, so
aip-rs can do them reflectively the way aip-go does. Generating them is also reasonable — field-mask validation
becomes a match against a path list known at codegen time, needing no
descriptor lookup at all — but it is a size-versus-runtime tradeoff rather
than a constraint. Prefer the runtime unless it measures badly, so the two
implementations stay structurally comparable.

Revisit the whole split if a buffa release lands `reflect_mut`.

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

```rust
let book = BookName::parse("publishers/p1/books/b1")?;
let publisher: PublisherName = book.parent();
assert_eq!(publisher.book_name("b2").to_string(), "publishers/p1/books/b2");
```

A resource with more than one pattern becomes an **enum** over one struct per
pattern. Go modelled this as a sealed interface; the enum is the closer fit,
because "which parent is this name under?" is exactly a match:

```rust
match BookName::parse(name)? {
    BookName::PublisherBook(book) => book.parent(), // -> PublisherName
    BookName::AuthorBook(book) => todo!(),
}
```

There is no annotation recording that `Publisher` is `Book`'s parent — AIP-122
does not have one. The only evidence is that some resource declares exactly the
pattern `Book`'s is nested in, so that is what the generator matches on, across
package boundaries. A parent that is not in the request simply yields no
`parent()`, rather than an error.

Cross-package references are emitted as paths relative to the root of the
generated module tree, so they resolve wherever a consumer mounts it. Nothing
has to be configured, and so nothing can be misconfigured.

#### A segment is typed by the request that mints it

A segment is a `String` unless the schema says otherwise, and the only place a
schema says otherwise is AIP-133's create request:

```proto
message CreateItemRequest {
  string parent = 1;
  Item item = 2;
  string item_id = 3 [(google.api.field_info).format = UUID4];
}
```

`ItemName.item_id` is then a `uuid::Uuid`, not a `String`, and the builder takes
one:

```rust
let item = organization.item_name(id);   // id: uuid::Uuid
```

Only a schema that annotates an ID needs the `uuid` crate — nothing else in the
output names it.

A **child inherits its parent's** segment formats, because the parent's create
request is what mints those IDs. `Item`'s `{organization}` is typed by
`CreateOrganizationRequest.organization_id`, never by `CreateItemRequest`. That
is what makes `parent()` able to hand the value straight over instead of
bridging through a string — and it is why a child and its parent can never
disagree about a segment's type.

A UUID segment can't hold the AIP-159 wildcard, since `-` is not a UUID;
`contains_wildcard()` only ever consults the string segments. A conversion that
fails is reported as `ScanError`, the same error as any other name that does not
match the pattern, rather than as a second error type at the call site.

Unlike the Go predecessor, no `Format<T>Name` / `Parse<T>ID` free functions are
emitted. Those existed for goverter's `extend` directive, which has no Rust
counterpart; a struct literal does the job here.

## Wiring it up

Run it as a **local** plugin. The Rust template already does this for
`protoc-gen-protovalidate-buffa`, which likewise publishes no BSR entry:

```yaml
plugins:
  - local: protoc-gen-rust-aip
    out: src/aip
    strategy: all
    opt:
      # Where the buffa-generated message types live. Each leaf of the emitted
      # module tree brings the matching package into scope from here, so an
      # accessor emitted for a message can name it. Defaults to crate::proto.
      - proto_module=crate::buffa
```

`strategy: all` is required, not cosmetic. The plugin emits one `mod.rs`
mounting every package it generated for; under buf's default per-directory
strategy each invocation would write a `mod.rs` covering only its own
directory, and the last one would win. It also matters for
`resource_reference`, which names its referent by *type string* and so can
point at a resource in a file the referrer never imports.

Mount the output with either `#[path]` or `include!` — the generated `mod.rs`
carries no inner attributes, so both work:

```rust
pub mod aip {
    include!("aip/mod.rs");
}
```

## License

MIT. See [LICENSE](LICENSE).
