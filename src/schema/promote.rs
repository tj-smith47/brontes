//! SEP-2243 `x-mcp-header` flag promotion.
//!
//! A promoted flag stops being a member of the tool's `flags` object and
//! becomes a top-level property of the input schema, annotated with
//! `x-mcp-header`. That placement is the whole point: rmcp reads the annotation
//! only from top-level properties and rejects it outright anywhere deeper, so a
//! flag left nested under `flags` can never be promoted where it sits.
//!
//! The promotion is duplicative routing metadata, not a second data channel.
//! The value still travels in the request body; the header is a copy an
//! intermediary can route on without parsing JSON, and the receiving server
//! rejects the request if the two disagree.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::schema::flag::FlagsSchema;

/// The JSON Schema types SEP-2243 can carry in a header value.
const PROMOTABLE_TYPES: [&str; 3] = ["string", "integer", "boolean"];

/// Top-level property names the base `ToolInput` schema already owns.
const RESERVED_PROPERTIES: [&str; 2] = ["flags", "args"];

/// What promotion contributes to a tool's input schema, plus what the call
/// path needs to undo it.
#[derive(Debug, Default)]
pub struct Promotions {
    /// Top-level properties to splice in, each carrying `x-mcp-header`.
    pub properties: Map<String, Value>,
    /// Promoted flags that were required, for the top-level `required` array.
    pub required: Vec<String>,
    /// Promoted flag names, so `tools/call` can fold the hoisted values back
    /// into `flags` before the command runs.
    pub names: BTreeSet<String>,
}

/// True for an RFC 9110 §5.6.2 token character.
///
/// Header names outside this set cannot be sent at all, so a config that
/// produces one is rejected rather than advertised.
const fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
        )
}

/// Reject a header name that could not survive the wire, and a pair of flags
/// that would collide once HTTP case-folds their headers.
///
/// Runs over [`Config`] alone, so it can fire during build-time path validation
/// alongside the other config-shape checks, before any schema exists.
pub fn validate_header_names(cfg: &Config) -> Result<()> {
    // Case-insensitive header uniqueness is per command: two commands may each
    // promote their own `--region`, but one command cannot promote two flags
    // onto `Mcp-Param-Region`.
    let mut seen: BTreeMap<(&str, String), &str> = BTreeMap::new();

    for ((cmd_path, flag), header) in &cfg.promoted_flags {
        if header.is_empty() {
            return Err(Error::Config(format!(
                "Config.promoted_flags: flag '{flag}' on command path '{cmd_path}' \
                 has an empty header name"
            )));
        }
        if !header.chars().all(is_tchar) {
            return Err(Error::Config(format!(
                "Config.promoted_flags: header name {header:?} for flag '{flag}' on \
                 command path '{cmd_path}' is not a valid HTTP token; pass an explicit \
                 name via Config::promote_flag_as"
            )));
        }
        if RESERVED_PROPERTIES.contains(&flag.as_str()) {
            return Err(Error::Config(format!(
                "Config.promoted_flags: flag '{flag}' on command path '{cmd_path}' \
                 cannot be promoted; '{flag}' is already a top-level property of every \
                 tool's input schema"
            )));
        }
        if let Some(previous) = seen.insert(
            (cmd_path.as_str(), header.to_ascii_lowercase()),
            flag.as_str(),
        ) {
            return Err(Error::Config(format!(
                "Config.promoted_flags: flags '{previous}' and '{flag}' on command path \
                 '{cmd_path}' both promote to header {header:?}; HTTP header names are \
                 case-insensitive"
            )));
        }
    }

    Ok(())
}

/// Move this command's promoted flags out of `flags` and into a set of
/// top-level properties.
///
/// The flag's own schema is carried over verbatim with `x-mcp-header` added, so
/// descriptions, defaults, and enums survive promotion.
pub fn take_promoted(flags: &mut FlagsSchema, cfg: &Config, cmd_path: &str) -> Result<Promotions> {
    let mut out = Promotions::default();

    for ((path, flag), header) in &cfg.promoted_flags {
        if path != cmd_path {
            continue;
        }

        let Some(mut schema) = flags.properties.remove(flag) else {
            // The flag exists on the command — build-time validation already
            // proved that — so reaching here means a selector's flag matcher
            // excluded it. Promoting a flag the tool does not expose is a
            // no-op, but it is also a contradiction the user asked for twice.
            tracing::warn!(
                target: "brontes::schema",
                command = %cmd_path,
                flag = %flag,
                "promoted flag is not exposed by this tool; a selector flag matcher \
                 excluded it, so the x-mcp-header annotation was dropped"
            );
            continue;
        };

        let declared = schema.get("type").and_then(Value::as_str);
        if !declared.is_some_and(|t| PROMOTABLE_TYPES.contains(&t)) {
            let got = declared.unwrap_or("none");
            return Err(Error::Config(format!(
                "Config.promoted_flags: flag '{flag}' on command path '{cmd_path}' has \
                 schema type '{got}'; SEP-2243 promotes only string, integer, and \
                 boolean properties"
            )));
        }

        if let Some(obj) = schema.as_object_mut() {
            obj.insert("x-mcp-header".into(), Value::String(header.clone()));
        }

        // A promoted flag keeps whatever required-ness it had; the entry just
        // moves from `flags.required` to the schema's top-level `required`.
        if let Some(pos) = flags.required.iter().position(|r| r == flag) {
            flags.required.remove(pos);
            out.required.push(flag.clone());
        }

        out.properties.insert(flag.clone(), schema);
        out.names.insert(flag.clone());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::flag::{FlagRender, FlagSpec};
    use serde_json::json;

    fn flags_with(name: &str, schema: Value, required: bool) -> FlagsSchema {
        let mut out = FlagsSchema::default();
        out.properties.insert(name.to_owned(), schema);
        if required {
            out.required.push(name.to_owned());
        }
        out.specs.insert(
            name.to_owned(),
            FlagSpec {
                render: FlagRender::Value,
            },
        );
        out
    }

    #[test]
    fn a_promoted_flag_leaves_flags_and_gains_the_annotation() {
        let cfg = Config::default().promote_flag("cli deploy", "region");
        let mut flags = flags_with("region", json!({"type": "string"}), false);

        let promoted = take_promoted(&mut flags, &cfg, "cli deploy").expect("promotion");

        assert!(
            !flags.properties.contains_key("region"),
            "the flag must not remain under `flags`; two sources of truth is the bug"
        );
        assert_eq!(
            promoted.properties.get("region"),
            Some(&json!({"type": "string", "x-mcp-header": "region"})),
            "the hoisted property carries the annotation"
        );
        assert!(promoted.names.contains("region"));
    }

    #[test]
    fn promotion_preserves_the_flags_own_schema() {
        let cfg = Config::default().promote_flag_as("cli deploy", "region", "Region");
        let mut flags = flags_with(
            "region",
            json!({"type": "string", "description": "Target region", "enum": ["us", "eu"]}),
            false,
        );

        let promoted = take_promoted(&mut flags, &cfg, "cli deploy").expect("promotion");

        assert_eq!(
            promoted.properties.get("region"),
            Some(&json!({
                "type": "string",
                "description": "Target region",
                "enum": ["us", "eu"],
                "x-mcp-header": "Region",
            })),
            "description and enum must survive the move"
        );
    }

    #[test]
    fn a_required_promoted_flag_moves_its_required_entry() {
        let cfg = Config::default().promote_flag("cli deploy", "region");
        let mut flags = flags_with("region", json!({"type": "string"}), true);

        let promoted = take_promoted(&mut flags, &cfg, "cli deploy").expect("promotion");

        assert!(
            flags.required.is_empty(),
            "the entry must not stay in flags.required, which now cannot be satisfied"
        );
        assert_eq!(promoted.required, vec!["region".to_string()]);
    }

    #[test]
    fn promotion_is_scoped_to_its_command_path() {
        let cfg = Config::default().promote_flag("cli deploy", "region");
        let mut flags = flags_with("region", json!({"type": "string"}), false);

        let promoted = take_promoted(&mut flags, &cfg, "cli status").expect("promotion");

        assert!(
            promoted.names.is_empty(),
            "a different command is untouched"
        );
        assert!(flags.properties.contains_key("region"));
    }

    #[test]
    fn a_non_primitive_flag_is_rejected_rather_than_advertised() {
        let cfg = Config::default().promote_flag("cli deploy", "tags");
        let mut flags = flags_with("tags", json!({"type": "array"}), false);

        let err = take_promoted(&mut flags, &cfg, "cli deploy").expect_err("must reject");
        let msg = err.to_string();
        assert!(msg.contains("'tags'") && msg.contains("array"), "{msg}");
    }

    #[test]
    fn a_number_flag_is_rejected_because_only_integer_is_promotable() {
        // `number` reads like a primitive but is not on SEP-2243's list, so the
        // check must be an allowlist rather than a "not a container" test.
        let cfg = Config::default().promote_flag("cli deploy", "ratio");
        let mut flags = flags_with("ratio", json!({"type": "number"}), false);

        assert!(take_promoted(&mut flags, &cfg, "cli deploy").is_err());
    }

    #[test]
    fn a_flag_excluded_by_a_selector_is_skipped_not_fatal() {
        let cfg = Config::default().promote_flag("cli deploy", "region");
        let mut flags = FlagsSchema::default();

        let promoted = take_promoted(&mut flags, &cfg, "cli deploy").expect("must not fail");
        assert!(promoted.names.is_empty());
    }

    #[test]
    fn header_names_must_be_http_tokens() {
        let cfg = Config::default().promote_flag_as("cli deploy", "region", "bad header");
        let err = validate_header_names(&cfg).expect_err("must reject");
        assert!(err.to_string().contains("valid HTTP token"));
    }

    #[test]
    fn an_empty_header_name_is_rejected() {
        let cfg = Config::default().promote_flag_as("cli deploy", "region", "");
        assert!(validate_header_names(&cfg).is_err());
    }

    #[test]
    fn two_flags_cannot_share_a_header_case_insensitively() {
        let cfg = Config::default()
            .promote_flag_as("cli deploy", "region", "Region")
            .promote_flag_as("cli deploy", "zone", "region");

        let err = validate_header_names(&cfg).expect_err("must reject");
        assert!(err.to_string().contains("case-insensitive"), "{err}");
    }

    #[test]
    fn the_same_header_on_two_commands_is_fine() {
        // The disconfirming direction for the uniqueness check: it must scope
        // to one command, or every CLI with a shared global flag breaks.
        let cfg = Config::default()
            .promote_flag("cli deploy", "region")
            .promote_flag("cli status", "region");

        assert!(validate_header_names(&cfg).is_ok());
    }

    #[test]
    fn a_flag_named_after_a_reserved_property_is_rejected() {
        for reserved in RESERVED_PROPERTIES {
            let cfg = Config::default().promote_flag("cli deploy", reserved);
            let err = validate_header_names(&cfg).expect_err("reserved name must be rejected");
            assert!(
                err.to_string().contains("already a top-level property"),
                "{reserved}: {err}"
            );
        }
    }
}
