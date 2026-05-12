//! Pins for happy-path input edge cases on `brontes::generate_tools`.
//!
//! Five shapes are covered:
//!
//! - **Deep nesting** — a 10-level subcommand chain must walk and render
//!   without stack overflow, and the deepest tool name must reflect the
//!   full path joined by underscores.
//! - **Wide flat tree** — a root with 100 sibling subcommands must produce
//!   100 leaf tools (the group-only root is filtered).
//! - **No-args leaf** — a leaf with no positionals and no user-defined flags
//!   still has a per-tool input schema whose `flags.properties` map exists,
//!   and is empty: clap's auto-injected `--help` is filtered from the tool
//!   surface per the v0.1.0 contract.
//! - **Positional-only leaf** — a leaf with one required positional surfaces
//!   ONLY in `args.description` (as a `Usage pattern:` line). Positional
//!   args are intentionally absent from `flags.properties` and
//!   `flags.required`; they flow through `properties.args` alone.
//! - **Empty-string prefix fallback** — `Config::tool_name_prefix("")` does
//!   not produce names starting with `_`; the empty override falls back to
//!   the root command name.
//!
//! Schema navigation is inlined per test rather than factored into a shared
//! module — these files stay self-contained to avoid coupling across the
//! test suite.

use brontes::Config;
use clap::{Arg, Command};

// ---------------------------------------------------------------------------
// 1. Deep nesting
// ---------------------------------------------------------------------------

#[test]
fn deep_nesting_no_stack_overflow() {
    // Build a 10-level chain bottom-up: c0 → c1 → … → c10.
    // The clone-then-build inside `generate_tools` recurses through
    // `walk::walk_recursive`; a stack-overflow regression here would surface
    // as a process abort, not an Err.
    let mut leaf = Command::new("c10");
    for i in (0..10).rev() {
        leaf = Command::new(format!("c{i}")).subcommand(leaf);
    }
    let root = leaf;

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("deep tree must walk and render without error");

    // The deepest tool's name is the prefix (= root name "c0") followed by
    // every nested component joined with underscores: "c0_c1_c2_…_c10".
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    let deepest = "c0_c1_c2_c3_c4_c5_c6_c7_c8_c9_c10";
    assert!(
        names.contains(&deepest),
        "expected deepest tool {deepest:?} in tool list, got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Wide flat tree
// ---------------------------------------------------------------------------

#[test]
fn wide_flat_tree_renders_all_tools() {
    // Root + 100 sibling subcommands. The root is marked
    // `subcommand_required(true)` and has no user-facing args, so it is
    // group-only and filtered. The expected output is exactly 100 leaf tools.
    let mut root = Command::new("wide").subcommand_required(true);
    for i in 0..100 {
        root = root.subcommand(Command::new(format!("leaf{i:03}")));
    }

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("wide tree must render without error");

    assert_eq!(
        tools.len(),
        100,
        "expected 100 leaf tools (group-only root filtered), got {}",
        tools.len()
    );

    // Sample-check three names to confirm the full range is present without
    // a 100-entry assertion vector.
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for sample in ["wide_leaf000", "wide_leaf050", "wide_leaf099"] {
        assert!(
            names.contains(&sample),
            "expected {sample:?} in tool list (first 5 names: {:?})",
            &names[..names.len().min(5)]
        );
    }
}

// ---------------------------------------------------------------------------
// 3. No-args command — empty user-flags schema
// ---------------------------------------------------------------------------

#[test]
fn no_args_command_has_empty_flags_schema() {
    // `bare` has no positionals and no user-defined flags. The group-only
    // root is filtered. clap auto-injects a `--help` flag on every command,
    // but brontes filters `help` (and `version`) from the tool surface —
    // those are runtime CLI behaviors, not part of the MCP tool contract.
    // The expected `flags.properties` shape is therefore the empty object.
    let root = Command::new("noargs")
        .subcommand_required(true)
        .subcommand(Command::new("bare"));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("no-args leaf must render without error");

    let bare = tools
        .iter()
        .find(|t| t.name.as_ref() == "noargs_bare")
        .expect("noargs_bare tool must be present");

    // Navigate to flags.properties — the per-flag map for this command.
    let flags_props = bare
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("flags"))
        .and_then(serde_json::Value::as_object)
        .and_then(|f| f.get("properties"))
        .and_then(serde_json::Value::as_object)
        .expect("flags.properties must be an object for a no-args leaf");

    // Pin reality: the map is empty. `help` is filtered (auto-injected by
    // clap on every command, excluded from the tool surface). `version`
    // would be filtered too, but it's root-only and the root itself is
    // filtered as group-only.
    let keys: Vec<&str> = flags_props.keys().map(String::as_str).collect();
    assert!(
        keys.is_empty(),
        "no-args leaf must expose an empty flags.properties map (clap's `help` filtered), got: {keys:?}"
    );

    // The flags object itself must remain `additionalProperties: false` so
    // the MCP layer rejects unknown flag names.
    let flags_obj = bare
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("flags"))
        .and_then(serde_json::Value::as_object)
        .expect("flags must be an object");
    assert_eq!(
        flags_obj.get("additionalProperties"),
        Some(&serde_json::Value::Bool(false)),
        "no-args leaf flags must keep additionalProperties: false"
    );
}

// ---------------------------------------------------------------------------
// 4. Positional-only command — args + required-positional surfacing
// ---------------------------------------------------------------------------

#[test]
fn positional_only_command_renders_args_schema() {
    // `touch` has one required positional `path` and no flags. The
    // group-only root is filtered. Pin the contract: positionals appear in
    // `properties.args` (via `args_description`) and nowhere else —
    // `properties.flags.properties` and `properties.flags.required` MUST
    // NOT carry `path`. clap exposes positionals through
    // `Command::get_arguments()`, but the per-flag schema loop filters
    // `Arg::is_positional()` so positionals surface exactly once.
    let root = Command::new("posonly")
        .subcommand_required(true)
        .subcommand(Command::new("touch").arg(Arg::new("path").required(true)));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("positional-only leaf must render without error");

    let touch = tools
        .iter()
        .find(|t| t.name.as_ref() == "posonly_touch")
        .expect("posonly_touch tool must be present");

    // (a) args description carries the positional's usage pattern.
    let args_obj = touch
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("args"))
        .and_then(serde_json::Value::as_object)
        .expect("properties.args must be an object");
    let args_desc = args_obj
        .get("description")
        .and_then(serde_json::Value::as_str)
        .expect("args.description must be a string");
    assert!(
        args_desc.starts_with("Positional command line arguments"),
        "args.description must start with the canonical phrase, got: {args_desc:?}"
    );
    assert!(
        args_desc.contains("Usage pattern:") && args_desc.contains("<path>"),
        "args.description must carry the `<path>` usage pattern, got: {args_desc:?}"
    );
    assert_eq!(
        args_obj.get("type"),
        Some(&serde_json::Value::String("array".into())),
        "args must remain `type: array`"
    );
    let items = args_obj
        .get("items")
        .and_then(serde_json::Value::as_object)
        .expect("args.items must be an object");
    assert_eq!(
        items.get("type"),
        Some(&serde_json::Value::String("string".into())),
        "args.items.type must be string"
    );

    // (b) `path` must NOT appear in flags — neither in `flags.properties`
    // (where the per-flag schema lives) nor in `flags.required`. Positionals
    // belong to `properties.args` alone.
    let flags_obj = touch
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("flags"))
        .and_then(serde_json::Value::as_object)
        .expect("properties.flags must be an object");
    let flags_props = flags_obj
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("flags.properties must be an object");
    assert!(
        !flags_props.contains_key("path"),
        "positional `path` must NOT appear in flags.properties, got keys: {:?}",
        flags_props.keys().collect::<Vec<_>>()
    );
    // `flags.required` is only emitted when at least one flag is required.
    // With `path` filtered (and no other flags), the key must be absent
    // entirely — emitting `required: []` would be a regression.
    assert!(
        flags_obj.get("required").is_none(),
        "flags.required must be absent when no flags are required, got: {:?}",
        flags_obj.get("required")
    );
}

// ---------------------------------------------------------------------------
// 5. Empty-string prefix falls back to root command name
// ---------------------------------------------------------------------------

#[test]
fn empty_prefix_falls_back_to_root_name() {
    // `Config::default().tool_name_prefix("")` explicitly sets the prefix to
    // the empty string. The documented fallback (see
    // src/command.rs::generate_tools, around the
    // `as_deref().filter(|s| !s.is_empty()).unwrap_or_else(...)` chain) is to
    // ignore an empty override and use the root command name. The leaf must
    // therefore be named `mycli_op`, NOT `_op`.
    let root = Command::new("mycli")
        .subcommand_required(true)
        .subcommand(Command::new("op"));
    let cfg = Config::default().tool_name_prefix("");

    let tools = brontes::generate_tools(&root, &cfg)
        .expect("empty prefix must validate and fall back to the root name");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"mycli_op"),
        "expected leaf tool `mycli_op` after empty-prefix fallback, got: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.starts_with('_')),
        "no tool name may start with `_` (empty prefix must NOT be substituted verbatim), got: {names:?}"
    );
}
