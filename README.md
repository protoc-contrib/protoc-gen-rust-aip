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
| Create IDs | AIP-133 `{resource}_id`: the proposed ID, or a minted one |
| Query helpers | `filter`, `order_by` and `page_token` per List request, plus the combined `parse_query` |
| `update_mask` | AIP-134: which paths a resource allows, and the check against them |
| `OUTPUT_ONLY` clearing walk | recursive through singular, repeated, map and oneof fields |

`REQUIRED` is **not** generated, deliberately — see [Field
behavior](#field-behavior-clearing-is-generated-validating-is-protovalidates).

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

### The `update_mask` check

A request is an update request when it carries a `google.protobuf.FieldMask`
called `update_mask` **and exactly one other message field whose type is a
declared resource**. Recognised by shape, like a List request — a message with
the shape and not the AIP-134 name still works, and one with the name and not
the shape is not half-supported.

On the resource, which fields an update may write, read off
`google.api.field_behavior`: everything not `OUTPUT_ONLY`, `IDENTIFIER` or
`IMMUTABLE`. The three are one question — the server owns it, it selects the
target rather than being part of it, or it was settable once on create.

```rust
impl Shipment {
    pub const MUTABLE_PATHS: &'static [&'static str];   // top-level, = an empty mask
    pub fn is_mutable_path(path: &str) -> bool;         // at any depth
}

impl UpdateShipmentRequest {
    pub fn validate_update_mask(&self) -> Result<(), aip::field_mask::FieldMaskError>;
}
```

A nested path is **walked, not looked up**. Enumerating every dotted path a
mask could name is unbounded the moment a schema has a message that can reach
itself; recursing on the path the client sent is finite by construction, so a
cyclic schema costs nothing and there is no depth limit to tune.

`carrier.name` resolves when `carrier` is a writable message field and `name`
is writable on `Carrier`. Naming `carrier` alone is writable too — replacing a
subtree is writing it. A repeated or map field is writable as a whole, but is
not a way down: an AIP-134 mask addresses fields, not entries.

A checker is emitted for each updated resource and for every message a mask
path can reach from one, so `Address` gets one by virtue of being what
`Shipment.origin` holds, without declaring a resource or having an update
request of its own.

### Create IDs, per AIP-133

```rust
impl CreateCollectionRequest {
    pub fn collection_id_or_new(&self) -> Result<uuid::Uuid, aip::resource::ScanError>;
}
```

An empty `{resource}_id` means the server assigns one. That the ID is a UUID is
a schema fact — the same `google.api.field_info` annotation that types the
segment on `CollectionName`; that an empty one means "mint one" is an AIP fact,
identical on every such request, and every consumer was writing it out by hand.

Only for a single-pattern resource with a UUID-typed own ID. A `string` ID has
no minting rule the schema states, and a multi-pattern resource's create request
does not say which pattern it is creating under, so neither gets an accessor
rather than getting a guess. A failure is reported as the same `ScanError` an
unparseable segment produces when reading a whole name, so a call site handles
one error type either way.

Needs the `uuid` crate's `v4` feature, which is the only feature this plugin's
output requires beyond a crate's defaults.

### Field behavior: clearing is generated, validating is protovalidate's

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

Revisit the split if a buffa release lands `reflect_mut`.

**`REQUIRED` is not generated here, and should not be.** A schema that marks a
field `(google.api.field_behavior) = REQUIRED` almost always also constrains it
with `(buf.validate.field).required` — or a `min_len`, for a presence-less
scalar — and a server running protovalidate already enforces that. A second
check generated from the AIP annotation could only drift out of agreement with
the one that actually runs, which is the same argument this plugin makes
against [an allow-list in the `.proto`](#query-helpers-per-list-request).

What *is* worth having is a lint that the two annotations agree: a field marked
REQUIRED with nothing in `buf.validate` enforcing it is a field the schema
claims is required and no server checks. That belongs in
[protoc-gen-aip-lint](https://github.com/protoc-contrib/protoc-gen-aip-lint),
not here — a linter reports, a generator emits.

The presence rule such a lint has to respect, carried over from aip-go because
it was a bug there first: a field with presence — a message, an `optional`
scalar, a oneof member — is judged present or absent, so an explicit `false`
satisfies a REQUIRED `optional bool`. A presence-less proto3 scalar cannot tell
unset from zero, so a REQUIRED one needs a rule that rejects the zero value.

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

`strategy: all` is required, not cosmetic. Output is one
`<package>.aip.rs` per proto *package*, mirroring `protoc-gen-buffa` under
`file_per_package=true`; under buf's default per-directory strategy a package
spread over two directories would be generated twice, each invocation seeing
half of it, and the second write would win. It also matters for
`resource_reference`, which names its referent by *type string* and so can
point at a resource in a file the referrer never imports — and for the single
`mod.rs`, which has to mount every package at once.

Mount the output with either `#[path]` or `include!` — the generated `mod.rs`
carries no inner attributes, so both work:

```rust
pub mod aip {
    include!("aip/mod.rs");
}
```

### Or skip the packaging output: `packaging=false`

The tree above is a parallel `example::v1` next to the buffa one. A consumer
can instead put the resource names *beside* the resources, by writing the
output into the same directory as the buffa codegen and including
`<package>.aip.rs` into the module that already holds the message types:

```yaml
  - local: protoc-gen-rust-aip
    out: src/buffa          # alongside protoc-gen-buffa's own output
    strategy: all
    opt:
      - packaging=false
```

```rust
pub mod v1 {
    include!("example.v1.rs");        // protoc-gen-buffa
    include!("example.v1.aip.rs");    // this plugin
}
```

With `packaging=false` no `mod.rs` is emitted at all. That is not only
tidiness: a `mod.rs` written into a directory that already has a hand-written
one **replaces it**, and `buf generate` exits 0 without mentioning it.

`proto_module` is then unread — it only ever appears in the module tree — so
there is no second place for the consumer's layout to be described.

Two constraints come with it:

- **Single package.** A `<package>.aip.rs` names a same-package type by its
  short name, but reaches another package with `super` hops counted from the
  root of the tree that is no longer emitted. A multi-package schema has to
  mount the tree.
- **The file keeps its `use super::*`.** Harmless where the types are already
  in scope; it is what makes the file work when the enclosing module brings
  them in with a `use` instead.

## Development

The toolchain is pinned by `rust-toolchain.toml` and provided by the flake:

```bash
nix develop          # cargo, rustc, clippy, rustfmt, protoc
cargo test --workspace
```

Without Nix on the host, `.devcontainer/` supplies it inside the container —
open the repo in a devcontainer and the same `nix develop` works.

`cargo test --workspace` regenerates the schema in `tests/proto` through both
`buffa` and this plugin, so a change to an emitter is compiled and exercised
rather than only diffed.

## License

MIT. See [LICENSE](LICENSE).
