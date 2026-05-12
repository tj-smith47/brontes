//! Coarse JSON Schema type classification for clap flags.
//!
//! [`SchemaType`] is the value type for `Config.flag_type_overrides` and is
//! used by the flag-to-schema mapper to determine the `type` keyword in each
//! per-flag schema object.

use std::any::TypeId;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Coarse JSON Schema type for a clap flag.
///
/// Used as the `type` field in the per-flag schema, and as the value of
/// `Config.flag_type_overrides` when brontes cannot introspect a parser's
/// type.
///
/// Note: there is no `Enum` variant — enum values are represented as
/// `"string"` on the wire; the flag mapper adds an `enum:` keyword to the
/// property schema separately while keeping `type` as `"string"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaType {
    /// JSON Schema `boolean`.
    Boolean,
    /// JSON Schema `integer`.
    Integer,
    /// JSON Schema `number` (floating-point).
    Number,
    /// JSON Schema `string`.
    String,
    /// JSON Schema `string` with `format: "path"`. `as_json_type()` returns
    /// `"string"`; the per-flag schema mapper injects `format: "path"`
    /// separately into the property schema.
    StringPath,
    /// JSON Schema `array`.
    Array,
    /// JSON Schema `object`.
    Object,
}

impl SchemaType {
    /// Render this variant as the JSON Schema `type` keyword value.
    ///
    /// `StringPath` returns `"string"` per the JSON Schema convention; the
    /// `format: "path"` annotation is added separately by the flag mapper.
    #[must_use]
    pub fn as_json_type(self) -> &'static str {
        match self {
            SchemaType::Boolean => "boolean",
            SchemaType::Integer => "integer",
            SchemaType::Number => "number",
            SchemaType::String | SchemaType::StringPath => "string",
            SchemaType::Array => "array",
            SchemaType::Object => "object",
        }
    }
}

/// All `value_parser!` target types brontes recognises, paired with the
/// [`SchemaType`] they classify to. Single source of truth for the
/// clap-`AnyValueId` classifier in `schema::flag`.
///
/// Covered types:
/// - `i8`, `i16`, `i32`, `i64`, `isize`, `u8`, `u16`, `u32`, `u64`, `usize` → [`SchemaType::Integer`]
/// - `f32`, `f64` → [`SchemaType::Number`]
/// - `bool` → [`SchemaType::Boolean`]
/// - [`String`] → [`SchemaType::String`]
/// - [`PathBuf`], [`OsString`] → [`SchemaType::StringPath`]
pub(crate) fn known_type_classifications() -> &'static [(TypeId, SchemaType)] {
    static TABLE: OnceLock<Vec<(TypeId, SchemaType)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        vec![
            (TypeId::of::<i8>(), SchemaType::Integer),
            (TypeId::of::<i16>(), SchemaType::Integer),
            (TypeId::of::<i32>(), SchemaType::Integer),
            (TypeId::of::<i64>(), SchemaType::Integer),
            (TypeId::of::<isize>(), SchemaType::Integer),
            (TypeId::of::<u8>(), SchemaType::Integer),
            (TypeId::of::<u16>(), SchemaType::Integer),
            (TypeId::of::<u32>(), SchemaType::Integer),
            (TypeId::of::<u64>(), SchemaType::Integer),
            (TypeId::of::<usize>(), SchemaType::Integer),
            (TypeId::of::<f32>(), SchemaType::Number),
            (TypeId::of::<f64>(), SchemaType::Number),
            (TypeId::of::<bool>(), SchemaType::Boolean),
            (TypeId::of::<String>(), SchemaType::String),
            (TypeId::of::<PathBuf>(), SchemaType::StringPath),
            (TypeId::of::<OsString>(), SchemaType::StringPath),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_json_type_renders_correctly() {
        assert_eq!(SchemaType::Boolean.as_json_type(), "boolean");
        assert_eq!(SchemaType::Integer.as_json_type(), "integer");
        assert_eq!(SchemaType::Number.as_json_type(), "number");
        assert_eq!(SchemaType::String.as_json_type(), "string");
        // StringPath must return "string", not "path"
        assert_eq!(SchemaType::StringPath.as_json_type(), "string");
        assert_eq!(SchemaType::Array.as_json_type(), "array");
        assert_eq!(SchemaType::Object.as_json_type(), "object");
    }
}
