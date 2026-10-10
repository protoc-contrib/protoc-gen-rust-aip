//! Walks a `CodeGeneratorRequest` into a [`Registry`] of every resource the
//! schema declares, and every field that references one.
//!
//! The walk covers *all* files in the request, not just the ones scheduled for
//! generation: a `google.api.resource_reference` names its referent by resource
//! type, and a child pattern finds its parent by matching that parent's
//! pattern, so both need to see resources declared in files that are only
//! imported.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use buffa::ExtensionSet;
use buffa_codegen::generated::{
    compiler::CodeGeneratorRequest,
    descriptor::{
        DescriptorProto, FieldDescriptorProto, FieldOptions, FileDescriptorProto, FileOptions,
        MessageOptions, field_descriptor_proto,
    },
};

use crate::annotations::google::api::{
    FIELD_INFO, RESOURCE, RESOURCE_DEFINITION, RESOURCE_REFERENCE, ResourceDescriptor, field_info,
};
use crate::idents::snake_case;
use crate::presence::Scope;

/// The AIP-159 wildcard, which `google.api.resource_reference` uses to mean
/// "any resource type". A reference that names it is not bound to a pattern, so
/// there is nothing to generate a parser from.
const WILDCARD_TYPE: &str = "*";

/// The declared value type of a variable segment.
///
/// Defaults to [`String`](Format::String); anything else is read off the
/// `google.api.field_info` on the AIP-133 create request that mints the ID.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Format {
    /// An opaque ID, constrained only by AIP-122: non-empty and no `/`.
    #[default]
    String,
    /// `google.api.field_info.format = UUID4`.
    Uuid,
}

impl Format {
    /// How the format reads in a codegen error message.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Uuid => "uuid4",
        }
    }
}

/// One segment of a resource name pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The literal text, or the variable name without its braces.
    pub name: String,
    /// Whether the segment captures a value.
    pub variable: bool,
    /// The type the segment holds. Always [`Format::String`] for a literal.
    ///
    /// Deliberately excluded from pattern matching: a format is a projection
    /// onto an already-established topology, so letting it decide whether a
    /// child finds its parent would let an annotation on one message silently
    /// sever a relationship declared on another. A disagreement is reported
    /// instead, by [`Registry::annotate_formats`].
    pub format: Format,
}

/// A parsed resource name pattern, e.g. `publishers/{publisher}/books/{book}`.
///
/// Held here as well as in `aip::ResourcePattern` because the generator has to
/// reason about the segments — to derive struct fields, to match a child
/// against its parent — and the runtime keeps its segments private. Which
/// patterns are valid is the runtime's alone to say: [`parse`](Self::parse)
/// asks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The pattern as written in the annotation.
    pub source: String,
    pub segments: Vec<Segment>,
}

impl Pattern {
    /// Parses a pattern string, rejecting exactly what
    /// `aip::ResourcePattern::compile` rejects, by calling it.
    ///
    /// The generated code compiles the same string at startup and panics if
    /// it fails, so asking the runtime here — rather than keeping a copy of its
    /// rules that could fall behind — turns a schema mistake into a
    /// `buf generate` failure naming the resource.
    fn parse(source: &str) -> Result<Self> {
        aip::ResourcePattern::compile(source)?;
        // Past the runtime's checks, a segment is either a literal or a whole
        // `{name}`; nothing else is left to reject.
        let segments = source
            .split('/')
            .map(|part| {
                let variable = part.strip_prefix('{').and_then(|n| n.strip_suffix('}'));
                Segment {
                    name: variable.unwrap_or(part).to_owned(),
                    variable: variable.is_some(),
                    format: Format::default(),
                }
            })
            .collect();
        Ok(Self {
            source: source.to_owned(),
            segments,
        })
    }

    /// The variable segments, in pattern order. One generated struct field
    /// each.
    pub fn variables(&self) -> impl Iterator<Item = &Segment> {
        self.segments.iter().filter(|segment| segment.variable)
    }

    /// How many variable segments the pattern has.
    #[must_use]
    pub fn arity(&self) -> usize {
        self.variables().count()
    }

    /// The pattern of the collection this one is nested in — the pattern with
    /// its trailing `{id}` and the collection literal above it removed.
    ///
    /// `publishers/{publisher}/books/{book}` has parent
    /// `publishers/{publisher}`; `books/{book}` has none, since a top-level
    /// collection is nobody's child.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let len = self.segments.len();
        if len < 4 || !self.segments[len - 1].variable || self.segments[len - 2].variable {
            return None;
        }
        let segments = self.segments[..len - 2].to_vec();
        Some(Self {
            source: render(&segments),
            segments,
        })
    }

    /// Whether two patterns describe the same shape: same segments, same
    /// literals, same variable names in the same places.
    ///
    /// [`Segment::format`] is excluded — see the note there.
    fn matches(&self, other: &Self) -> bool {
        self.segments.len() == other.segments.len()
            && self
                .segments
                .iter()
                .zip(&other.segments)
                .all(|(a, b)| a.name == b.name && a.variable == b.variable)
    }
}

/// Renders segments back to the pattern syntax, for a derived parent pattern
/// that was never written down anywhere.
fn render(segments: &[Segment]) -> String {
    let mut out = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            out.push('/');
        }
        if segment.variable {
            out.push('{');
            out.push_str(&segment.name);
            out.push('}');
        } else {
            out.push_str(&segment.name);
        }
    }
    out
}

/// The message a resource is declared on, when it is declared on one.
///
/// A `google.api.resource_definition` at file scope has no message, and so no
/// `name` field to parse — it gets the name type and nothing hung off a
/// message.
#[derive(Debug, Clone)]
pub struct MessageBinding {
    /// The message's path within its package, e.g. `Book`, or `outer::Inner`
    /// for a nested message.
    pub rust_path: String,
    /// The proto name of the field holding the resource name — `name` unless
    /// the annotation says otherwise.
    pub name_field: String,
    /// Whether that field has explicit presence, and so is an `Option<String>`
    /// rather than a `String` on the generated message.
    pub name_field_optional: bool,
}

/// A single `google.api.resource` or `google.api.resource_definition`.
#[derive(Debug, Clone)]
pub struct Resource {
    /// The declared type, e.g. `example.com/Book`.
    pub resource_type: String,
    /// The service domain — the part of the type before the `/`.
    pub domain: String,
    /// The type's short name — the part after the `/`, e.g. `Book`. The
    /// generated name type is this plus `Name`.
    pub type_name: String,
    /// Every declared pattern, in declaration order. Never empty.
    pub patterns: Vec<Pattern>,
    /// The proto file the declaration is in, e.g. `library/v1/library.proto`.
    pub source_file: String,
    /// The proto package the declaration is in, e.g. `library.v1`.
    pub package: String,
    /// The message it is declared on, if any.
    pub message: Option<MessageBinding>,
}

impl Resource {
    /// The Rust type name for the parsed form, e.g. `BookName`.
    #[must_use]
    pub fn name_type(&self) -> String {
        format!("{}Name", self.type_name)
    }

    /// Whether the resource declares more than one pattern, and so needs an
    /// enum over one variant per pattern rather than a single struct.
    #[must_use]
    pub fn is_multi_pattern(&self) -> bool {
        self.patterns.len() > 1
    }
}

/// A field annotated `google.api.resource_reference`, resolved to what it
/// refers to.
#[derive(Debug, Clone)]
pub struct Reference {
    /// The message's path within its package, e.g. `CreateBookRequest`.
    pub rust_path: String,
    /// The proto name of the referring field, e.g. `parent`.
    pub field_name: String,
    /// Whether that field has explicit presence, and so is an `Option<String>`
    /// rather than a `String` on the generated message.
    pub field_optional: bool,
    /// Index into [`Registry::resources`] of the resource referred to.
    pub resource: usize,
}

/// Every resource in the request, indexed for the two lookups the emitter
/// needs: by resource type, for references, and by pattern, for parents.
#[derive(Debug, Default)]
pub struct Registry {
    /// Every resource, in the order the walk found it. Generation iterates this
    /// so output does not depend on map iteration order.
    pub resources: Vec<Resource>,
    /// Every resolved reference, grouped by the file that declares the
    /// referring field.
    pub references: BTreeMap<String, Vec<Reference>>,
    /// Every declaration of each type, in the order the walk found them. More
    /// than one when versions of an API -- `v1` and `v2` -- each declare it.
    by_type: BTreeMap<String, Vec<usize>>,
}

/// What a resource type names, as seen from one package.
enum Lookup {
    /// The declaration in that package, or the only one there is.
    Found(usize),
    /// No package declares it.
    Missing,
    /// Several packages declare it and none is the one asking.
    Ambiguous(Vec<String>),
}

impl Registry {
    /// The first declaration of `resource_type`, if the request has one. See
    /// [`Registry::resolve`] for the one a given package means.
    #[must_use]
    pub fn by_type(&self, resource_type: &str) -> Option<&Resource> {
        self.by_type
            .get(resource_type)
            .and_then(|indices| indices.first())
            .map(|i| &self.resources[*i])
    }

    /// The declaration of `resource_type` that `package` means: its own, if
    /// it declares one -- each version of an API refers to its own resources --
    /// else the only one there is.
    fn resolve(&self, resource_type: &str, package: &str) -> Lookup {
        let Some(indices) = self.by_type.get(resource_type) else {
            return Lookup::Missing;
        };
        if let Some(own) = indices
            .iter()
            .find(|i| self.resources[**i].package == package)
        {
            return Lookup::Found(*own);
        }
        match indices.as_slice() {
            [only] => Lookup::Found(*only),
            _ => Lookup::Ambiguous(
                indices
                    .iter()
                    .map(|i| self.resources[*i].package.clone())
                    .collect(),
            ),
        }
    }

    /// The resource and pattern index whose pattern is exactly `pattern`, as
    /// seen from `package`.
    ///
    /// This is how a child finds its parent: AIP-122 does not record the
    /// relationship, so the only evidence that `publishers/{publisher}` is
    /// `Book`'s parent is that some resource declares that exact pattern. When
    /// several packages declare it -- versions of one API -- the one in
    /// `package` wins, and with none there and more than one elsewhere there is
    /// no telling which was meant, so there is no parent.
    #[must_use]
    pub fn find_by_pattern(&self, pattern: &Pattern, package: &str) -> Option<(&Resource, usize)> {
        let mut found = self.resources.iter().filter_map(|resource| {
            resource
                .patterns
                .iter()
                .position(|candidate| candidate.matches(pattern))
                .map(|index| (resource, index))
        });
        let candidates: Vec<(&Resource, usize)> = found.by_ref().collect();
        if let Some(own) = candidates
            .iter()
            .find(|(resource, _)| resource.package == package)
        {
            return Some(*own);
        }
        match candidates.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    /// The resources declared in `file`, in declaration order.
    pub fn by_file<'a>(&'a self, file: &str) -> impl Iterator<Item = &'a Resource> {
        self.resources
            .iter()
            .filter(move |resource| resource.source_file == file)
    }

    /// Fills in [`Segment::format`] for every variable segment, from the
    /// `Create<Type>Request` of whichever resource owns that segment.
    ///
    /// The owner of a segment is the resource whose pattern is the prefix of
    /// this one up to and including it. So a child's leading segments take
    /// their format from the *parent's* create request rather than its own,
    /// which is both correct — that is the message which mints those IDs — and
    /// what makes a child's fields line up with the parent struct `parent()`
    /// builds.
    ///
    /// That is also why a child can never disagree with its parent about a
    /// segment's format, and why nothing checks: `parent()` resolves the parent
    /// with the same `find_by_pattern` over the same prefix that assigned the
    /// format here, so both arrive at one owner and one answer. Reading a
    /// resource's own create request instead would break that, and would need a
    /// consistency check to go with it.
    fn annotate_formats(&mut self, create_requests: &CreateRequests) {
        // Resolved against the pre-annotation registry and applied afterwards,
        // because `find_by_pattern` borrows the whole registry.
        let mut resolved: Vec<Resolved> = Vec::new();
        for (resource_index, resource) in self.resources.iter().enumerate() {
            for (pattern_index, pattern) in resource.patterns.iter().enumerate() {
                for segment_index in 0..pattern.segments.len() {
                    let segment = &pattern.segments[segment_index];
                    if !segment.variable {
                        continue;
                    }
                    let segments = pattern.segments[..=segment_index].to_vec();
                    let prefix = Pattern {
                        source: render(&segments),
                        segments,
                    };
                    let Some((owner, _)) = self.find_by_pattern(&prefix, &resource.package) else {
                        continue;
                    };
                    // The owner's own package: one version's `field_info` does
                    // not type another's segment.
                    let request = (
                        owner.package.clone(),
                        format!("Create{}Request", owner.type_name),
                    );
                    let id_field = format!("{}_id", segment.name);
                    let format = create_requests
                        .get(&request)
                        .and_then(|fields| fields.get(&id_field))
                        .copied()
                        .unwrap_or_default();
                    if format != Format::String {
                        resolved.push(Resolved {
                            resource: resource_index,
                            pattern: pattern_index,
                            segment: segment_index,
                            format,
                        });
                    }
                }
            }
        }
        for entry in resolved {
            self.resources[entry.resource].patterns[entry.pattern].segments[entry.segment].format =
                entry.format;
        }
    }

    fn insert(&mut self, resource: Resource) -> Result<()> {
        let index = self.resources.len();
        let declared = self
            .by_type
            .entry(resource.resource_type.clone())
            .or_default();
        // One per package: versions of an API each declare their resources,
        // but two in one package would leave a reference from it nothing to
        // choose by.
        if let Some(previous) = declared
            .iter()
            .find(|i| self.resources[**i].package == resource.package)
        {
            bail!(
                "resource type {:?} is declared twice in package {:?}: in {} and in {}",
                resource.resource_type,
                resource.package,
                self.resources[*previous].source_file,
                resource.source_file,
            );
        }
        declared.push(index);
        self.resources.push(resource);
        Ok(())
    }
}

/// One segment's format, resolved but not yet applied.
///
/// Three bare `usize` indices in a tuple would let a call site transpose them
/// silently; named, the assignment reads as what it is.
struct Resolved {
    resource: usize,
    pattern: usize,
    segment: usize,
    format: Format,
}

/// Builds the registry from every file in the request.
///
/// # Errors
///
/// If an annotation is malformed: a resource type without a `/`, a resource
/// with no patterns, an unparsable pattern, a `name_field` naming a field the
/// message does not have, or two resources claiming one type. Or, unless
/// `allow_unresolved_refs`, if a file scheduled for generation references a
/// resource type the request does not declare.
pub fn gather(request: &CodeGeneratorRequest, allow_unresolved_refs: bool) -> Result<Registry> {
    let mut registry = Registry::default();
    for file in &request.proto_file {
        walk_file(file, &mut registry)?;
    }
    // Both remaining passes resolve against the finished registry: a file may
    // reference, or be nested under, a resource declared in a file that comes
    // later in the request.
    registry.annotate_formats(&create_requests(request));
    for file in &request.proto_file {
        // Only a file being generated is held to its references. A dependency
        // may name a resource type it never imports -- types are strings -- and
        // nothing is emitted for it either way.
        let strict = !allow_unresolved_refs
            && file
                .name
                .as_ref()
                .is_some_and(|name| request.file_to_generate.contains(name));
        collect_references(file, &mut registry, strict)?;
    }
    Ok(registry)
}

/// Every `Create<Type>Request` in the request, by package and message name,
/// mapping each of its field names to that field's declared format.
type CreateRequests = BTreeMap<(String, String), BTreeMap<String, Format>>;

/// Every `Create<Type>Request` message in the request, by name, with the
/// `google.api.field_info` format of each of its fields.
///
/// AIP-133 gives a create request a `{resource}_id` field carrying the ID the
/// caller proposes, which is the only place a schema says what a resource ID
/// actually *is*. Reading the format there rather than inventing an annotation
/// on the pattern is what keeps this agreeing with `protoc-gen-go-aip`.
fn create_requests(request: &CodeGeneratorRequest) -> CreateRequests {
    let mut out = BTreeMap::new();
    for file in &request.proto_file {
        let package = file.package.clone().unwrap_or_default();
        for message in &file.message_type {
            collect_create_requests(message, &package, &mut out);
        }
    }
    out
}

/// Records `message` if it is a `Create<Type>Request`, then its nested
/// messages: protoc-gen-go-aip finds a nested one too.
fn collect_create_requests(message: &DescriptorProto, package: &str, out: &mut CreateRequests) {
    if let Some(name) = message.name.as_deref()
        && name.starts_with("Create")
        && name.ends_with("Request")
    {
        let fields: BTreeMap<String, Format> = message
            .field
            .iter()
            .filter_map(|field| Some((field.name.clone()?, field_format(field))))
            .collect();
        out.insert((package.to_owned(), name.to_owned()), fields);
    }
    for nested in &message.nested_type {
        collect_create_requests(nested, package, out);
    }
}

/// The declared format of a field, from `google.api.field_info`.
///
/// A field with no annotation, an unset format, or a format this plugin has no
/// Rust type for is a plain string — the same fallback `protoc-gen-go-aip`
/// takes, so an unrecognised format degrades rather than failing the build.
fn field_format(field: &FieldDescriptorProto) -> Format {
    let format = field
        .options
        .as_option()
        .and_then(|options: &FieldOptions| options.extension(&FIELD_INFO))
        .map(|info| info.format);
    match format.and_then(|format| format.as_known()) {
        Some(field_info::Format::UUID4) => Format::Uuid,
        _ => Format::String,
    }
}

fn walk_file(file: &FileDescriptorProto, registry: &mut Registry) -> Result<()> {
    let source_file = file.name.clone().unwrap_or_default();
    let package = file.package.clone().unwrap_or_default();

    let definitions = file
        .options
        .as_option()
        .map(|options: &FileOptions| options.extension(&RESOURCE_DEFINITION))
        .unwrap_or_default();
    for descriptor in definitions {
        let resource = build(&descriptor, &source_file, &package, None)?;
        registry.insert(resource)?;
    }

    let scope = Scope::file(file);
    for message in &file.message_type {
        walk_message(message, &[], &source_file, &package, scope, registry)?;
    }
    Ok(())
}

fn walk_message(
    message: &DescriptorProto,
    parents: &[&str],
    source_file: &str,
    package: &str,
    scope: Scope,
    registry: &mut Registry,
) -> Result<()> {
    let name = message.name.as_deref().unwrap_or_default();
    let scope = scope.message(message);
    if let Some(descriptor) = message
        .options
        .as_option()
        .and_then(|options: &MessageOptions| options.extension(&RESOURCE))
    {
        let name_field = if descriptor.name_field.is_empty() {
            "name"
        } else {
            &descriptor.name_field
        };
        let field = message
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some(name_field))
            .ok_or_else(|| {
                anyhow!(
                    "resource {:?}: {name} has no field {name_field:?} to hold the resource name",
                    descriptor.r#type,
                )
            })?;
        if !is_string(field) {
            bail!(
                "resource {:?}: {name}.{name_field} holds the resource name, so it must be a \
                 singular string",
                descriptor.r#type,
            );
        }
        let binding = MessageBinding {
            rust_path: rust_path(parents, name),
            name_field: name_field.to_owned(),
            name_field_optional: scope.is_option(field),
        };
        let resource = build(&descriptor, source_file, package, Some(binding))?;
        registry.insert(resource)?;
    }

    let mut nested_parents = parents.to_vec();
    nested_parents.push(name);
    for nested in &message.nested_type {
        walk_message(
            nested,
            &nested_parents,
            source_file,
            package,
            scope,
            registry,
        )?;
    }
    Ok(())
}

fn build(
    descriptor: &ResourceDescriptor,
    source_file: &str,
    package: &str,
    message: Option<MessageBinding>,
) -> Result<Resource> {
    let resource_type = descriptor.r#type.clone();
    let (domain, type_name) = resource_type.split_once('/').ok_or_else(|| {
        anyhow!(
            "resource type {resource_type:?} in {source_file}: want \"{{service}}/{{Type}}\", \
             e.g. \"library.example.com/Book\""
        )
    })?;
    if domain.is_empty() || type_name.is_empty() {
        bail!(
            "resource type {resource_type:?} in {source_file}: \
             both the service and the type must be non-empty"
        );
    }
    if descriptor.pattern.is_empty() {
        bail!("resource {resource_type:?} in {source_file}: declares no pattern");
    }
    let patterns = descriptor
        .pattern
        .iter()
        .map(|pattern| {
            Pattern::parse(pattern)
                .map_err(|error| anyhow!("resource {resource_type:?} in {source_file}: {error}"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Resource {
        resource_type: resource_type.clone(),
        domain: domain.to_owned(),
        type_name: type_name.to_owned(),
        patterns,
        source_file: source_file.to_owned(),
        package: package.to_owned(),
        message,
    })
}

/// Records every `google.api.resource_reference` whose referent is a resource
/// the registry knows, keyed by the file the referring field is in.
///
/// A reference the generator cannot turn into a parser is skipped rather than
/// rejected -- it is a valid annotation that this plugin has nothing to say
/// about, and failing the build over it would make the annotation unusable:
///
/// - `child_type` names the *children* of the field's value, so the field's own
///   pattern is not determined by it.
/// - `type: "*"` is deliberately unbound.
/// - a non-`string` or repeated field carries no single name to parse.
///
/// A `type` naming a resource the request does not declare is different: the
/// schema says the field holds one, and a silent skip drops its accessor with
/// nothing to say so. With `strict`, that fails generation, naming the field —
/// as protoc-gen-go-aip does — unless `allow_unresolved_refs` turned it off.
fn collect_references(
    file: &FileDescriptorProto,
    registry: &mut Registry,
    strict: bool,
) -> Result<()> {
    let source_file = file.name.clone().unwrap_or_default();
    let package = file.package.as_deref().unwrap_or_default();
    let scope = Scope::file(file);
    let mut found = Vec::new();
    for message in &file.message_type {
        collect_message_references(message, &[], package, scope, registry, strict, &mut found)?;
    }
    if !found.is_empty() {
        registry.references.insert(source_file, found);
    }
    Ok(())
}

fn collect_message_references(
    message: &DescriptorProto,
    parents: &[&str],
    package: &str,
    scope: Scope,
    registry: &Registry,
    strict: bool,
    found: &mut Vec<Reference>,
) -> Result<()> {
    let name = message.name.as_deref().unwrap_or_default();
    let scope = scope.message(message);
    for field in &message.field {
        match resolve_reference(field, parents, name, package, scope, registry) {
            Resolution::Resolved(reference) => found.push(reference),
            Resolution::Ambiguous(resource_type, packages) if strict => {
                let field = field.name.as_deref().unwrap_or_default();
                bail!(
                    "{}.{field}: reference to type {resource_type:?} is ambiguous: it is \
                     declared in {} but not in {package:?} (set allow_unresolved_refs=true \
                     to skip)",
                    qualified(package, parents, name),
                    packages.join(", "),
                );
            }
            Resolution::Unknown(resource_type) if strict => {
                let field = field.name.as_deref().unwrap_or_default();
                bail!(
                    "{}.{field}: reference to unknown type {resource_type:?} \
                     (set allow_unresolved_refs=true to skip)",
                    qualified(package, parents, name),
                );
            }
            Resolution::Skipped | Resolution::Unknown(_) | Resolution::Ambiguous(..) => {}
        }
    }

    let mut nested_parents = parents.to_vec();
    nested_parents.push(name);
    for nested in &message.nested_type {
        collect_message_references(
            nested,
            &nested_parents,
            package,
            scope,
            registry,
            strict,
            found,
        )?;
    }
    Ok(())
}

/// A message's fully-qualified proto name, without the leading dot.
fn qualified(package: &str, parents: &[&str], name: &str) -> String {
    let message = parents
        .iter()
        .copied()
        .chain([name])
        .collect::<Vec<_>>()
        .join(".");
    if package.is_empty() {
        message
    } else {
        format!("{package}.{message}")
    }
}

/// What a field's `google.api.resource_reference` comes to.
enum Resolution {
    /// A reference to a resource the request declares.
    Resolved(Reference),
    /// No reference, or one this generator has nothing to emit for.
    Skipped,
    /// A reference to this resource type, which the request does not declare.
    Unknown(String),
    /// A reference to this resource type, declared in these packages, none of
    /// them the referring one.
    Ambiguous(String, Vec<String>),
}

fn resolve_reference(
    field: &FieldDescriptorProto,
    parents: &[&str],
    message_name: &str,
    package: &str,
    scope: Scope,
    registry: &Registry,
) -> Resolution {
    let Some(reference) = field
        .options
        .as_option()
        .and_then(|options: &FieldOptions| options.extension(&RESOURCE_REFERENCE))
    else {
        return Resolution::Skipped;
    };
    if reference.r#type.is_empty() || reference.r#type == WILDCARD_TYPE {
        return Resolution::Skipped;
    }
    // A `repeated string` of names, or a non-string field, carries no single
    // name to parse. Skipped for the same reason as `child_type`: the
    // annotation is legal, this generator just has nothing to emit for it.
    if !is_string(field) {
        return Resolution::Skipped;
    }
    let resource = match registry.resolve(&reference.r#type, package) {
        Lookup::Found(resource) => resource,
        Lookup::Missing => return Resolution::Unknown(reference.r#type.clone()),
        Lookup::Ambiguous(packages) => {
            return Resolution::Ambiguous(reference.r#type.clone(), packages);
        }
    };
    let Some(field_name) = field.name.clone() else {
        return Resolution::Skipped;
    };
    Resolution::Resolved(Reference {
        rust_path: rust_path(parents, message_name),
        field_name,
        field_optional: scope.is_option(field),
        resource,
    })
}

/// Whether a field is a singular `string` -- the only shape a resource name can
/// travel in, and the only one the emitted accessors know how to read.
fn is_string(field: &FieldDescriptorProto) -> bool {
    field.r#type == Some(field_descriptor_proto::Type::TYPE_STRING)
        && field.label != Some(field_descriptor_proto::Label::LABEL_REPEATED)
}

/// The Rust path of a message within its package's module.
///
/// buffa puts a nested message in a module named after its parent, so proto
/// `Outer.Inner` is Rust `outer::Inner`. Matching that here is what lets the
/// emitted `impl` blocks name the type buffa generated.
fn rust_path(parents: &[&str], name: &str) -> String {
    let mut path = String::new();
    for parent in parents {
        path.push_str(&buffa_codegen::idents::escape_mod_ident(&snake_case(
            parent,
        )));
        path.push_str("::");
    }
    path.push_str(name);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_pattern_into_segments() {
        let pattern = Pattern::parse("publishers/{publisher}/books/{book}").unwrap();
        assert_eq!(pattern.arity(), 2);
        assert_eq!(
            pattern
                .variables()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["publisher", "book"]
        );
    }

    #[test]
    fn rejects_malformed_patterns() {
        for bad in [
            "",
            "publishers//books",
            "publishers/{publisher",
            "publishers/publisher}",
            "publishers/{}",
            "books/{book}/editions/{book}",
            // AIP-122 spelling, which the runtime enforces: a camelCase
            // literal, a snake_case variable, no path-template syntax.
            "book_shelves/{shelf}",
            "Publishers/{publisher}",
            "books/{bookId}",
            "books/{book=**}",
        ] {
            assert!(
                Pattern::parse(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn derives_the_parent_pattern() {
        let parent = Pattern::parse("publishers/{publisher}/books/{book}")
            .unwrap()
            .parent()
            .unwrap();
        assert_eq!(parent.source, "publishers/{publisher}");
    }

    #[test]
    fn a_top_level_collection_has_no_parent() {
        assert!(Pattern::parse("books/{book}").unwrap().parent().is_none());
    }

    #[test]
    fn a_pattern_ending_in_a_literal_has_no_parent() {
        // A singleton like "publishers/{publisher}/settings" is not an instance
        // of a collection, so there is no {id} to strip.
        assert!(
            Pattern::parse("publishers/{publisher}/settings")
                .unwrap()
                .parent()
                .is_none()
        );
    }

    #[test]
    fn patterns_match_on_shape() {
        let a = Pattern::parse("publishers/{publisher}").unwrap();
        let b = Pattern::parse("publishers/{publisher}/books/{book}")
            .unwrap()
            .parent()
            .unwrap();
        assert!(a.matches(&b));
        assert!(!a.matches(&Pattern::parse("authors/{publisher}").unwrap()));
        // Same shape, different variable name: a parent whose segment is named
        // differently is a different pattern, and binding to it would silently
        // rename the field.
        assert!(!a.matches(&Pattern::parse("publishers/{author}").unwrap()));
    }

    /// A registry holding one resource per (type, pattern) pair, enough to
    /// drive format derivation without going through a descriptor set.
    fn registry(resources: &[(&str, &str)]) -> Registry {
        let mut registry = Registry::default();
        for (resource_type, pattern) in resources {
            let (domain, type_name) = resource_type.split_once('/').unwrap();
            registry
                .insert(Resource {
                    resource_type: (*resource_type).to_owned(),
                    domain: domain.to_owned(),
                    type_name: type_name.to_owned(),
                    patterns: vec![Pattern::parse(pattern).unwrap()],
                    source_file: "test.proto".to_owned(),
                    package: "test.v1".to_owned(),
                    message: None,
                })
                .unwrap();
        }
        registry
    }

    /// A request whose `ref.proto` declares `test.v1.Outer.Inner` with a
    /// `bar` field referencing `example.com/Missing`, which nothing declares.
    fn unresolved_request(generate: bool) -> CodeGeneratorRequest {
        use crate::annotations::google::api::ResourceReference;

        let mut options = FieldOptions::default();
        options.set_extension(
            &RESOURCE_REFERENCE,
            ResourceReference {
                r#type: "example.com/Missing".to_owned(),
                ..Default::default()
            },
        );
        let field = FieldDescriptorProto {
            name: Some("bar".to_owned()),
            number: Some(1),
            r#type: Some(field_descriptor_proto::Type::TYPE_STRING),
            label: Some(field_descriptor_proto::Label::LABEL_OPTIONAL),
            options: options.into(),
            ..Default::default()
        };
        let inner = DescriptorProto {
            name: Some("Inner".to_owned()),
            field: vec![field],
            ..Default::default()
        };
        let outer = DescriptorProto {
            name: Some("Outer".to_owned()),
            nested_type: vec![inner],
            ..Default::default()
        };
        CodeGeneratorRequest {
            file_to_generate: if generate {
                vec!["ref.proto".to_owned()]
            } else {
                Vec::new()
            },
            proto_file: vec![FileDescriptorProto {
                name: Some("ref.proto".to_owned()),
                package: Some("test.v1".to_owned()),
                message_type: vec![outer],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// `pkg.Thing` declaring `example.com/Thing` with pattern `things/{thing}`.
    fn declaring(package: &str) -> FileDescriptorProto {
        let mut options = MessageOptions::default();
        options.set_extension(
            &RESOURCE,
            ResourceDescriptor {
                r#type: "example.com/Thing".to_owned(),
                pattern: vec!["things/{thing}".to_owned()],
                ..Default::default()
            },
        );
        let name = FieldDescriptorProto {
            name: Some("name".to_owned()),
            number: Some(1),
            r#type: Some(field_descriptor_proto::Type::TYPE_STRING),
            label: Some(field_descriptor_proto::Label::LABEL_OPTIONAL),
            ..Default::default()
        };
        FileDescriptorProto {
            name: Some(format!("{package}.proto")),
            package: Some(package.to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Thing".to_owned()),
                field: vec![name],
                options: options.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// `pkg.Holder` with a `thing` field referencing `example.com/Thing`.
    fn referencing(mut file: FileDescriptorProto) -> FileDescriptorProto {
        use crate::annotations::google::api::ResourceReference;

        let mut options = FieldOptions::default();
        options.set_extension(
            &RESOURCE_REFERENCE,
            ResourceReference {
                r#type: "example.com/Thing".to_owned(),
                ..Default::default()
            },
        );
        file.message_type.push(DescriptorProto {
            name: Some("Holder".to_owned()),
            field: vec![FieldDescriptorProto {
                name: Some("thing".to_owned()),
                number: Some(1),
                r#type: Some(field_descriptor_proto::Type::TYPE_STRING),
                label: Some(field_descriptor_proto::Label::LABEL_OPTIONAL),
                options: options.into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        file
    }

    fn generating(files: Vec<FileDescriptorProto>) -> CodeGeneratorRequest {
        CodeGeneratorRequest {
            file_to_generate: files.iter().filter_map(|file| file.name.clone()).collect(),
            proto_file: files,
            ..Default::default()
        }
    }

    #[test]
    fn each_version_binds_to_its_own_declaration() {
        let request = generating(vec![
            referencing(declaring("a.v1")),
            referencing(declaring("a.v2")),
        ]);
        let registry = gather(&request, false).unwrap();
        for (file, package) in [("a.v1.proto", "a.v1"), ("a.v2.proto", "a.v2")] {
            let reference = &registry.references[file][0];
            assert_eq!(registry.resources[reference.resource].package, package);
        }
    }

    #[test]
    fn a_reference_from_a_third_package_is_ambiguous() {
        let mut third = referencing(FileDescriptorProto {
            name: Some("c.proto".to_owned()),
            package: Some("c.v1".to_owned()),
            syntax: Some("proto3".to_owned()),
            ..Default::default()
        });
        third
            .message_type
            .retain(|message| message.name.as_deref() == Some("Holder"));
        let request = generating(vec![declaring("a.v1"), declaring("a.v2"), third.clone()]);
        let error = gather(&request, false).unwrap_err().to_string();
        assert!(
            error.starts_with("c.v1.Holder.thing: reference to type"),
            "{error}"
        );
        assert!(error.contains("a.v1, a.v2"), "{error}");
        // And a unique declaration elsewhere is not ambiguous.
        let request = generating(vec![declaring("a.v1"), third]);
        assert!(gather(&request, false).is_ok());
    }

    #[test]
    fn two_declarations_in_one_package_fail() {
        let mut twice = declaring("a.v1");
        twice.message_type.push(twice.message_type[0].clone());
        twice.message_type[1].name = Some("Other".to_owned());
        let error = gather(&generating(vec![twice]), false).unwrap_err();
        assert!(
            error.to_string().contains("declared twice in package"),
            "{error}"
        );
    }

    #[test]
    fn a_reference_to_an_unknown_type_fails_naming_the_field() {
        let error = gather(&unresolved_request(true), false).unwrap_err();
        assert_eq!(
            error.to_string(),
            r#"test.v1.Outer.Inner.bar: reference to unknown type "example.com/Missing" (set allow_unresolved_refs=true to skip)"#
        );
    }

    #[test]
    fn allow_unresolved_refs_skips_it() {
        let registry = gather(&unresolved_request(true), true).unwrap();
        assert!(registry.references.is_empty());
    }

    #[test]
    fn a_dependency_is_not_held_to_its_references() {
        // Nothing is generated for a file only imported, and it may name a
        // resource type it never imports.
        let registry = gather(&unresolved_request(false), false).unwrap();
        assert!(registry.references.is_empty());
    }

    fn create_request(
        name: &str,
        field: &str,
        format: Format,
    ) -> ((String, String), BTreeMap<String, Format>) {
        (
            ("test.v1".to_owned(), name.to_owned()),
            [(field.to_owned(), format)].into_iter().collect(),
        )
    }

    fn formats(registry: &Registry, resource_type: &str) -> Vec<Format> {
        registry.by_type(resource_type).unwrap().patterns[0]
            .variables()
            .map(|segment| segment.format)
            .collect()
    }

    #[test]
    fn a_segment_is_typed_by_its_own_create_request() {
        let mut registry = registry(&[("example.com/Collection", "collections/{collection}")]);
        let requests = [create_request(
            "CreateCollectionRequest",
            "collection_id",
            Format::Uuid,
        )]
        .into_iter()
        .collect();
        registry.annotate_formats(&requests);
        assert_eq!(formats(&registry, "example.com/Collection"), [Format::Uuid]);
    }

    #[test]
    fn a_leading_segment_is_typed_by_the_parent_that_mints_it() {
        let mut registry = registry(&[
            ("example.com/Org", "organizations/{organization}"),
            (
                "example.com/Item",
                "organizations/{organization}/items/{item}",
            ),
        ]);
        // Only the parent's create request declares the format. The child's
        // {organization} must still pick it up, or `parent()` would try to
        // build a `Uuid` field from a `String`.
        let requests = [create_request(
            "CreateOrgRequest",
            "organization_id",
            Format::Uuid,
        )]
        .into_iter()
        .collect();
        registry.annotate_formats(&requests);
        assert_eq!(
            formats(&registry, "example.com/Item"),
            [Format::Uuid, Format::String]
        );
    }

    #[test]
    fn a_format_on_the_wrong_create_request_is_ignored() {
        let mut registry = registry(&[
            ("example.com/Org", "organizations/{organization}"),
            (
                "example.com/Item",
                "organizations/{organization}/items/{item}",
            ),
        ]);
        // Item does not own {organization}, so its own create request has no
        // say over that segment -- only over {item}.
        let requests = [
            create_request("CreateItemRequest", "organization_id", Format::Uuid),
            create_request("CreateItemRequest", "item_id", Format::Uuid),
        ]
        .into_iter()
        .collect();
        registry.annotate_formats(&requests);
        assert_eq!(
            formats(&registry, "example.com/Item"),
            [Format::String, Format::Uuid]
        );
    }

    #[test]
    fn an_unannotated_id_stays_a_string() {
        let mut registry = registry(&[("example.com/Book", "books/{book}")]);
        registry.annotate_formats(&BTreeMap::new());
        assert_eq!(formats(&registry, "example.com/Book"), [Format::String]);
    }

    #[test]
    fn a_format_does_not_decide_whether_a_pattern_matches() {
        // Two patterns of the same shape must still match once one of them has
        // been annotated, or annotating a create request would sever the
        // parent link declared on another message.
        let mut typed = Pattern::parse("organizations/{organization}").unwrap();
        typed.segments[1].format = Format::Uuid;
        assert!(typed.matches(&Pattern::parse("organizations/{organization}").unwrap()));
    }

    #[test]
    fn nests_a_message_the_way_buffa_does() {
        assert_eq!(rust_path(&[], "Book"), "Book");
        assert_eq!(rust_path(&["Outer"], "Inner"), "outer::Inner");
        assert_eq!(rust_path(&["Outer", "Inner"], "Leaf"), "outer::inner::Leaf");
        assert_eq!(rust_path(&["HTTPServer"], "Config"), "http_server::Config");
    }
}
