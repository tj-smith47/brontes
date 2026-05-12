//! `clap::Arg` → JSON Schema property mapping.
//!
//! Walks a `clap::Command`'s argument list, applies local-before-inherited
//! dedup, excludes hidden flags, and produces a `(properties, required)`
//! pair ready to splice into the per-tool input schema.

use std::collections::HashSet;

use clap::{Arg, ArgAction, Command};
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::schema::types::{SchemaType, known_type_classifications};
use crate::selector::FlagMatcher;

/// Walk `cmd`'s args (local first, inherited second; dedup by id) and
/// build a `(properties_map, required_list)` ready to splice into the
/// tool's input schema.
///
/// Hidden args are skipped. Args matching a key in `cfg.flag_schemas`
/// use the user-supplied override wholesale and skip the auto extraction.
///
/// `local_flag` and `inherited_flag` are optional [`FlagMatcher`] closures
/// sourced from the first-match-wins selector that claimed this command.
/// When `Some`, each arg is passed to the matcher before inclusion; `false`
/// means the flag is omitted from the schema.  When `None` all flags pass.
pub(crate) fn build_flags_schema(
    cmd: &Command,
    cfg: &Config,
    cmd_path: &str,
    local_flag: Option<&FlagMatcher>,
    inherited_flag: Option<&FlagMatcher>,
) -> (Map<String, Value>, Vec<String>) {
    let mut properties: Map<String, Value> = Map::new();
    let mut required: Vec<String> = Vec::new();

    // Collect local arg ids for dedup.
    let local_ids: HashSet<&str> = cmd
        .get_arguments()
        .filter(|a| !a.is_global_set())
        .map(|a| a.get_id().as_str())
        .collect();

    // Process local args first.
    for arg in cmd.get_arguments().filter(|a| !a.is_global_set()) {
        if local_flag.is_some_and(|m| !m(arg)) {
            continue;
        }
        process_arg(arg, cfg, cmd_path, &mut properties, &mut required);
    }

    // Process inherited (global) args, skipping any whose id was already
    // covered by a local arg.
    for arg in cmd.get_arguments().filter(|a| a.is_global_set()) {
        if local_ids.contains(arg.get_id().as_str()) {
            continue; // local won
        }
        if inherited_flag.is_some_and(|m| !m(arg)) {
            continue;
        }
        process_arg(arg, cfg, cmd_path, &mut properties, &mut required);
    }

    (properties, required)
}

/// Process a single [`Arg`], inserting into `properties` and `required`.
///
/// Skips hidden args. Applies a wholesale `flag_schemas` override when
/// present; otherwise auto-extracts type, description, defaults, and enum.
fn process_arg(
    arg: &Arg,
    cfg: &Config,
    cmd_path: &str,
    properties: &mut Map<String, Value>,
    required: &mut Vec<String>,
) {
    if arg.is_hide_set() {
        return;
    }

    let name = arg.get_id().as_str().to_owned();

    // Wholesale override via cfg.flag_schemas.
    let key = (cmd_path.to_owned(), name.clone());
    if let Some(override_schema) = cfg.flag_schemas.get(&key) {
        properties.insert(name.clone(), override_schema.clone());
        if arg.is_required_set() {
            required.push(name);
        }
        return;
    }

    // Auto-extract the schema for this arg.
    let prop = build_arg_schema(arg, cfg, cmd_path);
    if arg.is_required_set() {
        required.push(name.clone());
    }
    properties.insert(name, Value::Object(prop));
}

/// Build the JSON Schema object for a single arg via auto-extraction.
fn build_arg_schema(arg: &Arg, cfg: &Config, cmd_path: &str) -> Map<String, Value> {
    let mut prop: Map<String, Value> = Map::new();

    // Description from get_help() if set.
    if let Some(help) = arg.get_help() {
        prop.insert("description".into(), Value::String(help.to_string()));
    }

    // Coarse type classification.
    let coarse_type = classify(arg, cfg, cmd_path);
    prop.insert(
        "type".into(),
        Value::String(coarse_type.as_json_type().to_owned()),
    );

    // Per-type extras.
    match coarse_type {
        SchemaType::StringPath => {
            prop.insert("format".into(), Value::String("path".into()));
        }
        SchemaType::Array => {
            let item_type = classify_array_item(arg, cfg, cmd_path);
            let mut items_obj = Map::new();
            items_obj.insert(
                "type".into(),
                Value::String(item_type.as_json_type().to_owned()),
            );
            if item_type == SchemaType::StringPath {
                items_obj.insert("format".into(), Value::String("path".into()));
            }
            prop.insert("items".into(), Value::Object(items_obj));
        }
        _ => {}
    }

    // Enum values from get_possible_values().
    let possible_values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .map(|pv| pv.get_name().to_string())
        .collect();
    if !possible_values.is_empty() {
        prop.insert(
            "enum".into(),
            Value::Array(possible_values.into_iter().map(Value::String).collect()),
        );
        // `PossibleValuesParser` carries an explicit enum domain; lower it as
        // `type: "string"` with an accompanying `enum` array.
        prop.insert("type".into(), Value::String("string".into()));
    }

    // Default value from get_default_values().
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|os| os.to_string_lossy().to_string())
        .collect();
    if !defaults.is_empty() {
        let encoded = encode_defaults(arg, &defaults);
        prop.insert("default".into(), encoded);
    }

    prop
}

/// Classify the coarse [`SchemaType`] for an arg.
///
/// Resolution order:
/// 1. `cfg.flag_type_overrides` — user-supplied override wins.
/// 2. `ArgAction` — `SetTrue`/`SetFalse` → Boolean; `Count` → Integer;
///    `Append` → Array.
/// 3. `value_parser` type id via [`known_type_classifications`].
/// 4. Fallback to `String`, emitting a `tracing::debug!`.
fn classify(arg: &Arg, cfg: &Config, cmd_path: &str) -> SchemaType {
    let name = arg.get_id().as_str();

    // 1. Explicit override.
    if let Some(&ty) = cfg
        .flag_type_overrides
        .get(&(cmd_path.to_owned(), name.to_owned()))
    {
        return ty;
    }

    // 2. Action-based classification.
    match arg.get_action() {
        ArgAction::SetTrue | ArgAction::SetFalse => return SchemaType::Boolean,
        ArgAction::Count => return SchemaType::Integer,
        ArgAction::Append => return SchemaType::Array,
        _ => {}
    }

    // 3. Value parser type id.
    classify_by_type_id(arg, cmd_path)
}

/// Classify the item type inside an `ArgAction::Append` arg.
///
/// The action itself signals "Array"; this function resolves the scalar
/// element type using the same value-parser lookup used by [`classify`],
/// but without the action-based shortcut.
fn classify_array_item(arg: &Arg, cfg: &Config, cmd_path: &str) -> SchemaType {
    let name = arg.get_id().as_str();

    // Explicit override takes precedence for the item type. If the override
    // was set to Array (i.e. it named the outer container, not the item), fall
    // through to the TypeId classifier below, which correctly returns the
    // scalar SchemaType (Integer / Number / StringPath / etc.) for all
    // well-known parsers. The fallback for unknown parsers is
    // SchemaType::String, which serialises as `{"type": "string"}` items.
    if let Some(&ty) = cfg
        .flag_type_overrides
        .get(&(cmd_path.to_owned(), name.to_owned()))
        && ty != SchemaType::Array
    {
        return ty;
    }

    classify_by_type_id(arg, cmd_path)
}

/// Look up the [`SchemaType`] from the arg's value parser type id, falling
/// back to `String` with a debug log when the type is unrecognised.
///
/// Walks [`known_type_classifications`] comparing each entry's [`TypeId`]
/// against the arg's `AnyValueId` via `PartialEq<TypeId>` — no macro
/// needed, and no duplicate table to maintain.
fn classify_by_type_id(arg: &Arg, cmd_path: &str) -> SchemaType {
    let parser_id = arg.get_value_parser().type_id();

    // AnyValueId implements PartialEq<TypeId>, so the comparison works
    // without naming the opaque AnyValueId type.
    for &(known_ty, schema) in known_type_classifications() {
        if parser_id == known_ty {
            return schema;
        }
    }

    tracing::debug!(
        target: "brontes::schema::flag",
        flag = arg.get_id().as_str(),
        cmd_path,
        "unrecognized value parser; falling back to string"
    );
    SchemaType::String
}

/// Encode the default value(s) from an arg into a [`Value`].
///
/// - `SetTrue` → `true`, `SetFalse` → `false` (bool, not string).
/// - `Count` → number (0 when no parseable default is present).
/// - Single default → string.
/// - Multiple defaults → array of strings.
fn encode_defaults(arg: &Arg, defaults: &[String]) -> Value {
    match arg.get_action() {
        ArgAction::SetTrue => Value::Bool(true),
        ArgAction::SetFalse => Value::Bool(false),
        ArgAction::Count => defaults
            .first()
            .and_then(|s| s.parse::<u64>().ok())
            .map_or_else(|| json!(0u64), |n| Value::Number(n.into())),
        _ => {
            if defaults.len() == 1 {
                Value::String(defaults[0].clone())
            } else {
                Value::Array(defaults.iter().map(|s| Value::String(s.clone())).collect())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::{Arg, ArgAction, Command, value_parser};
    use serde_json::{Value, json};

    use super::*;
    use crate::config::Config;
    use crate::schema::SchemaType;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Build a single-command fixture and call `build_flags_schema`.
    fn schema_for(cmd: &Command) -> (Map<String, Value>, Vec<String>) {
        let cfg = Config::default();
        build_flags_schema(cmd, &cfg, "my-cli", None, None)
    }

    fn cmd_with_arg(arg: Arg) -> Command {
        Command::new("my-cli").arg(arg)
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    #[test]
    fn bool_flag_set_true_action_becomes_boolean() {
        let cmd = cmd_with_arg(
            Arg::new("verbose")
                .long("verbose")
                .action(ArgAction::SetTrue),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("verbose").expect("verbose in props");
        assert_eq!(prop["type"], json!("boolean"));
    }

    #[test]
    fn count_action_becomes_integer() {
        let cmd = cmd_with_arg(
            Arg::new("verbosity")
                .long("verbosity")
                .action(ArgAction::Count),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("verbosity").expect("verbosity in props");
        assert_eq!(prop["type"], json!("integer"));
    }

    #[test]
    fn append_action_becomes_array() {
        let cmd = cmd_with_arg(
            Arg::new("tags")
                .long("tags")
                .action(ArgAction::Append)
                .value_parser(value_parser!(String)),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("tags").expect("tags in props");
        assert_eq!(prop["type"], json!("array"));
        assert_eq!(prop["items"], json!({"type": "string"}));
    }

    #[test]
    fn value_parser_i64_becomes_integer() {
        let cmd = cmd_with_arg(
            Arg::new("count")
                .long("count")
                .value_parser(value_parser!(i64)),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("count").expect("count in props");
        assert_eq!(prop["type"], json!("integer"));
    }

    #[test]
    fn value_parser_pathbuf_becomes_string_path() {
        let cmd = cmd_with_arg(
            Arg::new("output")
                .long("output")
                .value_parser(value_parser!(PathBuf)),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("output").expect("output in props");
        assert_eq!(prop["type"], json!("string"));
        assert_eq!(prop["format"], json!("path"));
    }

    #[test]
    fn possible_values_become_enum_string() {
        let cmd = cmd_with_arg(
            Arg::new("level")
                .long("level")
                .value_parser(["debug", "info", "warn"]),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("level").expect("level in props");
        assert_eq!(prop["type"], json!("string"));
        let enum_vals = prop["enum"].as_array().expect("enum array");
        let names: Vec<&str> = enum_vals
            .iter()
            .map(|v| v.as_str().expect("string enum value"))
            .collect();
        assert_eq!(names, vec!["debug", "info", "warn"]);
    }

    #[test]
    fn required_flag_lands_in_required_list() {
        let cmd = cmd_with_arg(
            Arg::new("input")
                .long("input")
                .required(true)
                .value_parser(value_parser!(String)),
        );
        let (props, required) = schema_for(&cmd);
        assert!(props.contains_key("input"), "input in props");
        assert!(required.contains(&"input".to_owned()), "input in required");
    }

    #[test]
    fn default_value_populates_default() {
        let cmd = cmd_with_arg(
            Arg::new("level")
                .long("level")
                .default_value("info")
                .value_parser(value_parser!(String)),
        );
        let (props, _) = schema_for(&cmd);
        let prop = props.get("level").expect("level in props");
        assert_eq!(prop["type"], json!("string"));
        assert_eq!(prop["default"], json!("info"));
    }

    #[test]
    fn hidden_flag_is_skipped() {
        let cmd = cmd_with_arg(
            Arg::new("secret")
                .long("secret")
                .hide(true)
                .value_parser(value_parser!(String)),
        );
        let (props, required) = schema_for(&cmd);
        assert!(
            !props.contains_key("secret"),
            "hidden flag must not appear in props"
        );
        assert!(
            !required.contains(&"secret".to_owned()),
            "hidden flag must not appear in required"
        );
    }

    #[test]
    fn local_then_inherited_dedup() {
        // Parent has a global "log-level" arg with description "parent".
        // Child re-declares "log-level" locally with description "child".
        // build_flags_schema on the child command should use the LOCAL version.
        let parent = Command::new("my-cli")
            .arg(
                Arg::new("log-level")
                    .long("log-level")
                    .global(true)
                    .help("parent")
                    .value_parser(value_parser!(String)),
            )
            .subcommand(
                Command::new("sub").arg(
                    Arg::new("log-level")
                        .long("log-level")
                        .help("child")
                        .value_parser(value_parser!(String)),
                ),
            );

        // Resolve the parent (required for clap to propagate globals).
        // build() takes &mut self and returns (); use a mut binding.
        let mut parent = parent;
        parent.build();
        let sub = parent
            .get_subcommands()
            .find(|s: &&Command| s.get_name() == "sub")
            .expect("sub command");

        let cfg = Config::default();
        let (props, _) = build_flags_schema(sub, &cfg, "my-cli sub", None, None);

        let prop = props.get("log-level").expect("log-level in props");
        assert_eq!(
            prop["description"],
            json!("child"),
            "local flag should win over inherited"
        );
    }

    #[test]
    fn flag_schemas_wholesale_override_applies() {
        let mut cfg = Config::default();
        cfg.flag_schemas.insert(
            ("my-cli".to_owned(), "limit".to_owned()),
            json!({"type": "integer", "minimum": 1, "maximum": 100}),
        );

        let cmd = Command::new("my-cli").arg(
            Arg::new("limit")
                .long("limit")
                .value_parser(value_parser!(String)), // would auto-extract as string
        );

        let (props, _) = build_flags_schema(&cmd, &cfg, "my-cli", None, None);
        let prop = props.get("limit").expect("limit in props");
        assert_eq!(
            *prop,
            json!({"type": "integer", "minimum": 1, "maximum": 100}),
            "wholesale override must be used verbatim"
        );
    }

    #[test]
    fn flag_type_overrides_nudges_classify() {
        // The arg uses value_parser!(String) which would normally classify as
        // SchemaType::String. The override nudges it to Array.
        //
        // Because build_arg_schema enters the Array branch for any Array
        // coarse_type (override or action-based), it calls classify_array_item
        // to determine the item type. For a value_parser!(String) arg,
        // classify_array_item resolves to SchemaType::String via the TypeId
        // lookup, so items: {"type": "string"} is always present.
        let mut cfg = Config::default();
        cfg.flag_type_overrides
            .insert(("my-cli".to_owned(), "tags".to_owned()), SchemaType::Array);

        let cmd = Command::new("my-cli").arg(
            Arg::new("tags")
                .long("tags")
                .value_parser(value_parser!(String)),
        );

        let (props, _) = build_flags_schema(&cmd, &cfg, "my-cli", None, None);
        let prop = props.get("tags").expect("tags in props");
        assert_eq!(
            prop["type"],
            json!("array"),
            "type override must produce array"
        );
        // The item type is resolved from the value parser (String → "string")
        // even when the outer type is forced to Array via an override.
        assert_eq!(
            prop["items"],
            json!({"type": "string"}),
            "items must reflect the value_parser scalar type"
        );
    }
}
