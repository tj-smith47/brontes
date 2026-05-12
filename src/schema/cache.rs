//! Base-schema cache for [`ToolInput`] and [`ToolOutput`].
//!
//! Each static is initialised once on first read via [`std::sync::LazyLock`].
//! Per-tool customisation should call [`fresh_tool_input_schema`] or
//! [`fresh_tool_output_schema`] to obtain an independent clone to mutate,
//! rather than touching the cached [`Arc`] directly.

use std::sync::{Arc, LazyLock};

use rmcp::model::JsonObject;

/// Cached base schema for [`crate::tool::ToolInput`].
///
/// The value is computed once and then shared; callers that need a mutable
/// copy should use [`fresh_tool_input_schema`].
// Task 11 (per-tool orchestrator) will be the first external consumer.
#[allow(dead_code)]
pub(crate) static TOOL_INPUT_BASE_SCHEMA: LazyLock<Arc<JsonObject>> =
    LazyLock::new(|| Arc::new(build_schema::<crate::tool::ToolInput>()));

/// Cached base schema for [`crate::tool::ToolOutput`].
///
/// The value is computed once and then shared; callers that need a mutable
/// copy should use [`fresh_tool_output_schema`].
// Task 11 (per-tool orchestrator) will be the first external consumer.
#[allow(dead_code)]
pub(crate) static TOOL_OUTPUT_BASE_SCHEMA: LazyLock<Arc<JsonObject>> =
    LazyLock::new(|| Arc::new(build_schema::<crate::tool::ToolOutput>()));

/// Generate the root JSON Schema for `T` and return it as a [`JsonObject`].
///
/// # Panics
///
/// Panics (via `unreachable!`) if `schemars` produces a non-Object root for a
/// `#[derive(JsonSchema)]` struct.  That cannot happen for any well-formed
/// struct — the `JsonSchema` derive contract guarantees an Object root with
/// `"properties"`.  If this ever fires it is a bug in schemars, not in brontes.
// Only called from the two LazyLock initialisers above; suppress false positive.
#[allow(dead_code)]
fn build_schema<T: schemars::JsonSchema>() -> JsonObject {
    let schema = schemars::schema_for!(T);
    match schema.to_value() {
        serde_json::Value::Object(map) => map,
        other => unreachable!(
            "schemars produced a non-Object root for a JsonSchema-derived struct: {other:?}"
        ),
    }
}

/// Return a fresh [`JsonObject`] clone of the `ToolInput` base schema.
///
/// Each call returns an independent copy.  Mutating the returned map does not
/// affect the cached singleton; wrap the result in [`Arc::new`] before handing
/// it to `rmcp`.
// Task 11 (per-tool orchestrator) will be the first external consumer.
#[allow(dead_code)]
pub(crate) fn fresh_tool_input_schema() -> JsonObject {
    (**TOOL_INPUT_BASE_SCHEMA).clone()
}

/// Return a fresh [`JsonObject`] clone of the `ToolOutput` base schema.
///
/// Each call returns an independent copy.  Mutating the returned map does not
/// affect the cached singleton; wrap the result in [`Arc::new`] before handing
/// it to `rmcp`.
// Task 11 (per-tool orchestrator) will be the first external consumer.
#[allow(dead_code)]
pub(crate) fn fresh_tool_output_schema() -> JsonObject {
    (**TOOL_OUTPUT_BASE_SCHEMA).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_input_schema_is_object_with_properties() {
        let schema = fresh_tool_input_schema();
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("ToolInput schema must have a 'properties' object");
        assert!(props.contains_key("flags"), "must have 'flags' property");
        assert!(props.contains_key("args"), "must have 'args' property");
    }

    #[test]
    fn tool_output_schema_is_object_with_properties() {
        let schema = fresh_tool_output_schema();
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("ToolOutput schema must have a 'properties' object");
        assert!(props.contains_key("stdout"), "must have 'stdout' property");
        assert!(props.contains_key("stderr"), "must have 'stderr' property");
        assert!(
            props.contains_key("exit_code"),
            "must have 'exit_code' property"
        );
    }

    #[test]
    fn fresh_tool_input_schema_is_independent() {
        let mut fresh = fresh_tool_input_schema();
        fresh.insert("test_key".into(), serde_json::Value::Bool(true));

        let second = fresh_tool_input_schema();
        assert!(
            !second.contains_key("test_key"),
            "mutating a fresh clone must not affect the cache"
        );
    }

    #[test]
    fn cache_returns_same_arc_for_repeated_reads() {
        let first = Arc::clone(&TOOL_INPUT_BASE_SCHEMA);
        let second = Arc::clone(&TOOL_INPUT_BASE_SCHEMA);
        assert!(
            Arc::ptr_eq(&first, &second),
            "repeated reads must return the same Arc allocation"
        );
    }
}
