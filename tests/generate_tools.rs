//! End-to-end pins on the public `brontes::generate_tools` contract.
//!
//! These tests are the Rust analog of ophis `test/tools_test.go` (`TestGetTools`
//! and `TestCmdNamesToToolNames`). They drive `generate_tools` through its
//! public surface — `clap::Command` in, `Vec<rmcp::model::Tool>` out — and
//! pin two things you can only observe end-to-end:
//!
//! 1. Which commands in a walked tree survive `walk::should_filter` and reach
//!    the tool list when `Config` is left at defaults.
//! 2. How nested command paths map onto MCP tool names via the prefix +
//!    underscore-joining contract in [`crate::command::build_tool_name`].
//!
//! Unit tests inside `src/command.rs` cover `build_tool_name` and
//! `validate_paths` in isolation; this file exercises the orchestration.
//!
//! ## Note on the root command
//!
//! `walk::walk` includes the root command as the first entry, and the default
//! `should_filter` does not exclude a bare root (no `mcp` / `help` /
//! `completion` substring, not a group-only navigation node). Consequently
//! the root surfaces as its own tool whenever its name passes the substring
//! filter. The assertions below match that observed contract rather than
//! the "root is never a tool" claim in the original plan.

use clap::{Arg, Command};

use brontes::Config;

#[test]
fn generate_tools_returns_expected_tool_set() {
    // Two leaves under a plain root. With `Config::default()` no selectors are
    // configured, so every command that survives `should_filter` becomes a tool.
    // The root passes the filter (no "mcp" / "help" / "completion" substring,
    // not group-only), so we expect 3 tools: testcli, testcli_get, testcli_list.
    let root = Command::new("testcli")
        .subcommand(
            Command::new("get")
                .about("Get a resource")
                .arg(Arg::new("name").required(false)),
        )
        .subcommand(
            Command::new("list")
                .about("List resources")
                .arg(Arg::new("filter").long("filter")),
        );

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("generate_tools should succeed on a well-formed tree");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"testcli_get"),
        "expected testcli_get in {names:?}"
    );
    assert!(
        names.contains(&"testcli_list"),
        "expected testcli_list in {names:?}"
    );

    // Each emitted tool must have a non-empty description (the orchestrator
    // falls back to "Execute the {name} command" when no about/long_about is
    // set) and a populated input schema (the field is always present on
    // `rmcp::model::Tool`, never `None`).
    for tool in &tools {
        let desc = tool
            .description
            .as_ref()
            .unwrap_or_else(|| panic!("tool {:?} missing description", tool.name));
        assert!(
            !desc.is_empty(),
            "tool {:?} has empty description",
            tool.name
        );
        // input_schema is Arc<JsonObject>; serialize to confirm it has content.
        assert!(
            !tool.input_schema.is_empty(),
            "tool {:?} has empty input_schema",
            tool.name
        );
    }
}

#[test]
fn cmd_paths_to_tool_names() {
    // Build a tree such that the leaf paths produce three predictable tool
    // names via the prefix + underscore-joining contract:
    //   omctl get             → omctl_get
    //   omctl list all        → omctl_list_all
    //   omctl create this item → omctl_create_this_item
    //
    // Per `walk::should_filter`, intermediate group nodes ("list", "create",
    // "create this") are filtered out only when they are group-only AND have
    // no user-facing args. Here we mark them `.subcommand_required(true)` so
    // they ARE group-only, ensuring only the leaves surface — that's the
    // contract `TestCmdNamesToToolNames` pins.
    let root = Command::new("omctl")
        .subcommand_required(true)
        .subcommand(Command::new("get").about("Get a resource"))
        .subcommand(
            Command::new("list")
                .subcommand_required(true)
                .subcommand(Command::new("all").about("List all")),
        )
        .subcommand(
            Command::new("create").subcommand_required(true).subcommand(
                Command::new("this")
                    .subcommand_required(true)
                    .subcommand(Command::new("item").about("Create this item")),
            ),
        );

    let tools =
        brontes::generate_tools(&root, &Config::default()).expect("generate_tools should succeed");

    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort_unstable();

    // The root `omctl` is itself group-only (subcommand_required and no user
    // args), so it is filtered. Intermediate `list`, `create`, `create this`
    // are likewise group-only. Only the three leaves remain.
    let expected = vec!["omctl_create_this_item", "omctl_get", "omctl_list_all"];
    assert_eq!(
        names, expected,
        "tool names must match exactly the leaf paths after prefix + underscore joining"
    );
}

#[test]
fn empty_tree_returns_one_root_tool() {
    // ophis `TestGetTools` asserts an empty tree → empty tool list. brontes
    // differs: `walk::walk` always emits the root entry, and the default
    // `should_filter` does not exclude a bare root whose name has no "mcp" /
    // "help" / "completion" substring and which is not group-only. The
    // observed contract is therefore one tool (the root itself) rather than
    // zero. Pinning that behaviour here is the point of this test.
    let root = Command::new("testcli");

    let tools = brontes::generate_tools(&root, &Config::default())
        .expect("generate_tools should succeed on a root-only tree");

    assert_eq!(
        tools.len(),
        1,
        "root with no subcommands produces exactly one tool (the root itself)"
    );
    assert_eq!(tools[0].name.as_ref(), "testcli");
}

#[test]
fn single_leaf_tree_returns_root_and_leaf_tools() {
    // A root with one leaf produces two tools: the root (which passes every
    // filter) and the leaf. Pinning the leaf's tool name (`testcli_hello`)
    // is the parity assertion against ophis's
    // `TestCmdNamesToToolNames` single-leaf case.
    let root = Command::new("testcli").subcommand(Command::new("hello").about("Say hello"));

    let tools =
        brontes::generate_tools(&root, &Config::default()).expect("generate_tools should succeed");

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"testcli_hello"),
        "expected testcli_hello leaf tool in {names:?}"
    );
    assert_eq!(
        tools.len(),
        2,
        "root + one leaf produces exactly two tools: {names:?}"
    );
}
