//! Pinning tests for brontes-vs-ophis deliberate divergences that aren't
//! covered elsewhere in the test suite. See the audit report for the
//! per-divergence cross-reference.
//!
//! Each test below pins exactly one divergence:
//!
//! 1. `annotation_keys_by_full_command_path` — annotation sidecar map is
//!    keyed by FULL space-joined command path (root name included), not the
//!    leaf-only path. Brontes additionally validates the path at build time:
//!    a key that names no walked command is rejected with `Error::Config`
//!    rather than silently ignored.
//! 2. `deprecated_command_filtered_from_tool_list` — the deprecated sidecar
//!    `HashSet<String>` removes the named command's tool from the generated
//!    list, leaving sibling tools untouched.
//! 3. `flag_schema_override_path_qualified` — per-flag schema overrides are
//!    keyed by `(command_path, flag_name)`, so the same flag name on two
//!    different commands can have independent schemas (one overridden, the
//!    other auto-derived).

use brontes::{Config, Error, ToolAnnotations};
use clap::{Arg, Command};
use pretty_assertions::assert_eq;

// ---------------------------------------------------------------------------
// 1. Annotation sidecar keying (full command path, root included)
// ---------------------------------------------------------------------------

#[test]
fn annotation_keys_by_full_command_path() {
    // Positive case: annotating with the full "testcli get" path attaches
    // annotations to the get leaf's tool.
    let root = Command::new("testcli").subcommand(Command::new("get").about("Get a resource"));
    let cfg = Config::default().annotation(
        "testcli get",
        ToolAnnotations {
            title: Some("Get-Operation".into()),
            read_only_hint: Some(true),
            ..Default::default()
        },
    );

    let tools =
        brontes::generate_tools(&root, &cfg).expect("full-path annotation must validate cleanly");

    // Brontes emits tool names as <prefix>_<subpath_with_spaces_as_underscores>.
    // For root "testcli" and leaf "get" the derived tool name is "testcli_get".
    // See src/command.rs::build_tool_name.
    let get_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "testcli_get")
        .expect("testcli_get tool must be present in the generated list");

    let ann = get_tool
        .annotations
        .as_ref()
        .expect("annotations must be attached to testcli_get when keyed by full path");
    assert_eq!(ann.read_only_hint, Some(true));
    assert_eq!(ann.title.as_deref(), Some("Get-Operation"));

    // Negative case: keying on the leaf-only name "get" (without the root
    // "testcli" prefix) is NOT a valid annotation key for brontes. The build
    // surfaces the mistake as Error::Config rather than silently ignoring it,
    // which is the audit-friendlier behavior. This pins both halves of the
    // contract: keys must be the full path, AND wrong keys fail loudly.
    let root2 = Command::new("testcli").subcommand(Command::new("get").about("Get a resource"));
    let cfg_bad = Config::default().annotation(
        "get",
        ToolAnnotations {
            title: Some("Get-Operation".into()),
            read_only_hint: Some(true),
            ..Default::default()
        },
    );

    let result = brontes::generate_tools(&root2, &cfg_bad);
    match result {
        Err(Error::Config(msg)) => {
            assert!(
                msg.contains("\"get\""),
                "expected error message to mention the offending key, got: {msg:?}"
            );
        }
        other => {
            panic!("leaf-only annotation key must be rejected with Error::Config; got: {other:?}")
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Deprecated sidecar set filters tool out
// ---------------------------------------------------------------------------

#[test]
fn deprecated_command_filtered_from_tool_list() {
    // Two no-arg leaves; one is marked deprecated. The deprecated leaf's tool
    // must be absent from the generated list; the sibling's tool must remain.
    let root = Command::new("testcli")
        .subcommand(Command::new("legacy").about("Legacy leaf"))
        .subcommand(Command::new("modern").about("Modern leaf"));

    let cfg = Config::default().deprecate("testcli legacy");

    let tools = brontes::generate_tools(&root, &cfg)
        .expect("deprecating a known path must validate cleanly");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        !names.contains(&"testcli_legacy"),
        "deprecated leaf testcli_legacy must be filtered out, got: {names:?}"
    );
    assert!(
        names.contains(&"testcli_modern"),
        "sibling testcli_modern must remain in the tool list, got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Per-flag schema override is path-qualified
// ---------------------------------------------------------------------------

#[test]
fn flag_schema_override_path_qualified() {
    // Both leaves declare a "format" flag with the same possible values, but
    // only `testcli get`'s "format" is overridden via Config.flag_schema.
    // The override must apply ONLY to the get leaf; the list leaf must keep
    // the auto-derived schema.
    //
    // Schema layout the assertions navigate:
    //   tool.input_schema
    //     .properties
    //       .flags
    //         .properties
    //           .format            ← the per-flag schema lives here
    //
    // See src/schema/mod.rs::build_input_schema_with_matchers and
    // src/schema/flag.rs::process_arg for the layout source of truth.
    let root = Command::new("testcli")
        .subcommand(
            Command::new("get").about("Get a resource").arg(
                Arg::new("format")
                    .long("format")
                    .value_parser(["json", "yaml"]),
            ),
        )
        .subcommand(
            Command::new("list").about("List resources").arg(
                Arg::new("format")
                    .long("format")
                    .value_parser(["json", "yaml"]),
            ),
        );

    let override_schema = serde_json::json!({
        "type": "string",
        "enum": ["json", "csv"],
    });
    let cfg = Config::default().flag_schema("testcli get", "format", override_schema.clone());

    let tools = brontes::generate_tools(&root, &cfg)
        .expect("path-qualified flag_schema must validate cleanly");

    // --- Overridden tool: testcli_get's format flag must use the override
    //     value verbatim (enum ["json", "csv"]).
    let get_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "testcli_get")
        .expect("testcli_get tool must be present");
    let get_format = navigate_to_flag_property(&get_tool.input_schema, "format")
        .expect("testcli_get input_schema must expose properties.flags.properties.format");
    assert_eq!(
        get_format, &override_schema,
        "testcli_get format flag must use the wholesale override schema verbatim"
    );

    // --- Non-overridden sibling: testcli_list's format flag must keep its
    //     auto-derived schema (string + enum ["json", "yaml"]). We don't pin
    //     the entire object shape — just the two fields that prove the
    //     override did NOT bleed across paths.
    let list_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == "testcli_list")
        .expect("testcli_list tool must be present");
    let list_format = navigate_to_flag_property(&list_tool.input_schema, "format")
        .expect("testcli_list input_schema must expose properties.flags.properties.format");
    assert_eq!(
        list_format.get("type"),
        Some(&serde_json::Value::String("string".into())),
        "testcli_list format must remain a plain string-typed auto schema, got: {list_format:?}"
    );
    let list_enum = list_format
        .get("enum")
        .and_then(serde_json::Value::as_array)
        .expect("testcli_list format must have an auto-derived enum array");
    let list_enum_strs: Vec<&str> = list_enum
        .iter()
        .map(|v| v.as_str().expect("enum entries must be strings"))
        .collect();
    assert_eq!(
        list_enum_strs,
        vec!["json", "yaml"],
        "testcli_list format enum must remain the clap-derived values, not the override values"
    );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Navigate an MCP tool input schema to `properties.flags.properties.<flag>`.
///
/// Returns `None` if any layer of the layout is absent, which signals a
/// shape regression in `build_input_schema_with_matchers`.
fn navigate_to_flag_property<'a>(
    input_schema: &'a rmcp::model::JsonObject,
    flag: &str,
) -> Option<&'a serde_json::Value> {
    input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("flags"))
        .and_then(serde_json::Value::as_object)
        .and_then(|f| f.get("properties"))
        .and_then(serde_json::Value::as_object)
        .and_then(|fp| fp.get(flag))
}
