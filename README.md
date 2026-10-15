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
| `impl aip::FieldBehavior` on every message: `clear_fields(kind)`, `validate_fields(kind, scope)` | AIP-203: unset the fields of a behavior, recursive through singular, repeated, map and oneof fields; report those of them a `FieldScope` contains that are unset, recursive through singular and oneof fields. One arm per kind a message carries; an empty impl, the trait's "nothing to do" defaults, when it carries none |
| `impl aip::Resource` on every resource: `type Name`, `parse_name`, `implied_field_mask` | AIP-122/134: the typed name, and the field mask an update of the resource implies when it omits one |
| Filter environment | AIP-160: a CEL `Env` per List request, and `parse_filter` to compile a `filter` in it |
| `impl aip::ResourceWrites` on every request, a `PROCEDURES` table per package (`connect=true`, the default) | AIP-133/134/233/234: the resources a create, update or batch request writes, and each method's message types, which aip-rs's `FieldBehaviorInterceptor` is driven by |

Which operation a service applies, with which kind, to which message of a
request — `REQUIRED` checked in full on a create and against the mask on an
update, say — is the service's own policy, built on these impls. aip-rs's
`FieldBehaviorInterceptor` is one such policy, AIP's, which the `connect`
output wires up: see [The interceptor's inputs](#the-interceptors-inputs-connecttrue).
See also [Field
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
| `buffa-types` | a resource is declared on a message (`implied_field_mask` returns its `FieldMask`) |
| `uuid` | a resource ID is annotated `UUID4` |
| [`cel`](https://crates.io/crates/cel) 0.15 | a List request carries a `filter` |
| `aip-rs` with its default `connect` feature, and protovalidate-buffa's `Validate` on each unary request | a service is declared, with `connect=true` (the default) |


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

### `aip::FieldBehavior` and `aip::Resource`, and what is *not* generated for AIP-134

Two traits from [aip-rs](https://github.com/protoc-contrib/aip-rs), whose docs
hold the semantics of each item. `aip::FieldBehavior` is implemented for every
message the plugin generates, `aip::Resource` for every resource:

```rust
impl aip::FieldBehavior for Shipment {
    fn clear_fields(&mut self, kind: aip::FieldBehaviorKind) { /* one arm per kind it carries */ }
    fn validate_fields(&self, kind: aip::FieldBehaviorKind, scope: aip::FieldScope<'_>)
        -> Result<(), aip::FieldBehaviorError> { /* likewise */ }
}

impl aip::Resource for Shipment {
    type Name = ShipmentName;
    fn parse_name(&self) -> Result<ShipmentName, <ShipmentName as FromStr>::Err> { self.name.parse() }
    fn implied_field_mask(&self) -> buffa_types::FieldMask { /* the writable fields set */ }
}

impl aip::FieldBehavior for Voucher {} // nothing annotated: the defaults
```

Every message gets a `FieldBehavior` impl, an empty one when it carries no
behavior, so one server layer over `T: aip::FieldBehavior` handles every request
a service takes — a table naming each of them type-checks without impls of the
consumer's own. The trait is dyn-compatible. Bring a trait into scope where its
items are called, as for any trait.

`implied_field_mask` is the one AIP-134 shorthand protovalidate cannot handle:
**an omitted mask** is every field an update may write — not `OUTPUT_ONLY`,
`IDENTIFIER` or `IMMUTABLE` — that the client *populated*:

```rust
use aip::{FieldBehavior, FieldBehaviorKind, Resource};

// The client sent no update_mask: write what it populated.
shipment.clear_fields(FieldBehaviorKind::OutputOnly);
let update_mask = shipment.implied_field_mask();
```

Populated is read as protobuf reads presence — protobuf-go's `Has`, which is
what aip-go's `ImpliedFieldMask` uses: an `optional` field, a message or a
oneof member when set, even to its zero value; any other scalar when not its
zero value; a repeated field or map when not empty. So **clear, then imply**:
a populated message field is implied as one path — `carrier`, not
`carrier.name` — so the update writes everything beneath it, nested
`OUTPUT_ONLY` values included. And **a zero value is never implied**: `false`,
`0` and `""` on a field without explicit presence are indistinguishable from
unset, so a client clears one with an explicit mask, or the schema makes the
field `optional`.

**Validating a mask is not generated, and should not be.** That is
`(buf.validate.field).field_mask.in`, which protovalidate-buffa implements as a
first-class rule:

```proto
google.protobuf.FieldMask update_mask = 2 [
  (buf.validate.field).field_mask.in = ["display_name", "description"]
];
```

It already matches subpaths and reports as an ordinary violation under rule id
`field_mask.in`. protoc-gen-aip-lint's `update-mask-writable-fields` rule keeps
that list equal to the writable fields. A path naming an `OUTPUT_ONLY` field is
ignored rather than rejected, as AIP-203 has a server do: aip-rs's
`FieldBehaviorInterceptor` drops such paths from an explicit mask first, with
`aip::Resource::clear_field_mask` -- built on the generated `validate_fields` --
so `field_mask.in` only sees what remains. Any other path outside the list,
such as `name`, is still rejected.

`*`, full replacement, is not expanded here either: a service that accepts it
expands it to the writable fields itself, e.g. from the schema's descriptors
with aip-rs's `descriptor` feature.

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

`clear_fields(kind)` unsets the fields annotated `kind`: `OutputOnly` drops
server-owned values from an inbound request, `InputOnly` values a client may
send but must never get back from an outbound response. It is generated, not
reflective — and not as an optimisation. buffa 0.9 has **no reflective path to
mutation in any mode**: `Reflectable` exposes only `reflect(&self)`, and the
source marks `reflect_mut` as designed but deferred to the MergeSink work. The
read-only check is generated alongside, so field behavior has one mechanism.

```rust
impl aip::FieldBehavior for Shipment {
    fn clear_fields(&mut self, kind: aip::FieldBehaviorKind) {
        match kind {
            aip::FieldBehaviorKind::OutputOnly => {
                self.tracking_id = Default::default();
                for item in &mut self.parcels { item.clear_fields(kind); }
                // ...
            }
            aip::FieldBehaviorKind::Identifier => self.name = Default::default(),
            _ => {}
        }
    }
}
```

Each message has one arm per kind it carries at some depth, and passes the call
on to the messages it holds — singular, repeated, map-valued and oneof — through
their own impl; arms with the same body are shared (`A | B`). A field that is
cleared is not descended into. A `oneof` is cleared entirely when the member
that is set carries the kind, since there is no individual field to assign to.

Every operation and kind is generated; which a service applies, and where, is
its own policy. AIP-203 defines clearing `OUTPUT_ONLY` on input and
`INPUT_ONLY` on output, and checking `REQUIRED`.

**`REQUIRED` on a resource is checked here, not by protovalidate.** AIP-203
gives a `REQUIRED` resource field two meanings: a create must set it, and a
partial update may omit it unless its mask names it — then it must be set, or
the update would clear it. A resource message is shared by both requests, and a
`buf.validate` rule on it cannot tell which one it is in. So the split is by
what protovalidate can see:

- **protovalidate** keeps a request's own fields — `parent`, the resource
  field itself, `update_mask` — and value rules on the resource's fields:
  lengths, patterns, ranges. A value rule on a `REQUIRED` field needs
  `ignore = IGNORE_IF_ZERO_VALUE`, so an update that leaves it out passes.
- **`validate_fields(Required, scope)`** reads `REQUIRED` off the resource,
  within the scope the method asks for:

```rust
use aip::{FieldBehavior, FieldBehaviorKind, FieldScope};

// create: every REQUIRED field
item.validate_fields(FieldBehaviorKind::Required, FieldScope::All)?;
// update, after the implied mask is filled in
item.validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&update_mask.paths))?;
// Err(aip::FieldBehaviorError { kind, paths }): InvalidArgument, naming each path
```

`FieldScope::Mask` checks only the fields the mask names. A path naming a
message checks everything beneath it; a nested path, just that field. `*` is
matched as written, not expanded. The mask is the owned `FieldMask`'s `paths` as
it is, so a request's mask is passed without conversion.

Nested messages follow AIP-203's rule that a nested message's annotations are
independent of its parent's: a set singular message or oneof member is checked
through its own impl, and its failing fields are reported as dotted paths
(`destination.city`) from the message checked. An unset one is not entered with
`FieldScope::All`, or when the mask names it whole — its own `REQUIRED` is
reported at its level. A nested path is checked even when its parent is unset,
against the parent's default: `update_mask: destination.city` with no
`destination` writes `destination.city` as empty, so it would clear a
`REQUIRED` field and is reported. (For an unset oneof member whose type is in
another package, that last case is not checked: the generated code cannot name
the type to take its default.) Repeated and map fields are present when not
empty and are not entered, as a mask cannot address an element. A message
type that contains itself is followed as deep as it is set.

Presence is read as protobuf reads it, the rule carried over from aip-go
because it was a bug there first: a field with presence — a message, an
`optional` scalar, a oneof member — is judged set or unset, so an explicit
`false` satisfies a `REQUIRED` `optional bool`. A presence-less proto3 scalar
cannot tell unset from zero, so a `REQUIRED` one must be non-zero, and a
`REQUIRED` `bool` must be `true`.

An operation reaches only messages this run generates. A generated message
holding one from a file that is *not* generated, with fields of a kind beneath
it, fails generation, naming both — those fields could not be reached. Generate
that file too.

Handling a whole request is not left to the service either: with `connect=true`
every request gets an `aip::ResourceWrites` impl, saying which resources it
writes and how -- a create with its `<resource>_id`, an update with its
`update_mask`, each entry of a batch under `requests[i]` -- and each package a
`PROCEDURES` table, which aip-rs's `FieldBehaviorInterceptor` applies these
traits through: checking a create's resource in full and an update's against
its mask, minting an omitted id, implying an omitted mask (see [The
interceptor's inputs](#the-interceptors-inputs-connecttrue)). Naming a create's
resource stays the service's, from the request's `parent` and id. aip-go's
`ClearFields`, `ValidateFields` and `ImpliedFieldMask` answer the same, by
reflection.

A `REQUIRED` field on a resource that also carries
`(buf.validate.field).required` is one every partial update that omits it is
rejected for; [protoc-gen-aip-lint](https://github.com/protoc-contrib/protoc-gen-aip-lint)'s
`resource-required-rejects-zero` reports it.

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

`aip::FieldBehavior` and `aip::Resource` are not implemented for views.
`clear_fields` takes `&mut self`, and a view does not own what it would clear;
an implied mask is written onto the request, which is the owned message's job.

Off by default, and it has to be: nothing in a `CodeGeneratorRequest` says
whether buffa generated views, and emitting `impl FooView<'_>` when it did not
is a compile error in the consumer rather than a missing method.

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

### The interceptor's inputs: `connect=true`

On by default. For every message a service takes — and the entries of a batch
one — the plugin emits `aip::ResourceWrites`, reading the request's shape off
its fields as aip-go does by reflection: a create (a singular field whose type
is a resource), an update (that, and a `google.protobuf.FieldMask
update_mask`), or a batch (a repeated `requests` of either). Its
`for_each_write` hands each resource the request writes to the caller, as a
`ResourceWrite` with its path (`item`, `requests[1].item`, each step with the
field's number and type) and a `WriteMethod`: a create's `<resource>_id`, when
the request has one, or an update's `update_mask`, made present. Every other
request gets an empty impl. A resource needs nothing more: aip-rs makes every
`aip::Resource` an `aip::DynResource`.

```rust
impl ::aip::ResourceWrites for CreateOrderRequest {
    fn for_each_write(&mut self, f: &mut dyn ::core::ops::FnMut(::aip::ResourceWrite<'_>)) {
        if let ::core::option::Option::Some(resource) = self.order.as_option_mut() {
            f(::aip::ResourceWrite {
                path: ::aip::FieldPath::field("order", 2, ::aip::FieldType::Message),
                resource,
                method: ::aip::WriteMethod::Create {
                    id: ::core::option::Option::Some(&mut self.order_id),
                },
            });
        }
    }
}
```

The interceptor fills in only an omitted id; the name is the handler's to
build from the request's `parent` and id (AIP-133), with the generated
`parse_parent` and `parse_<resource>_id`.

Each package whose files declare a service gets a `PROCEDURES` table, one
`aip::Procedure` per method in declaration order: `Procedure::new`, the full
policy on each message, unary or streamed. A well-known type is named from
buffa-types, which aip-rs gives an empty `FieldBehavior`; a method whose
request is one is `Procedure::clear`, which clears only, as there is
nothing to fill in or validate. Any other message type this run does not
generate fails generation, naming the method.

```rust
let service = ConnectRpcService::new(router)
    .with_interceptor(aip::FieldBehaviorInterceptor::new(sdk::v1::PROCEDURES));
```

Every request but a well-known type must implement protovalidate-buffa's `Validate`,
which protoc-gen-protovalidate-buffa generates. Set `connect=false` to emit
none of this, for a consumer that does not serve Connect or builds aip-rs
without default features.

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
