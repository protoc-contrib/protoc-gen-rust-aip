//! Which fields buffa generates as `Option<T>`.
//!
//! A generated accessor reads a field the way buffa typed it, so this has to
//! agree with buffa exactly: a field read as a bare `String` that buffa made an
//! `Option<String>` is a compile error in the consumer. buffa keeps its own
//! answer private (`impl_message::is_explicit_presence_scalar`), so this is the
//! same rule, written out:
//!
//! - a message field is a `MessageField`, and a repeated field a `Vec`, never
//!   an `Option`;
//! - a proto3 `optional` field is an `Option`, in any syntax;
//! - a member of a real `oneof` is a variant of the oneof's enum, not an
//!   `Option` of its own;
//! - otherwise **proto2** makes every `optional`-labelled field an `Option`,
//!   **proto3** none, and **editions** the ones whose resolved
//!   `field_presence` is `EXPLICIT` — the default in edition 2023 and later,
//!   overridable on the file, each enclosing message and the field.

use buffa_codegen::generated::descriptor::{
    DescriptorProto, FeatureSet, FieldDescriptorProto, FileDescriptorProto,
    feature_set::FieldPresence, field_descriptor_proto,
};

/// The presence the fields of a scope default to, resolved from the file down
/// through each enclosing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `syntax = "proto2"`, or no syntax at all.
    Proto2,
    /// `syntax = "proto3"`.
    Proto3,
    /// An editions file, with the `field_presence` its fields default to here.
    Editions(FieldPresence),
}

impl Scope {
    /// The scope a file's top-level messages are declared in.
    #[must_use]
    pub fn file(file: &FileDescriptorProto) -> Self {
        match file.syntax.as_deref() {
            Some("proto3") => Self::Proto3,
            Some("editions") => {
                // Every edition so far -- 2023 and 2024 -- defaults to explicit
                // presence; `file.edition` would only matter once one does not.
                let default = FieldPresence::EXPLICIT;
                let features = file
                    .options
                    .as_option()
                    .and_then(|options| options.features.as_option());
                Self::Editions(field_presence(features).unwrap_or(default))
            }
            _ => Self::Proto2,
        }
    }

    /// The scope `message`'s own fields and nested messages are declared in.
    #[must_use]
    pub fn message(self, message: &DescriptorProto) -> Self {
        let Self::Editions(inherited) = self else {
            return self;
        };
        let features = message
            .options
            .as_option()
            .and_then(|options| options.features.as_option());
        Self::Editions(field_presence(features).unwrap_or(inherited))
    }

    /// Whether buffa generates `field`, declared in this scope, as an
    /// `Option<T>`.
    #[must_use]
    pub fn is_option(self, field: &FieldDescriptorProto) -> bool {
        use field_descriptor_proto::{Label, Type};

        if matches!(field.r#type, Some(Type::TYPE_MESSAGE | Type::TYPE_GROUP)) {
            return false;
        }
        if field.label == Some(Label::LABEL_REPEATED) {
            return false;
        }
        if field.proto3_optional.unwrap_or(false) {
            return true;
        }
        if field.oneof_index.is_some() {
            return false;
        }
        match self {
            Self::Proto2 => field.label == Some(Label::LABEL_OPTIONAL),
            Self::Proto3 => false,
            Self::Editions(inherited) => {
                let features = field
                    .options
                    .as_option()
                    .and_then(|options| options.features.as_option());
                field_presence(features).unwrap_or(inherited) == FieldPresence::EXPLICIT
            }
        }
    }
}

/// The `field_presence` a feature set sets, if it sets one.
fn field_presence(features: Option<&FeatureSet>) -> Option<FieldPresence> {
    features
        .and_then(|features| features.field_presence)
        .filter(|presence| *presence != FieldPresence::FIELD_PRESENCE_UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa_codegen::generated::descriptor::{
        Edition, FieldOptions, FileOptions, MessageOptions,
    };
    use field_descriptor_proto::{Label, Type};

    fn scalar(label: Label) -> FieldDescriptorProto {
        FieldDescriptorProto {
            r#type: Some(Type::TYPE_STRING),
            label: Some(label),
            ..Default::default()
        }
    }

    fn features(presence: FieldPresence) -> FeatureSet {
        FeatureSet {
            field_presence: Some(presence),
            ..Default::default()
        }
    }

    fn editions(file_presence: Option<FieldPresence>) -> FileDescriptorProto {
        FileDescriptorProto {
            syntax: Some("editions".to_owned()),
            edition: Some(Edition::EDITION_2023),
            options: file_presence
                .map(|presence| FileOptions {
                    features: features(presence).into(),
                    ..Default::default()
                })
                .into(),
            ..Default::default()
        }
    }

    #[test]
    fn proto3_makes_only_optional_fields_options() {
        let scope = Scope::file(&FileDescriptorProto {
            syntax: Some("proto3".to_owned()),
            ..Default::default()
        });
        assert!(!scope.is_option(&scalar(Label::LABEL_OPTIONAL)));
        let optional = FieldDescriptorProto {
            proto3_optional: Some(true),
            ..scalar(Label::LABEL_OPTIONAL)
        };
        assert!(scope.is_option(&optional));
    }

    #[test]
    fn proto2_makes_every_optional_label_an_option() {
        let scope = Scope::file(&FileDescriptorProto::default());
        assert_eq!(scope, Scope::Proto2);
        assert!(scope.is_option(&scalar(Label::LABEL_OPTIONAL)));
        assert!(!scope.is_option(&scalar(Label::LABEL_REQUIRED)));
        assert!(!scope.is_option(&scalar(Label::LABEL_REPEATED)));
    }

    #[test]
    fn edition_2023_defaults_to_explicit_presence() {
        let scope = Scope::file(&editions(None));
        assert!(scope.is_option(&scalar(Label::LABEL_OPTIONAL)));
    }

    #[test]
    fn presence_is_overridden_file_then_message_then_field() {
        let implicit_file = Scope::file(&editions(Some(FieldPresence::IMPLICIT)));
        assert!(!implicit_file.is_option(&scalar(Label::LABEL_OPTIONAL)));

        let explicit_message = implicit_file.message(&DescriptorProto {
            options: MessageOptions {
                features: features(FieldPresence::EXPLICIT).into(),
                ..Default::default()
            }
            .into(),
            ..Default::default()
        });
        assert!(explicit_message.is_option(&scalar(Label::LABEL_OPTIONAL)));

        let implicit_field = FieldDescriptorProto {
            options: FieldOptions {
                features: features(FieldPresence::IMPLICIT).into(),
                ..Default::default()
            }
            .into(),
            ..scalar(Label::LABEL_OPTIONAL)
        };
        assert!(!explicit_message.is_option(&implicit_field));
    }

    #[test]
    fn messages_and_oneof_members_are_never_options() {
        let scope = Scope::file(&editions(None));
        let message = FieldDescriptorProto {
            r#type: Some(Type::TYPE_MESSAGE),
            ..scalar(Label::LABEL_OPTIONAL)
        };
        assert!(!scope.is_option(&message));
        let member = FieldDescriptorProto {
            oneof_index: Some(0),
            ..scalar(Label::LABEL_OPTIONAL)
        };
        assert!(!scope.is_option(&member));
    }
}
