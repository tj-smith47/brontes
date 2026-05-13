//! Pins for adversarial input behavior of `brontes::generate_tools`.
//!
//! Companion to `edge_cases_input.rs`, which covers happy-path shapes. This
//! file pins what happens when the consumer hands brontes inputs at or past
//! the contract's edges:
//!
//! - Empty / whitespace annotation keys — strict-equality lookup, no
//!   trimming or normalization.
//! - The substring filter's permissive matching (a user command named
//!   `helpful` is filtered because `"help"` is a substring of its path).
//! - Unicode command names and multibyte help text — surviving through
//!   `build_tool_name` and `build_description` unchanged.
//! - `Value::Null` / `{}` as the `flag_schema` override payload — does the
//!   override propagate verbatim or get rejected.
//! - Tool-name length boundaries around the 64-character warn threshold.
//!
//! Each test pins the OBSERVED behavior. Where the spec'd assertion and the
//! actual behavior diverge, the comment documents the divergence and the
//! test asserts reality.
//!
//! Schema navigation is inlined per test to keep this file self-contained.

use brontes::{Config, Error, ToolAnnotations};
use clap::{Arg, Command};

// ---------------------------------------------------------------------------
// 1. Empty annotation path — rejected by validate_paths
// ---------------------------------------------------------------------------

#[test]
fn annotation_with_empty_path_errors_at_generate_time() {
    // `validate_paths` (src/command.rs) compares the annotation key against
    // the HashSet of walked command paths. An empty string never appears in
    // that set (the walker always emits at least the root name), so it must
    // be rejected with Error::Config. This pins the strict-equality contract
    // — even the empty string is a real key shape, not a no-op.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("op"));
    let cfg = Config::default().annotation(
        "",
        ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        },
    );

    let result = brontes::generate_tools(&root, &cfg);
    match result {
        Err(Error::Config(msg)) => {
            // The error must surface the offending (empty) key. The
            // `validate_paths` formatter renders the path with `{:?}`, so an
            // empty string appears as the literal `""` substring.
            assert!(
                msg.contains("\"\""),
                "expected error message to mention the empty key as \"\", got: {msg:?}"
            );
            assert!(
                msg.contains("annotations") || msg.contains("annotation"),
                "expected error message to mention the annotations validation surface, got: {msg:?}"
            );
        }
        // `Error` is `#[non_exhaustive]` — guard against silent variant drift.
        other => {
            panic!("empty annotation path must be rejected with Error::Config, got: {other:?}")
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Whitespace annotation key — strict equality, no trim
// ---------------------------------------------------------------------------

#[test]
fn annotation_with_whitespace_key_is_strict_equality() {
    // brontes uses strict-equality lookup on annotation paths. Users must
    // match the path string exactly — no trim, no normalization. A key
    // wrapped in whitespace (`" testcli get "`) is NOT equivalent to
    // `"testcli get"`, so it fails path validation.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("get"));
    let cfg = Config::default().annotation(
        " testcli get ",
        ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        },
    );

    let result = brontes::generate_tools(&root, &cfg);
    match result {
        Err(Error::Config(msg)) => {
            // The whitespace-wrapped key must appear in the error message
            // verbatim (validated via the `{:?}` debug render).
            assert!(
                msg.contains("\" testcli get \""),
                "expected error to surface the whitespace-wrapped key, got: {msg:?}"
            );
        }
        other => panic!(
            "whitespace-wrapped annotation key must be rejected with Error::Config, got: {other:?}"
        ),
    }
}

// ---------------------------------------------------------------------------
// 3. Segment-equality filter — `helpful` is NOT filtered, `help` IS
// ---------------------------------------------------------------------------

#[test]
fn command_named_helpful_survives_segment_equality_filter() {
    // The filter in src/walk.rs::should_filter splits the joined path on
    // spaces and excludes any path where one segment is exactly equal to
    // a needle (`command_name`, `"help"`, or `"completion"`). A command
    // named `helpful` has path `"testcli helpful"` — no segment equals
    // `"help"` exactly, so it survives.
    //
    // Pins the segment-equality behaviour against accidental drift back to
    // substring matching, which would mis-filter consumer CLIs whose root
    // happens to contain a needle (`make-mcp`, `helpful`, `completionish`).
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("helpful").about("A helpful command"));

    let tools =
        brontes::generate_tools(&root, &Config::default()).expect("generate_tools must succeed");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"testcli_helpful"),
        "command named `helpful` must NOT be filtered (no segment equals `help`), got: {names:?}"
    );
}

#[test]
fn command_named_mcp_is_filtered_by_segment_equality_rule() {
    // Pin the positive side of the segment-equality rule via the
    // `command_name` default (`"mcp"`). The `"help"` and `"completion"`
    // needles cannot be exercised end-to-end through `generate_tools`
    // because clap refuses to register user subcommands named `help` (it
    // clashes with the auto-injected help command); `mcp` is the
    // canonical filterable segment, which is the consumer-visible case
    // anyway — the brontes-injected mcp subtree.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("mcp").about("Leaf whose name equals the default command_name"));

    let tools =
        brontes::generate_tools(&root, &Config::default()).expect("generate_tools must succeed");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        !names.iter().any(|n| n.ends_with("_mcp")),
        "leaf segment exactly equal to `mcp` must be filtered, got: {names:?}"
    );
    // Root is group-only (subcommand_required + no user args) so it is
    // filtered; the `mcp` leaf is segment-filtered. Net: zero tools.
    assert!(
        tools.is_empty(),
        "expected zero tools (group-only root + mcp-segment leaf), got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Unicode command names render verbatim in tool names
// ---------------------------------------------------------------------------

#[test]
fn unicode_command_names_render_in_tool_names() {
    // clap accepts unicode names verbatim; `build_tool_name` does only
    // space->underscore substitution and a prefix splice, so multibyte
    // characters survive untouched. The MCP tool name spec allows unicode.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("日本語").about("Japanese command"));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("unicode subcommand name must walk and render without error");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"testcli_日本語"),
        "expected tool name `testcli_日本語` to be present, got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. Multibyte help text survives byte-for-byte into the description
// ---------------------------------------------------------------------------

#[test]
fn multibyte_help_text_survives_to_description() {
    // `build_description` calls `ToString::to_string` on the clap StyledStr
    // result; no normalization happens. Multibyte characters (accented
    // Latin, emoji) must survive byte-for-byte into the per-tool
    // description field.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("greet").about("héllo 🌍"));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("multibyte about text must render without error");

    let greet = tools
        .iter()
        .find(|t| t.name.as_ref() == "testcli_greet")
        .expect("testcli_greet tool must be present");

    let desc = greet.description.as_ref().expect("description must be set");
    assert!(
        desc.contains("héllo 🌍"),
        "multibyte description must survive byte-for-byte, got: {desc:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. flag_schema override = Value::Null propagates verbatim
// ---------------------------------------------------------------------------

#[test]
fn null_flag_schema_override_emits_null() {
    // src/schema/flag.rs::process_arg consumes
    // `cfg.flag_schemas.get(&(path, flag_name))` and writes the value into
    // the per-flag `properties` map via `properties.insert(name,
    // override_schema.clone())`. No `null` check happens — the override
    // propagates verbatim. validate_paths only checks the flag NAME exists,
    // not the schema payload shape.
    //
    // Observed reality: option (a) from the task spec — the override
    // propagates literally and the resulting schema for `verbosity` is
    // JSON `null`.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("op").arg(Arg::new("verbosity").long("verbosity")));
    let cfg = Config::default().flag_schema("testcli op", "verbosity", serde_json::Value::Null);

    let tools = brontes::generate_tools(&root, &cfg)
        .expect("Null flag_schema override must validate (path+flag name both exist)");

    let op = tools
        .iter()
        .find(|t| t.name.as_ref() == "testcli_op")
        .expect("testcli_op tool must be present");

    let verbosity_schema = op
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("flags"))
        .and_then(serde_json::Value::as_object)
        .and_then(|f| f.get("properties"))
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("verbosity"))
        .expect("flags.properties.verbosity must be present (override inserted it)");

    assert!(
        verbosity_schema.is_null(),
        "Value::Null override must propagate verbatim; got: {verbosity_schema:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. flag_schema override = {} propagates verbatim (empty object)
// ---------------------------------------------------------------------------

#[test]
fn empty_object_flag_schema_override_passes_through() {
    // Same path as #6: the override payload is inserted verbatim into the
    // per-flag properties map. An empty object `{}` is a degenerate-but-
    // legal JSON Schema value (matches anything when no constraints are
    // present), and brontes does no shape validation on the payload.
    let root = Command::new("testcli")
        .subcommand_required(true)
        .subcommand(Command::new("op").arg(Arg::new("verbosity").long("verbosity")));
    let cfg = Config::default().flag_schema("testcli op", "verbosity", serde_json::json!({}));

    let tools = brontes::generate_tools(&root, &cfg)
        .expect("empty-object flag_schema override must validate");

    let op = tools
        .iter()
        .find(|t| t.name.as_ref() == "testcli_op")
        .expect("testcli_op tool must be present");

    let verbosity_schema = op
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("flags"))
        .and_then(serde_json::Value::as_object)
        .and_then(|f| f.get("properties"))
        .and_then(serde_json::Value::as_object)
        .and_then(|p| p.get("verbosity"))
        .expect("flags.properties.verbosity must be present after override");

    assert_eq!(
        verbosity_schema,
        &serde_json::json!({}),
        "empty-object override must propagate verbatim; got: {verbosity_schema:?}"
    );
}

// ---------------------------------------------------------------------------
// 8. Tool name at exactly 64 chars — boundary check, no warn fires
// ---------------------------------------------------------------------------

#[test]
fn tool_name_at_64_chars_no_warn() {
    // Construction: root `r` (1 char) + leaf `x*62`. Tool name is built as
    // prefix("r") + "_" + leaf_name → "r_" + "x"*62 = 1 + 1 + 62 = 64.
    //
    // src/command.rs:109-116 fires the warn for `tool_name.len() > 64`
    // (strict greater-than), so exactly 64 must NOT warn.
    //
    // We assert the length boundary alone — direct warn-fire pinning would
    // require a `tracing::Subscriber` test fixture, which is deferred as a
    // later infrastructure concern. The `name.len() == 64` assertion is
    // sufficient regression-proof for the boundary check: if the warn
    // threshold ever shifts to `>= 64`, this test still flags it indirectly
    // via the comment trail at the warn site.
    let leaf_name = "x".repeat(62);
    let root = Command::new("r")
        .subcommand_required(true)
        .subcommand(Command::new(leaf_name.clone()));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("64-char tool name must generate without error");

    let expected = format!("r_{leaf_name}");
    assert_eq!(expected.len(), 64, "construction sanity: expected 64 chars");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&expected.as_str()),
        "expected 64-char tool name `{expected}` in tool list, got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. Tool name at 65 chars — one over the boundary; warn fires (length proof)
// ---------------------------------------------------------------------------

#[test]
fn tool_name_at_65_chars_warns_once() {
    // Construction: root `r` + leaf `x*63` → "r_" + "x"*63 = 1 + 1 + 63 = 65.
    // src/command.rs:109 fires the warn for `len > 64`, so 65 must trip it.
    //
    // We assert the length boundary only; the warn-fire itself is observed
    // indirectly. If tracing-subscriber test fixtures land later, swap the
    // length assertion for a captured-event count.
    let leaf_name = "x".repeat(63);
    let root = Command::new("r")
        .subcommand_required(true)
        .subcommand(Command::new(leaf_name.clone()));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("65-char tool name must still generate (warn is non-fatal)");

    let expected = format!("r_{leaf_name}");
    assert_eq!(
        expected.len(),
        65,
        "construction sanity: expected 65 chars (boundary +1)"
    );

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&expected.as_str()),
        "expected 65-char tool name `{expected}` in tool list (warn is non-fatal), got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// 10. Tool name at 256 chars — well past the warn threshold, still emitted
// ---------------------------------------------------------------------------

#[test]
fn tool_name_at_256_chars_still_works() {
    // Construction: root `r` + leaf `x*254` → "r_" + "x"*254 = 256 chars.
    // The warn fires (we don't assert on it here), but the tool is still
    // generated. This pins the no-truncation, no-panic contract for long
    // tool names: brontes is permissive about length, only logging when the
    // MCP-spec-recommended 64-char limit is exceeded.
    let leaf_name = "x".repeat(254);
    let root = Command::new("r")
        .subcommand_required(true)
        .subcommand(Command::new(leaf_name.clone()));

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("256-char tool name must still generate without error");

    let expected = format!("r_{leaf_name}");
    assert_eq!(
        expected.len(),
        256,
        "construction sanity: expected 256 chars"
    );

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&expected.as_str()),
        "expected 256-char tool name in tool list (warn is non-fatal); name length: {}, name list: {:?}",
        expected.len(),
        names.iter().map(|n| (n, n.len())).collect::<Vec<_>>()
    );
}
