//! Tool-name parity ports from ophis (PLAN §5.6).
//!
//! Verifies that `generate_tools` builds MCP tool names according to the
//! prefix-substitution rule: only the first space-delimited token (the root
//! command name) is replaced by the configured prefix; remaining tokens are
//! joined with underscores; hyphens inside token names are preserved.

use clap::Command;

use brontes::Config;

#[test]
fn omctl_cost_by_cell_list_naming() {
    // PLAN §5.6 canonical case: long command path with explicit prefix.
    let root = Command::new("omnistrate-ctl").subcommand(
        Command::new("cost").subcommand_required(true).subcommand(
            Command::new("by-cell")
                .subcommand_required(true)
                .subcommand(Command::new("list").about("List by cell")),
        ),
    );

    let cfg = Config::default().tool_name_prefix("omctl");

    let tools = brontes::generate_tools(&root, &cfg).expect("generate_tools should succeed");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    assert!(
        names.contains(&"omctl_cost_by-cell_list"),
        "expected omctl_cost_by-cell_list in {names:?}"
    );
}

#[test]
fn myapp_default_prefix_no_explicit_setting() {
    // tools_test.go:122-135 case: no tool_name_prefix set, default to root name.
    let root = Command::new("myapp").subcommand(
        Command::new("mcp")
            .about("MCP business service")
            .subcommand(Command::new("install").about("Install MCP")),
    );

    // command_name = "agent" means the filter uses "agent" as the substring,
    // so the user's "mcp" subtree is NOT filtered out.
    let cfg = Config::default().command_name("agent");

    let tools = brontes::generate_tools(&root, &cfg).expect("generate_tools should succeed");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    // The root name is the implicit prefix; mcp install → myapp_mcp_install.
    assert!(
        names.contains(&"myapp_mcp_install"),
        "expected myapp_mcp_install in {names:?}"
    );
}
