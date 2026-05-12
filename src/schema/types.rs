//! Coarse JSON Schema type classification for clap flags.
//!
//! [`SchemaType`] is the value type for `Config.flag_type_overrides` and is
//! used by the flag-to-schema mapper to determine the `type` keyword in each
//! per-flag schema object.

use std::any::TypeId;

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

/// Look up the [`SchemaType`] for a known `value_parser!` target type.
///
/// Returns `None` for parser types brontes does not recognise — the flag
/// mapper falls back to [`SchemaType::String`] and emits a
/// `tracing::debug!` in that case.
///
/// Covered types (per PLAN §5.1 mapping table):
/// - `i8`, `i16`, `i32`, `i64`, `isize`, `u8`, `u16`, `u32`, `u64`, `usize` → [`SchemaType::Integer`]
/// - `f32`, `f64` → [`SchemaType::Number`]
/// - `bool` → [`SchemaType::Boolean`]
/// - [`String`] → [`SchemaType::String`]
/// - [`std::path::PathBuf`], [`std::ffi::OsString`] → [`SchemaType::StringPath`]
// Task 9 (flag-to-schema mapper) will be the first external consumer.
#[allow(dead_code)]
pub(crate) fn schema_type_for_type_id(id: TypeId) -> Option<SchemaType> {
    if id == TypeId::of::<i8>()
        || id == TypeId::of::<i16>()
        || id == TypeId::of::<i32>()
        || id == TypeId::of::<i64>()
        || id == TypeId::of::<isize>()
        || id == TypeId::of::<u8>()
        || id == TypeId::of::<u16>()
        || id == TypeId::of::<u32>()
        || id == TypeId::of::<u64>()
        || id == TypeId::of::<usize>()
    {
        Some(SchemaType::Integer)
    } else if id == TypeId::of::<f32>() || id == TypeId::of::<f64>() {
        Some(SchemaType::Number)
    } else if id == TypeId::of::<bool>() {
        Some(SchemaType::Boolean)
    } else if id == TypeId::of::<String>() {
        Some(SchemaType::String)
    } else if id == TypeId::of::<std::path::PathBuf>() || id == TypeId::of::<std::ffi::OsString>() {
        Some(SchemaType::StringPath)
    } else {
        None
    }
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

    #[test]
    fn schema_type_for_type_id_recognized() {
        // signed integers
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<i8>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<i16>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<i32>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<i64>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<isize>()),
            Some(SchemaType::Integer)
        );
        // unsigned integers
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<u8>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<u16>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<u32>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<u64>()),
            Some(SchemaType::Integer)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<usize>()),
            Some(SchemaType::Integer)
        );
        // floats
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<f32>()),
            Some(SchemaType::Number)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<f64>()),
            Some(SchemaType::Number)
        );
        // bool
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<bool>()),
            Some(SchemaType::Boolean)
        );
        // string
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<String>()),
            Some(SchemaType::String)
        );
        // path types
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<std::path::PathBuf>()),
            Some(SchemaType::StringPath)
        );
        assert_eq!(
            schema_type_for_type_id(TypeId::of::<std::ffi::OsString>()),
            Some(SchemaType::StringPath)
        );
    }

    #[test]
    fn schema_type_for_type_id_unknown() {
        struct MyCustomType;
        assert_eq!(schema_type_for_type_id(TypeId::of::<MyCustomType>()), None);
    }
}
