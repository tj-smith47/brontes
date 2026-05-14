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
pub mod args;
pub mod cache;
pub mod flag;
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
pub fn build_input_schema_with_matchers(
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
pub fn build_output_schema() -> Arc<JsonObject> {
    // Output shape is invariant per tool — share the single cached
    // Arc allocation across every Tool's output_schema field.
    Arc::clone(&cache::TOOL_OUTPUT_BASE_SCHEMA)
}

/// Build the per-tool description string.
///
/// Resolution order:
/// 1. If [`Config::descriptions`] has an entry for `cmd_path`, that text is
///    returned verbatim — the `long_about`/`about`/`after_help` cascade is
///    bypassed (the override is the user's wholesale replacement).
/// 2. Otherwise the effective [`crate::config::DescriptionMode`] is resolved
///    via [`Config::description_modes`] (per-path), falling back to
///    [`Config::description_mode`] (global default `Long`).
/// 3. The primary text is built from the resolved mode's cascade:
///    - `Long` (default): `long_about` → fallback `about` → fallback
///      `"Execute the {name} command"`.
///    - `Short`: `about` → fallback `long_about` → fallback
///      `"Execute the {name} command"`.
/// 4. If `cmd.get_after_help()` is set and non-empty after `trim`, a blank
///    line followed by `"Examples:"` and the trimmed after-help text is
///    appended.
pub fn build_description(cmd: &Command, cfg: &Config, cmd_path: &str) -> String {
    // (1) Literal override wins outright — no cascade, no Examples append.
    if let Some(text) = cfg.descriptions.get(cmd_path) {
        return text.clone();
    }

    let name = cmd.get_name();

    // (2) Resolve effective mode: per-path entry wins over the global default.
    let mode = cfg
        .description_modes
        .get(cmd_path)
        .copied()
        .unwrap_or(cfg.description_mode);

    // (3) Primary text — preferred field with the other as fallback.
    let main = {
        let (primary, fallback) = match mode {
            crate::config::DescriptionMode::Long => (cmd.get_long_about(), cmd.get_about()),
            crate::config::DescriptionMode::Short => (cmd.get_about(), cmd.get_long_about()),
        };
        primary.or(fallback).map_or_else(
            || format!("Execute the {name} command"),
            ToString::to_string,
        )
    };

    let mut out = main;

    // (4) Optional Examples block.
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
        let cfg = Config::default();
        let desc = build_description(&cmd, &cfg, "test");
        assert!(
            desc.starts_with("Long form description"),
            "must use long_about: {desc:?}"
        );
    }

    #[test]
    fn description_falls_back_to_about_then_default() {
        let cfg = Config::default();

        // Long set → uses long.
        let cmd_long = Command::new("test")
            .about("Short description")
            .long_about("Long description");
        assert!(
            build_description(&cmd_long, &cfg, "test").starts_with("Long description"),
            "long_about takes precedence"
        );

        // Only short set → uses short.
        let cmd_short = Command::new("test").about("Short only");
        assert_eq!(build_description(&cmd_short, &cfg, "test"), "Short only");

        // Neither set → default fallback.
        let cmd_neither = Command::new("test");
        assert_eq!(
            build_description(&cmd_neither, &cfg, "test"),
            "Execute the test command"
        );
    }

    #[test]
    fn description_appends_examples_block() {
        let cmd = Command::new("test")
            .about("Does a thing")
            .after_help("example one\nexample two");
        let cfg = Config::default();
        let desc = build_description(&cmd, &cfg, "test");
        assert!(
            desc.contains("\n\nExamples:\nexample one\nexample two"),
            "expected examples block, got: {desc:?}"
        );
    }

    #[test]
    fn description_no_examples_block_when_after_help_empty() {
        let cfg = Config::default();

        // Explicit empty string.
        let cmd_empty = Command::new("test").about("Does a thing").after_help("");
        assert!(
            !build_description(&cmd_empty, &cfg, "test").contains("Examples:"),
            "empty after_help must not produce an Examples block"
        );

        // No after_help at all.
        let cmd_none = Command::new("test").about("Does a thing");
        assert!(
            !build_description(&cmd_none, &cfg, "test").contains("Examples:"),
            "absent after_help must not produce an Examples block"
        );
    }

    #[test]
    fn description_examples_block_no_trailing_blank_line() {
        let cmd = Command::new("test")
            .about("Does a thing")
            .after_help("my example\n\n");
        let cfg = Config::default();
        let desc = build_description(&cmd, &cfg, "test");
        // trim_end on after_str means the block ends without a trailing newline.
        assert!(
            !desc.ends_with('\n'),
            "description must not end with a trailing newline: {desc:?}"
        );
    }

    // -----------------------------------------------------------------------
    // build_description tests — per-command mode + literal override
    // -----------------------------------------------------------------------

    #[test]
    fn description_short_mode_prefers_about_over_long_about() {
        let cmd = Command::new("test")
            .about("Short text")
            .long_about("Verbose long-about that wastes context");
        let cfg = Config::default().description_mode(crate::config::DescriptionMode::Short);
        let desc = build_description(&cmd, &cfg, "test");
        assert!(
            desc.starts_with("Short text"),
            "Short mode must prefer about over long_about: {desc:?}"
        );
        assert!(
            !desc.contains("Verbose long-about"),
            "Short mode must not include long_about when about is present: {desc:?}"
        );
    }

    #[test]
    fn description_short_mode_falls_back_to_long_about() {
        let cmd = Command::new("test").long_about("Only long-about set");
        let cfg = Config::default().description_mode(crate::config::DescriptionMode::Short);
        let desc = build_description(&cmd, &cfg, "test");
        assert_eq!(
            desc, "Only long-about set",
            "Short mode must fall back to long_about when about is absent"
        );
    }

    #[test]
    fn description_mode_for_overrides_global_mode() {
        let cmd = Command::new("test")
            .about("Short text")
            .long_about("Long text");
        // Global = Long (default), but per-path override = Short.
        let cfg =
            Config::default().description_mode_for("test", crate::config::DescriptionMode::Short);
        let desc = build_description(&cmd, &cfg, "test");
        assert!(
            desc.starts_with("Short text"),
            "per-path Short override must win over global Long default: {desc:?}"
        );

        // A different path under the same Config still uses the global default.
        let cmd_other = Command::new("other")
            .about("other short")
            .long_about("other long");
        let desc_other = build_description(&cmd_other, &cfg, "other");
        assert!(
            desc_other.starts_with("other long"),
            "non-overridden path must use global Long mode: {desc_other:?}"
        );
    }

    #[test]
    fn description_literal_override_bypasses_cascade_and_examples() {
        let cmd = Command::new("test")
            .long_about("ignored long-about")
            .after_help("ignored example");
        let cfg = Config::default().description("test", "Custom prompt");
        let desc = build_description(&cmd, &cfg, "test");
        assert_eq!(
            desc, "Custom prompt",
            "literal description override must be returned verbatim with no Examples append"
        );
        assert!(
            !desc.contains("Examples:"),
            "literal description override must not append Examples block: {desc:?}"
        );
    }

    #[test]
    fn description_literal_override_wins_over_description_mode_for() {
        let cmd = Command::new("test")
            .about("Short text")
            .long_about("Long text");
        let cfg = Config::default()
            .description_mode_for("test", crate::config::DescriptionMode::Short)
            .description("test", "Literal beats mode");
        let desc = build_description(&cmd, &cfg, "test");
        assert_eq!(
            desc, "Literal beats mode",
            "literal description must win over description_mode_for for the same path"
        );
    }

    #[test]
    fn description_default_mode_preserves_backward_compat() {
        // Config::default() must keep long_about preferred over about and
        // append the Examples block — byte-identical-output contract for
        // existing consumers that rely on the historical description shape.
        let cmd = Command::new("test")
            .about("Short")
            .long_about("Long form")
            .after_help("ex1\nex2");
        let cfg = Config::default();
        let desc = build_description(&cmd, &cfg, "test");
        assert_eq!(
            desc, "Long form\n\nExamples:\nex1\nex2",
            "default mode must produce byte-identical output (backward-compat)"
        );
    }

    #[test]
    fn description_mode_for_overrides_global_mode_long_over_short() {
        // Symmetric to `description_mode_for_overrides_global_mode`:
        // global=Short, per-path=Long, with both `about` and `long_about`
        // set — long_about wins on the overridden path.
        let cmd = Command::new("test")
            .about("Short text")
            .long_about("Long text");
        let cfg = Config::default()
            .description_mode(crate::config::DescriptionMode::Short)
            .description_mode_for("test", crate::config::DescriptionMode::Long);
        let desc = build_description(&cmd, &cfg, "test");
        assert!(
            desc.starts_with("Long text"),
            "per-path Long override must win over global Short default: {desc:?}"
        );

        // A different path under the same Config still uses the global Short.
        let cmd_other = Command::new("other")
            .about("other short")
            .long_about("other long");
        let desc_other = build_description(&cmd_other, &cfg, "other");
        assert!(
            desc_other.starts_with("other short"),
            "non-overridden path must use global Short mode: {desc_other:?}"
        );
    }
}
