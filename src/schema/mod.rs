//! JSON Schema generation for the MCP tool surface.
//!
//! The cache module holds the base schemas for `ToolInput` and `ToolOutput`;
//! the types module exposes [`SchemaType`] for coarse flag-type classification
//! and `Config.flag_type_overrides`.
//!
//! The orchestrator functions at the module root ([`build_input_schema_with_matchers`],
//! [`build_output_schema`], [`build_description`]) tie the sub-modules
//! together into per-tool schemas and description strings ready for the MCP
//! tool surface.

// `args` is crate-internal — the orchestrator consumes it to build
// the `args` property description for per-tool input schemas.
// `cache` is crate-internal — only the per-tool orchestrator consumes it.
// `types` is public because consumers reference `SchemaType` via
// `Config.flag_type_overrides`.
pub(crate) mod args;
pub(crate) mod cache;
pub(crate) mod flag;
pub mod types;

pub use types::SchemaType;

use std::sync::Arc;

use clap::Command;
use rmcp::model::JsonObject;
use serde_json::Value;

use crate::config::Config;
use crate::selector::FlagMatcher;

/// Build the per-tool input schema for `cmd`, applying optional selector
/// flag matchers.
///
/// `local_flag` and `inherited_flag` are sourced from the selector that
/// claimed this command in the first-match-wins evaluation; pass `None`
/// when no selector is active (which includes all flags).
///
/// The base schema (derived from `ToolInput`'s `JsonSchema` impl) is cloned;
/// the `flags` and `args` property objects are then populated with per-flag
/// JSON Schema entries and the positional-args description respectively.
///
/// The returned `JsonObject` is wrapped in [`Arc`] for direct assignment to
/// `rmcp::model::Tool::input_schema`.
pub(crate) fn build_input_schema_with_matchers(
    cmd: &Command,
    cfg: &Config,
    cmd_path: &str,
    local_flag: Option<&FlagMatcher>,
    inherited_flag: Option<&FlagMatcher>,
) -> Arc<JsonObject> {
    let mut schema = cache::fresh_tool_input_schema();

    if let Some(Value::Object(properties_root)) = schema.get_mut("properties") {
        if let Some(Value::Object(flags_obj)) = properties_root.get_mut("flags") {
            let (properties, required) =
                flag::build_flags_schema(cmd, cfg, cmd_path, local_flag, inherited_flag);

            // Replace the schemars-emitted generic shape with brontes's
            // per-flag concrete object.  `additionalProperties: false` means
            // the MCP layer will reject any unknown flag name before it
            // reaches the spawned CLI process.
            flags_obj.clear();
            flags_obj.insert("type".into(), Value::String("object".into()));
            flags_obj.insert("properties".into(), Value::Object(properties));
            if !required.is_empty() {
                flags_obj.insert(
                    "required".into(),
                    Value::Array(required.into_iter().map(Value::String).collect()),
                );
            }
            flags_obj.insert("additionalProperties".into(), Value::Bool(false));
        }

        if let Some(Value::Object(args_obj)) = properties_root.get_mut("args") {
            args_obj.insert(
                "description".into(),
                Value::String(args::args_description(cmd)),
            );
        }
    }

    Arc::new(schema)
}

/// Build the per-tool output schema.
///
/// The `ToolOutput` shape (`stdout` / `stderr` / `exit_code`) does not vary
/// per command; this function shares the single cached [`Arc`] allocation
/// across every tool (via `Arc::clone`), rather than allocating fresh.
pub(crate) fn build_output_schema() -> Arc<JsonObject> {
    // Output shape is invariant per tool — share the single cached
    // Arc allocation across every Tool's output_schema field.
    Arc::clone(&cache::TOOL_OUTPUT_BASE_SCHEMA)
}

/// Build the per-tool description string.
///
/// Resolution order:
/// 1. `cmd.get_long_about()` if set.
/// 2. `cmd.get_about()` if set.
/// 3. Fallback: `"Execute the {name} command"`.
///
/// If `cmd.get_after_help()` is set and non-empty, a blank line followed by
/// `"Examples:"` and the after-help text is appended to the description.
pub(crate) fn build_description(cmd: &Command) -> String {
    let name = cmd.get_name();
    let main = cmd
        .get_long_about()
        .or_else(|| cmd.get_about())
        .map_or_else(
            || format!("Execute the {name} command"),
            ToString::to_string,
        );

    let mut out = main;

    if let Some(after) = cmd.get_after_help() {
        let after_str = after.to_string();
        if !after_str.trim().is_empty() {
            out.push_str("\n\nExamples:\n");
            out.push_str(after_str.trim_end());
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use clap::{Arg, Command};

    use super::*;
    use crate::config::Config;

    // -----------------------------------------------------------------------
    // build_input_schema tests
    // -----------------------------------------------------------------------

    #[test]
    fn input_schema_has_flags_and_args_properties() {
        let cmd = Command::new("test").arg(Arg::new("foo").long("foo"));
        let cfg = Config::default();
        let schema = build_input_schema_with_matchers(&cmd, &cfg, "test", None, None);
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("schema must have properties");
        assert!(props.contains_key("flags"), "must have 'flags' property");
        assert!(props.contains_key("args"), "must have 'args' property");
    }

    #[test]
    fn input_schema_flags_object_has_per_flag_property() {
        let cmd = Command::new("test").arg(Arg::new("foo").long("foo"));
        let cfg = Config::default();
        let schema = build_input_schema_with_matchers(&cmd, &cfg, "test", None, None);
        let flags_props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .and_then(|p| p.get("flags"))
            .and_then(|v| v.as_object())
            .and_then(|f| f.get("properties"))
            .and_then(|v| v.as_object())
            .expect("flags.properties must be an object");
        assert!(
            flags_props.contains_key("foo"),
            "flags.properties must contain 'foo'"
        );
    }

    #[test]
    fn input_schema_flags_object_is_additional_properties_false() {
        let cmd = Command::new("test").arg(Arg::new("foo").long("foo"));
        let cfg = Config::default();
        let schema = build_input_schema_with_matchers(&cmd, &cfg, "test", None, None);
        let flags_obj = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .and_then(|p| p.get("flags"))
            .and_then(|v| v.as_object())
            .expect("flags must be an object");
        assert_eq!(
            flags_obj.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "flags.additionalProperties must be false"
        );
    }

    #[test]
    fn output_schema_matches_cache() {
        // Behavioral assertion: output schema has the expected ToolOutput
        // properties.
        let s1 = build_output_schema();
        let props = s1
            .get("properties")
            .and_then(|v| v.as_object())
            .expect("output schema must have properties");
        assert!(props.contains_key("stdout"), "must have 'stdout'");
        assert!(props.contains_key("stderr"), "must have 'stderr'");
        assert!(props.contains_key("exit_code"), "must have 'exit_code'");

        // Allocation-sharing assertion: build_output_schema returns the SAME
        // Arc allocation each call (Arc::clone of the cached static).
        let s2 = build_output_schema();
        assert!(
            Arc::ptr_eq(&s1, &s2),
            "build_output_schema must Arc::clone the cached static, not allocate fresh"
        );
    }

    #[test]
    fn args_description_is_spliced_into_args_property() {
        let cmd = Command::new("test").arg(Arg::new("file").required(true));
        let cfg = Config::default();
        let schema = build_input_schema_with_matchers(&cmd, &cfg, "test", None, None);
        let args_desc = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .and_then(|p| p.get("args"))
            .and_then(|v| v.as_object())
            .and_then(|a| a.get("description"))
            .and_then(|v| v.as_str())
            .expect("args.description must be a string");
        assert!(
            args_desc.contains("Positional command line arguments"),
            "args.description must contain the canonical phrase"
        );
    }

    // -----------------------------------------------------------------------
    // build_description tests
    // -----------------------------------------------------------------------

    #[test]
    fn description_uses_long_about_when_present() {
        let cmd = Command::new("test").long_about("Long form description");
        let desc = build_description(&cmd);
        assert!(
            desc.starts_with("Long form description"),
            "must use long_about: {desc:?}"
        );
    }

    #[test]
    fn description_falls_back_to_about_then_default() {
        // Long set → uses long.
        let cmd_long = Command::new("test")
            .about("Short description")
            .long_about("Long description");
        assert!(
            build_description(&cmd_long).starts_with("Long description"),
            "long_about takes precedence"
        );

        // Only short set → uses short.
        let cmd_short = Command::new("test").about("Short only");
        assert_eq!(build_description(&cmd_short), "Short only");

        // Neither set → default fallback.
        let cmd_neither = Command::new("test");
        assert_eq!(build_description(&cmd_neither), "Execute the test command");
    }

    #[test]
    fn description_appends_examples_block() {
        let cmd = Command::new("test")
            .about("Does a thing")
            .after_help("example one\nexample two");
        let desc = build_description(&cmd);
        assert!(
            desc.contains("\n\nExamples:\nexample one\nexample two"),
            "expected examples block, got: {desc:?}"
        );
    }

    #[test]
    fn description_no_examples_block_when_after_help_empty() {
        // Explicit empty string.
        let cmd_empty = Command::new("test").about("Does a thing").after_help("");
        assert!(
            !build_description(&cmd_empty).contains("Examples:"),
            "empty after_help must not produce an Examples block"
        );

        // No after_help at all.
        let cmd_none = Command::new("test").about("Does a thing");
        assert!(
            !build_description(&cmd_none).contains("Examples:"),
            "absent after_help must not produce an Examples block"
        );
    }

    #[test]
    fn description_examples_block_no_trailing_blank_line() {
        let cmd = Command::new("test")
            .about("Does a thing")
            .after_help("my example\n\n");
        let desc = build_description(&cmd);
        // trim_end on after_str means the block ends without a trailing newline.
        assert!(
            !desc.ends_with('\n'),
            "description must not end with a trailing newline: {desc:?}"
        );
    }
}
