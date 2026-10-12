# protoc-gen-rust-aip

[![CI](https://github.com/protoc-contrib/protoc-gen-rust-aip/actions/workflows/ci.yml/badge.svg)](https://github.com/protoc-contrib/protoc-gen-rust-aip/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/protoc-contrib/protoc-gen-rust-aip?include_prereleases)](https://github.com/protoc-contrib/protoc-gen-rust-aip/releases)
[![crates.io](https://img.shields.io/crates/v/protoc-gen-rust-aip?logo=rust)](https://crates.io/crates/protoc-gen-rust-aip)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust (edition 2024)](https://img.shields.io/badge/Rust-2024-black?logo=rust)](https://www.rust-lang.org/)
[![Nix Flake](https://img.shields.io/badge/Nix-Flake-5277C3?logo=nixos&logoColor=white)](https://nixos.wiki/wiki/Flakes)
[![protoc](https://img.shields.io/badge/protoc-compatible-blue)](https://protobuf.dev)

A protoc plugin that generates Rust helpers for
[Google AIP](https://google.aip.dev)-shaped APIs.

The Rust counterpart of
[protoc-gen-go-aip](https://github.com/protoc-contrib/protoc-gen-go-aip); its
generated code depends on
[aip-rs](https://github.com/protoc-contrib/aip-rs).

## Status

Every pass is implemented.

| Pass | Covers |
| --- | --- |
| Resource names | single and multi-pattern, `name_field`, file-scope `resource_definition`, `resource_reference`, UUID-typed segments, typed `parent()` and parent-to-child builders, across packages |
| Create IDs | AIP-133 `{resource}_id`: the proposed ID, or `None` for the server to mint one |
| `impl aip::FieldBehavior`: `MUTABLE_PATHS`, `implied_update_mask`, `changed_immutable_paths`, `missing_required_paths` | AIP-134/203: which fields an update may write, the mask a client implies by omitting one, the `IMMUTABLE` fields an update tries to change, and the `REQUIRED` fields a create or update leaves unset |
| `impl aip::FieldBehavior`: `clear_output_only`, `clear_input_only` | AIP-203 clearing walks, recursive through singular, repeated, map and oneof fields |
| `impl aip::FieldBehavior`: `missing_required_request_paths`, `imply_update_mask` | AIP-133/134/233/235: the resource's check and implied mask applied the way a create, update or batch request asks, reported as `aip::FieldPath`s from the request |
| Filter environment | AIP-160: a CEL `Env` per List request, and `parse_filter` to compile a `filter` in it |

`REQUIRED` on a resource's fields is checked by `missing_required_paths`, not
protovalidate, because the resource is shared by its create and update requests
and protovalidate cannot tell which one it is in — see [Field
behavior](#field-behavior-required-is-checked-here-value-rules-are-protovalidates).

`order_by` and `page_token` are **not** generated: see [List
queries](#list-queries-filter-gets-an-environment-ordering-and-paging-are-the-query-layers).

`tests/fixture/` compiles the schema in `tests/proto` with **both** `buffa` and this
plugin and exercises the result, so the generated code is type-checked against
real message types rather than only diffed against a golden file.

### Dependencies of the generated code

Only what a schema actually uses:

| Crate | Needed when |
| --- | --- |
| [`aip-rs`](https://crates.io/crates/aip-rs) (as `aip`) | always |
| `uuid` | a resource ID is annotated `UUID4` |
| [`cel`](https://crates.io/crates/cel) 0.15 | a List request carries a `filter` |


## What it generates

### List queries: `filter` gets an environment, ordering and paging are the query layer's

A request is a List request when a **unary** method of a service declared in
the request's **own file** takes it and returns a message with a single
repeated message field; that field's type is the resource. The request is a
**top-level** message, and a resource with no field that has a CEL type gets
nothing. Nothing is annotated — this is `protoc-gen-go-aip`'s rule, condition
for condition. For each one with a `string filter` field, the counterpart of
Go's `FilterEnv`:

```rust
/// The CEL environment `filter` expressions on `ListVolumesRequest` compile in.
pub static LIST_VOLUMES_FILTER_ENV: LazyLock<Arc<cel::Env>>;

impl ListVolumesRequest {
    pub fn parse_filter(&self) -> Result<Option<cel::Program>, cel::ParseErrors>;
}
```

The environment is CEL's standard library **without macros or optional
syntax** — `exists`, `has` and the like stay plain calls rather than expanding
to comprehensions, and `a.?b` does not parse; neither has a reading as a query —
with the resource registered as a struct type: every field with a CEL type,
mapped as Go maps them. Integers of every width and signedness are `int`, as
are enums; `Timestamp` and `Duration` are themselves; `bytes`, other messages,
repeated fields and maps are left out.

`parse_filter` returns `None` for a blank filter and otherwise compiles it.
The expression is plain CEL, not AIP-160's CEL-*like* grammar, so
`title = "x" AND published` is a syntax error rather than something misread.

**It checks syntax, and nothing else.** cel-rust has no type checker and no
variable declarations, so the registered fields constrain an
`example.v1.Volume{...}` literal, not the names a filter may use: `title > 3` and `shoe_size == 9` both
compile, where cel-go's checker rejects them. Names, types, and which fields a
client may *actually* filter by are the query layer's — the last through its
fail-closed column map.

The environment is the one a filter evaluates in, too. To evaluate in memory,
bind the row's fields on a context over it:

```rust
let mut context = cel::Context::with_env(Arc::clone(&LIST_VOLUMES_FILTER_ENV));
context.add_variable_from_value("title", volume.title.clone());
program.execute(&context)?;
```

The generated code needs no `cel` features; a filter calling `timestamp(...)`
or `duration(...)` needs `chrono`.

Nothing is generated for `order_by` or `page_token`. How a request sorts is the
columns the query orders by, and a page token is whatever the pager encodes in
it; a parser generated from the `.proto` knows neither, so it could only
disagree with the one that runs — decode a token the server never issued. Parse
them in the query layer — [sqlx-query](https://github.com/sqlx-contrib/sqlx-query)
does, for SQL.

### `MUTABLE_PATHS`, `implied_update_mask`, and what is *not* generated for AIP-134

On each resource, which of its fields an update may write — everything not
`OUTPUT_ONLY`, `IDENTIFIER` or `IMMUTABLE`. The three are one question, not
three: the server owns it, it selects the target rather than being part of it,
or it was settable once, on create.

These, and the clearing walks below, are one trait,
[`aip::FieldBehavior`](https://github.com/protoc-contrib/aip-rs), which the
plugin implements for every message with any of these behaviors at some depth:

```rust
impl aip::FieldBehavior for Shipment {
    const MUTABLE_PATHS: &'static [&'static str] = &["reference", /* ... */];
    fn implied_update_mask(&self) -> Vec<&'static str> { /* ... */ }
    fn changed_immutable_paths(&self, existing: &Self) -> Vec<&'static str> { /* ... */ }
    fn missing_required_paths(&self, mask: Option<&[String]>) -> Vec<&'static str> { /* ... */ }
    fn clear_output_only(&mut self) { /* ... */ }
    fn clear_input_only(&mut self) { /* ... */ }
}
```

An impl holds only the items its message needs; every other item is the
trait's default, the "nothing to do" answer — no writable paths, nothing
missing, nothing changed, nothing to clear. So one server layer over
`T: aip::FieldBehavior` handles every generated message alike, and a message
that only needs a clearing walk (a request, or a message nested in a resource)
still answers every item. The semantics of each item are documented on the
trait, in aip-rs.

Bring the trait into scope where its items are called, as for any trait;
`Shipment::MUTABLE_PATHS` and `Shipment::implied_update_mask` still work as
paths once it is:

```rust
use aip::FieldBehavior;

let paths = Shipment::MUTABLE_PATHS;
let implied = shipment.implied_update_mask();
```

The trait is not dyn-compatible — an associated constant, a method taking
`&Self` — so generic code takes `T: FieldBehavior`, not `dyn FieldBehavior`.

`changed_immutable_paths` is AIP-203's `IMMUTABLE` half: the fields an update
sets to something other than the stored value. Neither expansion ever writes
one, so without it a changed value would be dropped silently rather than
refused; an echoed-back value, or an unset one, is not reported. A repeated
field that is also `UNORDERED_LIST` is compared ignoring order, as a multiset,
since the service does not promise to keep it. aip-go's `ChangedImmutablePaths`
answers the same.

`missing_required_paths` is the `REQUIRED` half; see [Field
behavior](#field-behavior-required-is-checked-here-value-rules-are-protovalidates).

**Stricter than AIP-161, on purpose.** AIP-161 has a mask naming an
`OUTPUT_ONLY` field ignored; here `field_mask.in` rejects it, which is the
clearer error, and protoc-gen-aip-lint's `update-mask-writable-fields` rule
keeps that list equal to the writable fields.

**Validating a mask is not generated, and should not be.** That is
`(buf.validate.field).field_mask.in`, which protovalidate-buffa implements as a
first-class rule:

```proto
google.protobuf.FieldMask update_mask = 2 [
  (buf.validate.field).field_mask.in = ["display_name", "description"]
];
```

It already matches subpaths, already reports as an ordinary violation under rule
id `field_mask.in`, and is already run by `#[protovalidate_buffa::connect_impl]`
along with every other rule on the request. Anything emitted here would be a
second implementation of the same rule, reported differently, that a handler had
to remember to call. An earlier cut of this PR generated exactly that; it was
removed.

What protovalidate cannot do is **expand**. AIP-134 gives a mask two
shorthands, and a server has to turn each into a list of paths to write:

- **`*`**, full replacement, is every writable field: `MUTABLE_PATHS`.
- **An omitted mask** is every writable field the client *populated*:
  `implied_update_mask()`, read off the resource in the request.

```rust
// The client sent no update_mask: write what it populated.
let paths: Vec<&str> = shipment.implied_update_mask();
```

Populated is read as protobuf reads presence — protobuf-go's `Has`, which is
what aip-go's `ImpliedUpdateMask` uses: an `optional` field, a message or a
oneof member when set, even to its zero value; any other scalar when not its
zero value; a repeated field or map when not empty. It is emitted on the owned
message only, not on views: implying a mask means writing it, and a view is
read-only.

Two things follow from presence, and both are worth knowing at the call site:

- **Clear, then imply.** A populated message field is implied as one path —
  `carrier`, not `carrier.name` — so the update writes everything beneath it,
  nested `OUTPUT_ONLY` values included. Call `clear_output_only()` on the
  inbound resource first.
- **A zero value is never implied.** `false`, `0` and `""` on a field without
  explicit presence are indistinguishable from unset, so a client clears one
  with an explicit mask — or the schema makes the field `optional`.

Deriving both from the schema is the point — a new writable field joins them
without anyone remembering to.

Which leaves the `field_mask.in` annotation restating what `field_behavior`
already says. Keeping the two in step is a **lint's** job.

### Create IDs, per AIP-133

```rust
impl CreateCollectionRequest {
    pub fn parse_collection_id(&self) -> Result<Option<uuid::Uuid>, aip::resource::ParseError>;
}

// A server fills the empty case with the UUID it mints:
let collection_id = request.parse_collection_id()?.unwrap_or_else(Uuid::now_v7);
```

The ID the caller proposed, or `None` for an empty `{resource}_id`, which
AIP-133 reads as "the server assigns one". That the ID is a UUID is a schema
fact — the same `google.api.field_info` annotation that types the segment on
`CollectionName` — so parsing it is generated. What the server assigns is not:
a UUIDv7 keeps a primary-key index in insertion order, a v4 reveals nothing
about when a row was made, and that choice belongs to the server, not to the
SDK every client compiles. So the accessor mints nothing, and the call site
says which kind in one `unwrap_or_else`.

Only for a single-pattern resource with a UUID-typed own ID. A `string` ID has
no minting rule the schema states, and a multi-pattern resource's create request
does not say which pattern it is creating under, so neither gets an accessor
rather than getting a guess. A proposed nil UUID is refused too: it parses,
but no name built from it validates. A failure is reported as the same
`ParseError` an unparseable segment produces when reading a whole name, so a
call site handles one error type either way.

Needs no `uuid` feature beyond its defaults: the generated code only parses.
A server needs whichever minting feature it calls — `v7` for `now_v7`.

### Field behavior: `REQUIRED` is checked here, value rules are protovalidate's

Two walks clear what one side may not see: `clear_output_only` drops
server-owned values from an inbound request, and `clear_input_only` drops
values a client may send but must never get back from an outbound response.
Each is emitted only for messages with such a field at some depth.

A walk reaches only messages this run generates. A generated message holding
one from a file that is *not* generated, with `OUTPUT_ONLY` (or `INPUT_ONLY`)
fields beneath it, fails generation, naming both — those fields could not be
cleared, and for `OUTPUT_ONLY` an implied mask naming the holding field would
store them. Generate that file too.

The walks are generated, not reflective — and not as an
optimisation. buffa 0.9 has **no reflective path to mutation in any mode**:
`Reflectable` exposes only `reflect(&self)`, and the source marks
`reflect_mut` as designed but deferred to the MergeSink work.
`ReflectMessage::clear()` exists, but reaching it needs a
`&mut dyn ReflectMessage` that nothing produces.

```rust
impl aip::FieldBehavior for Shipment {
    fn clear_output_only(&mut self) {
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
`clear_input_only` is the same walk for `INPUT_ONLY`.

Revisit the split if a buffa release lands `reflect_mut`.

**`REQUIRED` on a resource is checked here, not by protovalidate.** AIP-203
gives a `REQUIRED` resource field two meanings: a create must set it, and a
partial update may omit it unless its mask names it — then it must be set, or
the update would clear it. A resource message is shared by both requests, and a
`buf.validate` rule on it cannot tell which one it is in. Marking the field
`(buf.validate.field).required` breaks every partial update that leaves it out;
the workaround is a CEL rule restated on each create and update request, which
is a second, hand-written copy of what `field_behavior` already says and which
no lint can practically keep in step.

So the split is by what protovalidate can see:

- **protovalidate** keeps a request's own fields — `parent`, the resource
  field itself, `update_mask` — and value rules on the resource's fields:
  lengths, patterns, ranges. A value rule on a `REQUIRED` field needs
  `ignore = IGNORE_IF_ZERO_VALUE`, so an update that leaves it out passes.
- **`missing_required_paths`** reads `REQUIRED` off the resource, with the mask
  the request carries:

```rust
use aip::FieldBehavior;

// create: every REQUIRED field
let missing = item.missing_required_paths(None);
// update, after the implied mask is filled in
let missing = item.missing_required_paths(Some(&update_mask.paths));
if !missing.is_empty() { /* InvalidArgument, naming each path */ }
```

`Some(paths)` checks only the fields the mask names. A path naming a message
checks everything beneath it; a nested path, just that field. Expand `*` with
`MUTABLE_PATHS` first. The parameter is the owned `FieldMask`'s `paths` as it
is, so a request's mask is passed without conversion, and `None` needs no type
annotation.

Nested messages follow AIP-203's rule that a nested message's annotations are
independent of its parent's: a set singular message is descended into, and its
own `REQUIRED` fields are reported as dotted paths (`destination.city`). An
unset one is not entered on a create, or when the mask names it whole — its own
`REQUIRED` is reported at its level. A nested path is checked even when its
parent is unset, reading the parent as empty: `update_mask: destination.city`
with no `destination` writes `destination.city` as empty, so it would clear a
`REQUIRED` field and is reported. Repeated and map fields are present when not
empty and are not descended into. A message type that contains itself is entered once per
path, since every path is a `&'static str` spelled out at generation time.

Presence is read as protobuf reads it, the rule carried over from aip-go
because it was a bug there first: a field with presence — a message, an
`optional` scalar, a oneof member — is judged set or unset, so an explicit
`false` satisfies a `REQUIRED` `optional bool`. A presence-less proto3 scalar
cannot tell unset from zero, so a `REQUIRED` one must be non-zero, and a
`REQUIRED` `bool` must be `true`.

A `REQUIRED` field in a oneof of a message from *another* package fails
generation: the check names the oneof's enum by its path within the package,
which does not reach another package's.

aip-go's `MissingRequiredPaths` answers the same, by reflection.

#### Per request: `missing_required_request_paths` and `imply_update_mask`

A server holds a request, not a resource, so the two are also generated on
every service method's create, update and batch request — and on a batch's
entry type — applying the resource's items the way the method asks:

| Request | Recognized by | `missing_required_request_paths` | `imply_update_mask` |
| --- | --- | --- | --- |
| create (AIP-133) | a singular field of a resource type, no `update_mask` | `resource.missing_required_paths(None)` | — |
| update (AIP-134) | that, plus a `google.protobuf.FieldMask update_mask` | `resource.missing_required_paths(Some(mask))`; an unset mask names nothing, never a create | fills an unset or empty mask from `resource.implied_update_mask()` |
| batch (AIP-233/235) | a repeated `requests` of creates or updates | each entry's, under `requests[i]` | each update entry's |

```rust
impl aip::FieldBehavior for UpdateOrderRequest {
    fn missing_required_request_paths(&self) -> Vec<aip::FieldPath> { /* ... */ }
    fn imply_update_mask(&mut self) { /* ... */ }
}
```

Each path is an `aip::FieldPath` from the request — `requests[1].order.title` —
carrying field names only; a server that reports a protovalidate violation
resolves numbers and types from its descriptor pool. With the trait's
"nothing to do" defaults on every other message, one interceptor covers every
request:

```rust
use aip::FieldBehavior;

request.clear_output_only();
request.imply_update_mask();
for path in request.missing_required_request_paths() {
    // a `required` violation at `path`
}
```

The calls go through the resource's own impl, and a batch's through its entry
type's, so their files have to be generated in the same run; generation fails
otherwise. Every resource gets an impl, an empty one if nothing in it differs
from the defaults. aip-go's `MissingRequiredRequestPaths` and `ImplyUpdateMask`
answer the same, by reflection.

What *is* still worth a lint is the inverse of the old one: a `REQUIRED` field
on a resource that also carries `(buf.validate.field).required` is one every
partial update that omits it is rejected for. That belongs in
[protoc-gen-aip-lint](https://github.com/protoc-contrib/protoc-gen-aip-lint).

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
fails is reported as `ParseError`, the same error as any other name that does not
match the pattern, rather than as a second error type at the call site.

No `Format<T>Name` / `Parse<T>ID` free functions are emitted. They were Go's,
for goverter's `extend` directive, which has no Rust counterpart; a struct
literal does the job here.

## Wiring it up

Install it from crates.io, or take `packages.default` from this repository's
flake:

```sh
cargo install protoc-gen-rust-aip
```

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

### Views: `views=true`

buffa's two-tier model gives every message a borrowed `FooView<'a>`, and a
connectrpc handler is passed a `ServiceRequest` that derefs to *that*, not to
the owned message. So an accessor emitted only on `Foo` is one a handler cannot
reach:

```rust
async fn create_item(&self, ctx: RequestContext, request: ServiceRequest<'_, CreateItemRequest>) {
    let parent = request.parse_parent()?;   // needs views=true
}
```

With `views=true` every read-only accessor — `parse_name`, `parse_full_name`,
`parse_<reference>`, `parse_<resource>_id`, `parse_filter` — is emitted
for `FooView<'_>` as well. The bodies are token-identical: a view's field holds
`&str` where the owned message holds `String`, and every operation these
accessors perform is spelled the same for both.

`aip::FieldBehavior` is not implemented for views. Its clearing walks take
`&mut self`, and a view does not own what they would clear; implying a mask
means writing it onto the request, which is the owned message's job.

Off by default, and it has to be: nothing in a `CodeGeneratorRequest` says
whether buffa generated views, and emitting `impl FooView<'_>` when it did not
is a compile error in the consumer rather than a missing method.

`MUTABLE_PATHS` needs no view either — it is an associated constant on the
resource, so there is nothing for a view to borrow.

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

### Unresolved references: `allow_unresolved_refs=true`

A `google.api.resource_reference` names its referent by type string. If the
request declares no resource of that type, generation fails, naming the field:

```text
example.v1.Shelf.owner: reference to unknown type "people.example.com/Person" (set allow_unresolved_refs=true to skip)
```

The schema says the field holds that resource's name, so skipping it would drop
the field's `parse_<field>` with nothing to say so. When the referent belongs to
another API whose `.proto` is deliberately not compiled, skip it:

```yaml
    opt:
      - allow_unresolved_refs=true
```

The same option, and the same default, as `protoc-gen-go-aip`. Only files being
generated are held to it: an imported file may name a resource type it never
imports, and nothing is emitted for it either way. References this plugin has
nothing to emit for are skipped regardless — `child_type`, `type: "*"`, and a
field that is not a singular `string`.

A reference whose accessor would collide with the resource name's own fails
generation regardless of the option: on a resource message, a reference on the
`name` field itself, or on a field named `full_name`, would emit a second
`parse_name` or `parse_full_name`. protoc-gen-go-aip refuses the equivalent
`ParseName`/`ParseFullName` collision.

**Versions of one API.** `v1` and `v2` may each declare `example.com/Book`. A
reference, a parent and a create request's ID format all bind to the
declaration in the referring package; from another package, to the only
declaration there is. A reference from a package that declares none, to a type
several others do, fails as ambiguous (or is skipped, with
`allow_unresolved_refs`). Two declarations of one type in one package fail.

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

`src/generated` is buffa's output for the vendored `google/api` annotations in
`proto/`, committed so installing the plugin needs no `protoc`. A test fails if
it drifts from a fresh generation. After changing a proto or the buffa
version, regenerate it with:

```bash
UPDATE_GENERATED=1 cargo test --test generated
```

## License

MIT. See [LICENSE](LICENSE).
