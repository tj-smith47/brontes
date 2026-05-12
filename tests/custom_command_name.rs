//! Port of ophis `tools_test.go::TestCustomCommandName`.
//!
//! Verifies that setting `Config.command_name` to a name other than `"mcp"`
//! changes the substring filter so that the user's own `mcp` service subtree
//! is NOT accidentally excluded from the generated tool list.

use clap::Command;

use brontes::Config;

#[test]
fn custom_command_name_does_not_filter_user_mcp_subtree() {
    // ophis tools_test.go::TestCustomCommandName (Phase 1 subset).
    //
    // The user's `mcp` subtree must survive because the brontes filter
    // uses Config.command_name ("agent") as the substring, not "mcp".
    let root = Command::new("myapp")
        .subcommand(
            Command::new("mcp")
                .about("MCP business service")
                .subcommand(Command::new("install").about("Install the mcp service")),
        )
        .subcommand(Command::new("status").about("Show status"));

    let cfg = Config::default().command_name("agent");

    let tools = brontes::generate_tools(&root, &cfg).expect("generate_tools should succeed");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    assert!(
        names.contains(&"myapp_mcp_install"),
        "user's mcp install should survive: got {names:?}"
    );
    assert!(
        names.contains(&"myapp_status"),
        "user's status should survive: got {names:?}"
    );
    // No "agent" tools — the brontes subtree is not built yet in Phase 1.
    assert!(
        !names.iter().any(|n| n.contains("agent")),
        "no agent tool should leak: {names:?}"
    );
}
