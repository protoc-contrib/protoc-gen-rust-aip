//! An index of every message and method in the request, in the shape the
//! field-behavior and query passes need.
//!
//! [`scan`](crate::scan) answers questions about resource *names*, which are
//! declared by annotations. These two passes ask about message *structure* —
//! what a field holds, which message a method returns — so they share an index
//! of that instead.

use std::collections::BTreeMap;

use buffa::ExtensionSet;
use buffa_codegen::generated::{
    compiler::CodeGeneratorRequest,
    descriptor::{
        DescriptorProto, FieldDescriptorProto, FieldOptions, FileDescriptorProto,
        field_descriptor_proto,
    },
};

use crate::annotations::google::api::{FIELD_BEHAVIOR, FieldBehavior};

/// What a field holds, to the resolution these passes care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A `string`.
    String,
    /// A `bool`.
    Bool,
    /// Any signed or unsigned integer type. CEL models them all as `int`.
    Integer,
    /// A `float` or `double`.
    Double,
    /// A `bytes`.
    Bytes,
    /// An enum. CEL models it as an integer, which is also how a column stores
    /// it.
    Enum,
    /// A message, named by [`Field::type_name`].
    Message,
}

/// One field of a message.
///
/// The flags are separate rather than folded into `kind` because they are
/// independent: a field can be repeated *and* a message, optional *and* in a
/// oneof, and the emitters ask about one at a time.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent descriptor bit, not a state machine"
)]
#[derive(Debug, Clone)]
pub struct Field {
    /// The proto field name, e.g. `create_time`.
    pub name: String,
    pub kind: Kind,
    /// The fully-qualified type name for a message or enum field, else empty.
    pub type_name: String,
    /// Whether the field is `repeated`. A map field is repeated too — see
    /// [`Field::map_value`].
    pub repeated: bool,
    /// Whether the field has explicit presence and so is an `Option<T>`.
    pub proto3_optional: bool,
    /// The name of the real `oneof` this field belongs to, if any. A proto3
    /// `optional` field sits in a synthetic oneof, which is not one of these.
    pub oneof: Option<String>,
    /// Annotated `google.api.field_behavior = OUTPUT_ONLY`.
    pub output_only: bool,
    /// For a map field, the fully-qualified type of its value if that value is
    /// a message, else `None`. A map is a repeated field of a generated entry
    /// message, which nothing should ever see.
    pub map_value: Option<String>,
    /// Whether the field is a map.
    pub is_map: bool,
}

/// One message.
#[derive(Debug, Clone)]
pub struct Message {
    /// The fully-qualified proto name, e.g. `.example.v1.Book`.
    pub fqn: String,
    /// The path within its package's module, e.g. `Book` or `outer::Inner`.
    pub rust_path: String,
    /// The proto package, e.g. `example.v1`.
    pub package: String,
    /// The file the message is declared in.
    pub source_file: String,
    /// The module buffa puts this message's oneof enums in, e.g. `event` for a
    /// top-level `Event`, `outer::inner` for a nested `Outer.Inner`.
    pub oneof_module: String,
    /// Whether this is the synthetic entry message of a map field.
    pub is_map_entry: bool,
    pub fields: Vec<Field>,
}

/// One RPC, reduced to the two messages that say whether it is a List.
#[derive(Debug, Clone)]
pub struct Method {
    /// The fully-qualified request message name.
    pub input: String,
    /// The fully-qualified response message name.
    pub output: String,
}

/// Every message and method in the request.
#[derive(Debug, Default)]
pub struct Index {
    messages: BTreeMap<String, Message>,
    /// In declaration order, so generation does not depend on map order.
    pub methods: Vec<Method>,
}

impl Index {
    /// The message with this fully-qualified name.
    #[must_use]
    pub fn get(&self, fqn: &str) -> Option<&Message> {
        self.messages.get(fqn)
    }

    /// Every message declared in `file`, in declaration order.
    pub fn in_file<'a>(&'a self, file: &str) -> impl Iterator<Item = &'a Message> {
        // BTreeMap orders by FQN, which for messages of one file is declaration
        // order for top-level messages and puts each nested message directly
        // after its parent. Either way it is stable.
        self.messages
            .values()
            .filter(move |message| message.source_file == file && !message.is_map_entry)
    }
}

/// Builds the index from every file in the request.
#[must_use]
pub fn gather(request: &CodeGeneratorRequest) -> Index {
    let mut index = Index::default();
    for file in &request.proto_file {
        walk_file(file, &mut index);
    }
    // Map value types are only resolvable once every entry message is indexed.
    resolve_maps(&mut index);
    index
}

fn walk_file(file: &FileDescriptorProto, index: &mut Index) {
    let package = file.package.clone().unwrap_or_default();
    let source_file = file.name.clone().unwrap_or_default();
    for message in &file.message_type {
        walk_message(message, &[], &package, &source_file, index);
    }
    for service in &file.service {
        for method in &service.method {
            let (Some(input), Some(output)) =
                (method.input_type.clone(), method.output_type.clone())
            else {
                continue;
            };
            index.methods.push(Method { input, output });
        }
    }
}

fn walk_message(
    message: &DescriptorProto,
    parents: &[&str],
    package: &str,
    source_file: &str,
    index: &mut Index,
) {
    let name = message.name.as_deref().unwrap_or_default();
    let mut fqn = String::from(".");
    if !package.is_empty() {
        fqn.push_str(package);
        fqn.push('.');
    }
    for parent in parents {
        fqn.push_str(parent);
        fqn.push('.');
    }
    fqn.push_str(name);

    let oneofs: Vec<&str> = message
        .oneof_decl
        .iter()
        .map(|oneof| oneof.name.as_deref().unwrap_or_default())
        .collect();

    let fields = message
        .field
        .iter()
        .map(|field| build_field(field, &oneofs))
        .collect();

    let is_map_entry = message
        .options
        .as_option()
        .and_then(|options| options.map_entry)
        .unwrap_or(false);

    index.messages.insert(
        fqn.clone(),
        Message {
            fqn,
            rust_path: rust_path(parents, name),
            package: package.to_owned(),
            source_file: source_file.to_owned(),
            oneof_module: oneof_module(parents, name),
            is_map_entry,
            fields,
        },
    );

    let mut nested = parents.to_vec();
    nested.push(name);
    for child in &message.nested_type {
        walk_message(child, &nested, package, source_file, index);
    }
}

fn build_field(field: &FieldDescriptorProto, oneofs: &[&str]) -> Field {
    use field_descriptor_proto::{Label, Type};

    let kind = match field.r#type {
        Some(Type::TYPE_STRING) => Kind::String,
        Some(Type::TYPE_BOOL) => Kind::Bool,
        Some(Type::TYPE_BYTES) => Kind::Bytes,
        Some(Type::TYPE_FLOAT | Type::TYPE_DOUBLE) => Kind::Double,
        Some(Type::TYPE_ENUM) => Kind::Enum,
        Some(Type::TYPE_MESSAGE | Type::TYPE_GROUP) => Kind::Message,
        // Every remaining scalar is an integer of some width. CEL has one
        // integer type, and so does a database column.
        _ => Kind::Integer,
    };

    let proto3_optional = field.proto3_optional.unwrap_or(false);
    // A proto3 `optional` field is placed in a synthetic one-member oneof.
    // Reporting it as a oneof would make generated code match on an enum buffa
    // does not emit for it.
    let oneof = if proto3_optional {
        None
    } else {
        field
            .oneof_index
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| oneofs.get(index))
            .map(|name| (*name).to_owned())
    };

    Field {
        name: field.name.clone().unwrap_or_default(),
        kind,
        type_name: field.type_name.clone().unwrap_or_default(),
        repeated: field.label == Some(Label::LABEL_REPEATED),
        proto3_optional,
        oneof,
        output_only: has_behavior(field, FieldBehavior::OUTPUT_ONLY),
        map_value: None,
        is_map: false,
    }
}

/// Marks each map field and records the type of its value.
///
/// A map is a repeated field of a synthetic entry message, and the difference
/// only shows up in that message's `map_entry` option — which may be indexed
/// after the field that uses it, hence the second pass.
fn resolve_maps(index: &mut Index) {
    let entries: BTreeMap<String, Option<String>> = index
        .messages
        .values()
        .filter(|message| message.is_map_entry)
        .map(|message| {
            let value = message
                .fields
                .iter()
                .find(|field| field.name == "value")
                .filter(|field| field.kind == Kind::Message)
                .map(|field| field.type_name.clone());
            (message.fqn.clone(), value)
        })
        .collect();

    for message in index.messages.values_mut() {
        for field in &mut message.fields {
            if !field.repeated || field.kind != Kind::Message {
                continue;
            }
            if let Some(value) = entries.get(&field.type_name) {
                field.is_map = true;
                field.map_value.clone_from(value);
            }
        }
    }
}

/// Whether `field` carries `want` among its `google.api.field_behavior`
/// annotations.
///
/// The annotation is repeated, so a field may be both `OUTPUT_ONLY` and
/// `IMMUTABLE`; carrying the one being asked about is what counts. The
/// extension decodes as raw `i32`, since an unknown behaviour from a newer
/// annotation release is still a valid wire value.
fn has_behavior(field: &FieldDescriptorProto, want: FieldBehavior) -> bool {
    field
        .options
        .as_option()
        .map(|options: &FieldOptions| options.extension(&FIELD_BEHAVIOR))
        .unwrap_or_default()
        .contains(&(want as i32))
}

/// The path of a message within its package's module. buffa puts a nested
/// message in a module named after its parent, so `Outer.Inner` is
/// `outer::Inner`.
fn rust_path(parents: &[&str], name: &str) -> String {
    let mut path = String::new();
    for parent in parents {
        path.push_str(&module(parent));
        path.push_str("::");
    }
    path.push_str(name);
    path
}

/// The module buffa puts a message's own nested items — including its oneof
/// enums — in: the whole path lowered, this message's name included.
fn oneof_module(parents: &[&str], name: &str) -> String {
    let mut path = String::new();
    for parent in parents {
        path.push_str(&module(parent));
        path.push_str("::");
    }
    path.push_str(&module(name));
    path
}

fn module(name: &str) -> String {
    buffa_codegen::idents::escape_mod_ident(&snake_case(name))
}

/// `PascalCase` to `snake_case`, matching buffa's module naming.
pub(crate) fn snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 2);
    for (index, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && index > 0 {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if previous.is_lowercase() || (previous.is_uppercase() && next_is_lower) {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nests_the_way_buffa_does() {
        assert_eq!(rust_path(&[], "Book"), "Book");
        assert_eq!(rust_path(&["Outer"], "Inner"), "outer::Inner");
        assert_eq!(oneof_module(&[], "Event"), "event");
        assert_eq!(oneof_module(&["Outer"], "Inner"), "outer::inner");
    }
}
