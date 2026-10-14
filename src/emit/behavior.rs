//! Emits AIP-203 field behavior as the two `aip::FieldBehavior` operations:
//! `clear_fields(kind)`, which unsets the fields annotated `kind` at any depth,
//! and `validate_fields(kind, scope)`, which reports those of them `scope`
//! contains that are unset. Each is emitted with one arm per kind the message
//! needs, and reaches the messages beneath it through their own impl.
//!
//! Generated rather than reflective, and not as an optimisation: buffa 0.9
//! exposes only `reflect(&self)`, and marks `reflect_mut` as designed but
//! deferred, so nothing can mutate a message reflectively in any mode. The
//! read-only check is generated alongside, so field behavior has one mechanism.
//!
//! Revisit the whole split if a buffa release lands `reflect_mut`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::messages::{Field, Index, Kind, Message};

/// A `google.api.field_behavior` value, as `aip::FieldBehaviorKind` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Behavior {
    Optional,
    Required,
    OutputOnly,
    InputOnly,
    Immutable,
    UnorderedList,
    NonEmptyDefault,
    Identifier,
}

impl Behavior {
    /// Every behavior, in `google.api.FieldBehavior` order.
    pub const ALL: [Self; 8] = [
        Self::Optional,
        Self::Required,
        Self::OutputOnly,
        Self::InputOnly,
        Self::Immutable,
        Self::UnorderedList,
        Self::NonEmptyDefault,
        Self::Identifier,
    ];

    /// Whether `field` carries this behavior.
    #[must_use]
    pub fn marks(self, field: &Field) -> bool {
        match self {
            Self::Optional => field.optional_behavior,
            Self::Required => field.must_set,
            Self::OutputOnly => field.output_only,
            Self::InputOnly => field.input_only,
            Self::Immutable => field.immutable,
            Self::UnorderedList => field.unordered_list,
            Self::NonEmptyDefault => field.non_empty_default,
            Self::Identifier => field.identifier,
        }
    }

    /// The annotation value, as the schema spells it.
    fn annotation(self) -> &'static str {
        match self {
            Self::Optional => "OPTIONAL",
            Self::Required => "REQUIRED",
            Self::OutputOnly => "OUTPUT_ONLY",
            Self::InputOnly => "INPUT_ONLY",
            Self::Immutable => "IMMUTABLE",
            Self::UnorderedList => "UNORDERED_LIST",
            Self::NonEmptyDefault => "NON_EMPTY_DEFAULT",
            Self::Identifier => "IDENTIFIER",
        }
    }

    /// The `aip::FieldBehaviorKind` variant.
    fn kind(self) -> TokenStream {
        let variant = format_ident!(
            "{}",
            match self {
                Self::Optional => "Optional",
                Self::Required => "Required",
                Self::OutputOnly => "OutputOnly",
                Self::InputOnly => "InputOnly",
                Self::Immutable => "Immutable",
                Self::UnorderedList => "UnorderedList",
                Self::NonEmptyDefault => "NonEmptyDefault",
                Self::Identifier => "Identifier",
            }
        );
        quote! { ::aip::FieldBehaviorKind::#variant }
    }
}

/// Which `aip::FieldBehavior` operation a plan is for. They reach nested
/// messages differently: clearing goes through every message field, list and
/// map elements included, but not into a field it clears; validating goes
/// through singular message fields and oneof members only, since a mask cannot
/// address an element, and into a field it checks too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Clear,
    Validate,
}

impl Operation {
    fn method(self) -> &'static str {
        match self {
            Self::Clear => "clear_fields",
            Self::Validate => "validate_fields",
        }
    }

    /// The message `field` leads the operation into, for `behavior`, if any.
    fn edge(self, field: &Field, behavior: Behavior) -> Option<String> {
        match self {
            Self::Clear => {
                if behavior.marks(field) {
                    None
                } else if field.is_map {
                    field.map_value.clone()
                } else if field.kind == Kind::Message {
                    Some(field.type_name.clone())
                } else {
                    None
                }
            }
            Self::Validate => (field.kind == Kind::Message && !field.repeated && !field.is_map)
                .then(|| field.type_name.clone()),
        }
    }
}

/// The messages an operation has work in, per behavior.
///
/// A message has work for a behavior when it has a field carrying it, or when
/// a message it leads into has: both operations recurse, so a message that
/// merely *holds* a marked one still has to pass the call on.
pub struct Plan {
    operation: Operation,
    needed: BTreeMap<Behavior, BTreeSet<String>>,
}

impl Plan {
    fn has(&self, behavior: Behavior, fqn: &str) -> bool {
        self.needed
            .get(&behavior)
            .is_some_and(|needed| needed.contains(fqn))
    }
}

/// Plans `operation` for every behavior over the messages in `generated`.
///
/// # Errors
///
/// When a generated message leads into a message in a file that is not being
/// generated, and that message has fields of a behavior beneath it: it gets no
/// impl, so the operation could not reach them, and silently skipping them
/// would leave a field uncleared or unchecked.
pub fn plan(index: &Index, generated: &BTreeSet<String>, operation: Operation) -> Result<Plan> {
    let candidates: Vec<&Message> = generated
        .iter()
        .flat_map(|file| index.in_file(file))
        .collect();
    let mut needed = BTreeMap::new();
    for behavior in Behavior::ALL {
        check_unreachable(index, generated, operation, behavior)?;
        let mut set: BTreeSet<String> = candidates
            .iter()
            .filter(|message| message.fields.iter().any(|field| behavior.marks(field)))
            .map(|message| message.fqn.clone())
            .collect();
        loop {
            let mut added = false;
            for message in &candidates {
                if set.contains(&message.fqn) {
                    continue;
                }
                if message
                    .fields
                    .iter()
                    .filter_map(|field| reachable(field, index, generated, operation, behavior))
                    .any(|target| set.contains(&target))
                {
                    set.insert(message.fqn.clone());
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        if !set.is_empty() {
            needed.insert(behavior, set);
        }
    }
    Ok(Plan { operation, needed })
}

/// Fails if a generated message leads into a message outside `generated` that
/// has `behavior` fields beneath it.
fn check_unreachable(
    index: &Index,
    generated: &BTreeSet<String>,
    operation: Operation,
    behavior: Behavior,
) -> Result<()> {
    let all: Vec<&Message> = index.all().collect();
    let mut dirty: BTreeSet<String> = all
        .iter()
        .filter(|message| message.fields.iter().any(|field| behavior.marks(field)))
        .map(|message| message.fqn.clone())
        .collect();
    loop {
        let mut added = false;
        for message in &all {
            if dirty.contains(&message.fqn) {
                continue;
            }
            if message
                .fields
                .iter()
                .filter_map(|field| operation.edge(field, behavior))
                .any(|target| dirty.contains(&target))
            {
                dirty.insert(message.fqn.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    for message in generated.iter().flat_map(|file| index.in_file(file)) {
        for field in &message.fields {
            let Some(target) = operation.edge(field, behavior) else {
                continue;
            };
            let Some(held) = index.get(&target) else {
                continue;
            };
            if !generated.contains(&held.source_file) && dirty.contains(&target) {
                bail!(
                    "{}.{}: holds {}, which has {} fields beneath it, but {} is not \
                     being generated, so {} cannot reach them; generate {} too",
                    message.fqn.trim_start_matches('.'),
                    field.name,
                    target.trim_start_matches('.'),
                    behavior.annotation(),
                    held.source_file,
                    operation.method(),
                    held.source_file,
                );
            }
        }
    }
    Ok(())
}

/// The message `field` leads into, when it is generated in this run and so has
/// an impl to call.
fn reachable(
    field: &Field,
    index: &Index,
    generated: &BTreeSet<String>,
    operation: Operation,
    behavior: Behavior,
) -> Option<String> {
    let target = operation.edge(field, behavior)?;
    let message = index.get(&target)?;
    generated.contains(&message.source_file).then_some(target)
}

/// The `clear_fields` and `validate_fields` items for each message declared in
/// `file` that has work, keyed by fully-qualified name.
#[must_use]
pub fn emit_file(
    file: &str,
    index: &Index,
    clear: &Plan,
    validate: &Plan,
    generated: &BTreeSet<String>,
) -> BTreeMap<String, Vec<TokenStream>> {
    let mut items = BTreeMap::new();
    for message in index.in_file(file) {
        let mut methods = Vec::new();
        if let Some(method) = emit_clear(message, index, clear, generated) {
            methods.push(method);
        }
        if let Some(method) = emit_validate(message, index, validate, generated) {
            methods.push(method);
        }
        if !methods.is_empty() {
            items.insert(message.fqn.clone(), methods);
        }
    }
    items
}

/// Dispatches `arms` on `kind`, one branch per distinct body: behaviors whose
/// statements are the same — passing the call on to a nested message, say —
/// share it, as `A | B`. One `if` when there is a single branch, a `match`
/// otherwise.
fn dispatch(arms: Vec<(Behavior, Vec<TokenStream>)>) -> TokenStream {
    let mut branches: Vec<(Vec<Behavior>, Vec<TokenStream>, String)> = Vec::new();
    for (behavior, statements) in arms {
        let key = quote! { #( #statements )* }.to_string();
        match branches
            .iter_mut()
            .find(|(_, _, existing)| *existing == key)
        {
            Some((behaviors, _, _)) => behaviors.push(behavior),
            None => branches.push((vec![behavior], statements, key)),
        }
    }
    if let [(behaviors, statements, _)] = branches.as_slice() {
        let kinds = behaviors.iter().map(|behavior| behavior.kind());
        return quote! {
            if ::core::matches!(kind, #( #kinds )|*) {
                #( #statements )*
            }
        };
    }
    let arms = branches.into_iter().map(|(behaviors, statements, _)| {
        let kinds = behaviors.iter().map(|behavior| behavior.kind());
        quote! { #( #kinds )|* => { #( #statements )* } }
    });
    quote! {
        match kind {
            #( #arms )*
            _ => {}
        }
    }
}

// ── clear_fields ────────────────────────────────────────────────────────────

fn emit_clear(
    message: &Message,
    index: &Index,
    plan: &Plan,
    generated: &BTreeSet<String>,
) -> Option<TokenStream> {
    let arms: Vec<(Behavior, Vec<TokenStream>)> = Behavior::ALL
        .into_iter()
        .filter(|behavior| plan.has(*behavior, &message.fqn))
        .map(|behavior| {
            (
                behavior,
                clear_statements(message, index, plan, generated, behavior),
            )
        })
        .collect();
    if arms.is_empty() {
        return None;
    }
    let body = dispatch(arms);
    Some(quote! {
        #[allow(
            clippy::default_trait_access,
            reason = "the concrete type of each field is buffa's to decide, not this plugin's"
        )]
        #[allow(
            clippy::collapsible_if,
            reason = "collapsing the kind test and a binding takes a let chain, which needs edition 2024"
        )]
        fn clear_fields(&mut self, kind: ::aip::FieldBehaviorKind) {
            #[allow(unused_imports)]
            use ::aip::FieldBehavior as _;
            #body
        }
    })
}

fn clear_statements(
    message: &Message,
    index: &Index,
    plan: &Plan,
    generated: &BTreeSet<String>,
    behavior: Behavior,
) -> Vec<TokenStream> {
    let mut oneofs: BTreeMap<&str, Vec<&Field>> = BTreeMap::new();
    let mut statements = Vec::new();
    for field in &message.fields {
        if let Some(oneof) = &field.oneof {
            oneofs.entry(oneof).or_default().push(field);
            continue;
        }
        if behavior.marks(field) {
            let ident = buffa_codegen::idents::make_field_ident(&field.name);
            statements.push(quote! { self.#ident = ::core::default::Default::default(); });
        } else if let Some(target) = reachable(field, index, generated, plan.operation, behavior)
            && plan.has(behavior, &target)
        {
            statements.push(clear_descend(field));
        }
    }
    for (oneof, fields) in oneofs {
        statements.push(clear_oneof(
            message, oneof, &fields, index, plan, generated, behavior,
        ));
    }
    statements
}

/// Passes the call on to whatever `field` holds.
fn clear_descend(field: &Field) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    if field.is_map {
        quote! {
            for value in self.#ident.values_mut() {
                value.clear_fields(kind);
            }
        }
    } else if field.repeated {
        quote! {
            for item in &mut self.#ident {
                item.clear_fields(kind);
            }
        }
    } else if field.optional {
        quote! {
            if let ::core::option::Option::Some(value) = self.#ident.as_mut() {
                value.clear_fields(kind);
            }
        }
    } else {
        quote! {
            if let ::core::option::Option::Some(value) = self.#ident.as_option_mut() {
                value.clear_fields(kind);
            }
        }
    }
}

/// A oneof's part of a clear: unsets it when the member set carries the
/// behavior, and passes the call on to a member message that does not.
fn clear_oneof(
    message: &Message,
    oneof: &str,
    fields: &[&Field],
    index: &Index,
    plan: &Plan,
    generated: &BTreeSet<String>,
    behavior: Behavior,
) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(oneof);
    let enum_path = oneof_enum(message, oneof);
    let clears: Vec<TokenStream> = fields
        .iter()
        .filter(|field| behavior.marks(field))
        .map(|field| {
            let variant = variant_ident(&field.name);
            quote! { ::core::option::Option::Some(#enum_path::#variant(_)) }
        })
        .collect();
    let descents: Vec<TokenStream> = fields
        .iter()
        .filter(|field| {
            reachable(field, index, generated, plan.operation, behavior)
                .is_some_and(|target| plan.has(behavior, &target))
        })
        .map(|field| {
            let variant = variant_ident(&field.name);
            quote! {
                if let ::core::option::Option::Some(#enum_path::#variant(value)) = &mut self.#ident {
                    value.clear_fields(kind);
                }
            }
        })
        .collect();
    let clear = if clears.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            if ::core::matches!(self.#ident, #( #clears )|*) {
                self.#ident = ::core::option::Option::None;
            }
        }
    };
    quote! {
        #clear
        #( #descents )*
    }
}

// ── validate_fields ─────────────────────────────────────────────────────────

fn emit_validate(
    message: &Message,
    index: &Index,
    plan: &Plan,
    generated: &BTreeSet<String>,
) -> Option<TokenStream> {
    let mut descends = false;
    let arms: Vec<(Behavior, Vec<TokenStream>)> = Behavior::ALL
        .into_iter()
        .filter(|behavior| plan.has(*behavior, &message.fqn))
        .map(|behavior| {
            let (statements, descended) =
                validate_statements(message, index, plan, generated, behavior);
            descends |= descended;
            (behavior, statements)
        })
        .collect();
    if arms.is_empty() {
        return None;
    }
    let body = dispatch(arms);
    // The mask paths beneath a message field, relative to it: the scope that
    // message is checked in when the mask names only fields beneath it.
    let beneath = if descends {
        quote! {
            let beneath = |field: &str| -> ::std::vec::Vec<::std::string::String> {
                match scope {
                    ::aip::FieldScope::All => ::std::vec::Vec::new(),
                    ::aip::FieldScope::Mask(paths) => paths
                        .iter()
                        .filter_map(|path| path.strip_prefix(field)?.strip_prefix('.'))
                        .map(::std::borrow::ToOwned::to_owned)
                        .collect(),
                }
            };
        }
    } else {
        TokenStream::new()
    };
    Some(quote! {
        #[allow(
            clippy::collapsible_if,
            clippy::collapsible_else_if,
            reason = "collapsing a test and a binding takes a let chain, which needs edition 2024"
        )]
        #[allow(
            clippy::too_many_lines,
            reason = "one statement per annotated field, spelled out at generation time"
        )]
        fn validate_fields(
            &self,
            kind: ::aip::FieldBehaviorKind,
            scope: ::aip::FieldScope<'_>,
        ) -> ::core::result::Result<(), ::aip::FieldBehaviorError> {
            #[allow(unused_imports)]
            use ::aip::FieldBehavior as _;
            let mut paths: ::std::vec::Vec<::aip::FieldPath> = ::std::vec::Vec::new();
            #beneath
            #body
            ::aip::FieldBehaviorError::check(kind, paths)
        }
    })
}

/// The statements checking `behavior` on `message`, and whether any of them
/// passes the check on to a nested message.
fn validate_statements(
    message: &Message,
    index: &Index,
    plan: &Plan,
    generated: &BTreeSet<String>,
    behavior: Behavior,
) -> (Vec<TokenStream>, bool) {
    let mut statements = Vec::new();
    let mut descends = false;
    for field in &message.fields {
        let name = Literal::string(&field.name);
        // proto2 `required` is set on every message that parsed.
        if behavior.marks(field) && !field.required {
            let unset = unset(message, field);
            let path = field_path(field);
            statements.push(quote! {
                if scope.contains(#name) && #unset {
                    paths.push(#path);
                }
            });
        }
        if let Some(target) = reachable(field, index, generated, plan.operation, behavior)
            && plan.has(behavior, &target)
        {
            statements.push(validate_descend(message, field, index));
            descends = true;
        }
    }
    (statements, descends)
}

/// The `aip::FieldPath` to `field`, with its name, number and type, as a
/// protovalidate violation path carries them.
fn field_path(field: &Field) -> TokenStream {
    let name = Literal::string(&field.name);
    let number = Literal::u32_unsuffixed(u32::try_from(field.number).unwrap_or_default());
    let ty = format_ident!("{}", field.field_type);
    quote! { ::aip::FieldPath::field(#name, #number, ::aip::FieldType::#ty) }
}

/// Passes the check on to the message `field` holds: in full when the scope
/// contains the field itself, set; or, when the mask names only fields beneath
/// it, over those — on the message as held, or as its default when unset, since
/// the update writes those fields either way.
fn validate_descend(message: &Message, field: &Field, index: &Index) -> TokenStream {
    let name = Literal::string(&field.name);
    let path = field_path(field);
    let absorb = |value: TokenStream, scope: TokenStream| {
        quote! {
            if let ::core::result::Result::Err(error) = #value.validate_fields(kind, #scope) {
                paths.extend(error.prefixed(&#path).paths);
            }
        }
    };
    let all = quote! { ::aip::FieldScope::All };
    let masked = quote! { ::aip::FieldScope::Mask(&beneath) };
    if let Some(oneof) = &field.oneof {
        let ident = buffa_codegen::idents::make_field_ident(oneof);
        let enum_path = oneof_enum(message, oneof);
        let variant = variant_ident(&field.name);
        let set_all = absorb(quote! { value }, all);
        let set_masked = absorb(quote! { value }, masked.clone());
        // An unset member has no value to check; its default does, when the
        // member type is this package's to name.
        let unset = index
            .get(&field.type_name)
            .filter(|held| held.package == message.package)
            .map(|held| {
                let path: TokenStream = held
                    .rust_path
                    .parse()
                    .expect("a message path built from proto identifiers is a valid Rust path");
                let check = absorb(
                    quote! { <#path as ::buffa::DefaultInstance>::default_instance() },
                    masked.clone(),
                );
                quote! {
                    else if !scope.contains(#name) {
                        let beneath = beneath(#name);
                        if !beneath.is_empty() {
                            #check
                        }
                    }
                }
            })
            .unwrap_or_default();
        return quote! {
            if let ::core::option::Option::Some(#enum_path::#variant(value)) = &self.#ident {
                if scope.contains(#name) {
                    #set_all
                } else {
                    let beneath = beneath(#name);
                    if !beneath.is_empty() {
                        #set_masked
                    }
                }
            } #unset
        };
    }
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    let set_all = absorb(quote! { value }, all);
    let held = absorb(quote! { (*self.#ident) }, masked);
    quote! {
        if scope.contains(#name) {
            if let ::core::option::Option::Some(value) = self.#ident.as_option() {
                #set_all
            }
        } else {
            let beneath = beneath(#name);
            if !beneath.is_empty() {
                #held
            }
        }
    }
}

/// The test for `field` being unset on `self`, as protobuf reads presence.
fn unset(message: &Message, field: &Field) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    if let Some(oneof) = &field.oneof {
        let oneof_ident = buffa_codegen::idents::make_field_ident(oneof);
        let enum_path = oneof_enum(message, oneof);
        let variant = variant_ident(&field.name);
        return quote! {
            !::core::matches!(self.#oneof_ident, ::core::option::Option::Some(#enum_path::#variant(_)))
        };
    }
    if field.repeated || field.is_map {
        return quote! { self.#ident.is_empty() };
    }
    if field.optional {
        return quote! { self.#ident.is_none() };
    }
    match field.kind {
        Kind::Message => quote! { self.#ident.as_option().is_none() },
        Kind::Double => quote! { self.#ident.to_bits() == 0 },
        Kind::String | Kind::Bytes => quote! { self.#ident.is_empty() },
        Kind::Bool => quote! { !self.#ident },
        Kind::Integer => quote! { self.#ident == 0 },
        Kind::Enum => quote! { self.#ident.to_i32() == 0 },
    }
}

fn oneof_enum(message: &Message, oneof: &str) -> TokenStream {
    format!(
        "{}::{}",
        message.oneof_module,
        buffa_codegen::idents::to_upper_camel_case(oneof),
    )
    .parse()
    .expect("a oneof path built from proto identifiers is a valid Rust path")
}

fn variant_ident(field: &str) -> proc_macro2::Ident {
    format_ident!("{}", buffa_codegen::idents::to_upper_camel_case(field))
}

#[cfg(test)]
mod tests {
    use buffa::ExtensionSet;
    use buffa_codegen::generated::{
        compiler::CodeGeneratorRequest,
        descriptor::{
            DescriptorProto, FieldDescriptorProto, FieldOptions, FileDescriptorProto,
            field_descriptor_proto::{Label, Type},
        },
    };

    use super::*;
    use crate::annotations::google::api::{FIELD_BEHAVIOR, FieldBehavior};

    /// `api.Thing` holds a `dep.Audit` from `dep.proto`, which is not being
    /// generated, whose one field carries `behavior`.
    fn request(behavior: Option<FieldBehavior>) -> CodeGeneratorRequest {
        let mut options = FieldOptions::default();
        if let Some(behavior) = behavior {
            options.set_extension(&FIELD_BEHAVIOR, vec![behavior as i32]);
        }
        let audit = FileDescriptorProto {
            name: Some("dep.proto".to_owned()),
            package: Some("dep".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Audit".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("create_time".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_STRING),
                    label: Some(Label::LABEL_OPTIONAL),
                    options: options.into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let thing = FileDescriptorProto {
            name: Some("api.proto".to_owned()),
            package: Some("api".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Thing".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("audit".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_MESSAGE),
                    type_name: Some(".dep.Audit".to_owned()),
                    label: Some(Label::LABEL_OPTIONAL),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        CodeGeneratorRequest {
            file_to_generate: vec!["api.proto".to_owned()],
            proto_file: vec![audit, thing],
            ..Default::default()
        }
    }

    fn plan_for(request: &CodeGeneratorRequest, operation: Operation) -> Result<Plan> {
        let index = crate::messages::gather(request);
        let generated = request.file_to_generate.iter().cloned().collect();
        plan(&index, &generated, operation)
    }

    #[test]
    fn an_unreachable_field_to_clear_fails_generation() {
        let error = plan_for(&request(Some(FieldBehavior::OUTPUT_ONLY)), Operation::Clear)
            .err()
            .expect("an unreachable OUTPUT_ONLY field");
        assert_eq!(
            error.to_string(),
            "api.Thing.audit: holds dep.Audit, which has OUTPUT_ONLY fields beneath it, but \
             dep.proto is not being generated, so clear_fields cannot reach them; \
             generate dep.proto too"
        );
    }

    #[test]
    fn an_unreachable_field_to_validate_fails_generation() {
        let error = plan_for(&request(Some(FieldBehavior::REQUIRED)), Operation::Validate)
            .err()
            .expect("an unreachable REQUIRED field");
        assert_eq!(
            error.to_string(),
            "api.Thing.audit: holds dep.Audit, which has REQUIRED fields beneath it, but \
             dep.proto is not being generated, so validate_fields cannot reach them; \
             generate dep.proto too"
        );
    }

    #[test]
    fn a_dependency_with_nothing_annotated_is_fine() {
        assert!(plan_for(&request(None), Operation::Clear).is_ok());
        assert!(plan_for(&request(None), Operation::Validate).is_ok());
    }

    #[test]
    fn a_plan_records_only_the_behaviors_present() {
        let mut request = request(Some(FieldBehavior::INPUT_ONLY));
        request.file_to_generate.push("dep.proto".to_owned());
        let plan = plan_for(&request, Operation::Clear).expect("everything is generated");
        assert!(plan.has(Behavior::InputOnly, ".dep.Audit"));
        assert!(plan.has(Behavior::InputOnly, ".api.Thing"));
        assert!(!plan.has(Behavior::OutputOnly, ".api.Thing"));
    }
}
