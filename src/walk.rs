//! Iterative depth-first walker for a `clap::Command` tree.
//!
//! Produces a flat list of [`ResolvedCmd`] entries — each carrying a
//! reference to the underlying [`clap::Command`], the full space-joined
//! path from the root, and the path's parts. Path-keyed `Config` lookups
//! (annotations, deprecated commands, flag-schema overrides) use the
//! `path` string built here, since clap commands have no parent pointer.

use clap::Command;

use crate::config::Config;

/// A clap command with the path brontes derives by walking from the root.
///
/// Consumed by `generate_tools` to apply selectors and assemble the MCP tool
/// list.
#[derive(Debug)]
pub(crate) struct ResolvedCmd<'a> {
    /// The clap command this entry refers to.
    pub cmd: &'a Command,
    /// Space-joined path from the root command (e.g. `"my-cli mcp claude enable"`).
    pub path: String,
    /// Path components in order (e.g. `["my-cli", "mcp", "claude", "enable"]`).
    #[allow(dead_code)]
    pub parts: Vec<&'a str>,
}

/// Walk the clap tree depth-first, producing a flat `Vec` of resolved
/// entries. The root command is included as the first entry.
///
/// Subcommands are visited in reverse registration order (the iterative DFS
/// pushes them onto a stack and pops). Order is deterministic across runs.
pub(crate) fn walk(root: &Command) -> Vec<ResolvedCmd<'_>> {
    let mut out = Vec::new();
    let mut stack: Vec<(&Command, Vec<&str>)> = vec![(root, vec![root.get_name()])];
    while let Some((cmd, parts)) = stack.pop() {
        let path = parts.join(" ");
        for sub in cmd.get_subcommands() {
            let mut p = parts.clone();
            p.push(sub.get_name());
            stack.push((sub, p));
        }
        out.push(ResolvedCmd { cmd, path, parts });
    }
    out
}

/// Per the filter order: hidden → deprecated → group-only → substring.
/// Returns `true` if `cmd` should be EXCLUDED from the tool list.
pub(crate) fn should_filter(cmd: &Command, path: &str, cfg: &Config) -> bool {
    // 1. Hidden commands are never exposed as tools.
    if cmd.is_hide_set() {
        return true;
    }

    // 2. Deprecated commands (recorded in the sidecar config, since clap has
    //    no built-in Deprecated field like cobra does).
    if cfg.deprecated_commands.contains(path) {
        return true;
    }

    // 3. Group-only: a command that requires a subcommand and defines no
    //    user-facing args of its own is a navigation node, not an action.
    if is_group_only(cmd) {
        return true;
    }

    // 4. Substring filter: remove the brontes subtree itself, the auto-injected
    //    `help` command, and any shell-completion command. The match is against
    //    the full space-joined path so that `myapp mcp install` is caught when
    //    `command_name == "mcp"`.
    let command_name = cfg.command_name.as_deref().unwrap_or("mcp");
    // Substring (not exact-match) by design — matches the ophis filter shape.
    // Consequence: a user command named "helpful" or "completion-server" will be
    // filtered because "help" / "completion" appear as substrings. Escape hatch for
    // the `command_name` token: rename via `Config.command_name` (the user-facing
    // knob that exists precisely for this).
    let needles = [command_name, "help", "completion"];
    if needles.iter().any(|n| path.contains(n)) {
        return true;
    }

    false
}

/// A group-only command requires a subcommand AND defines no user-facing
/// arguments. clap auto-injects `--help` (and `--version` on the root when
/// `Command::version` is set); those ids are excluded from the count.
fn is_group_only(cmd: &Command) -> bool {
    let requires_sub = cmd.is_subcommand_required_set();
    let user_args_count = cmd
        .get_arguments()
        .filter(|a| {
            let id = a.get_id().as_str();
            id != "help" && id != "version"
        })
        .count();
    requires_sub && user_args_count == 0
}

#[cfg(test)]
mod tests {
    use clap::Arg;

    use super::*;

    // ── Walk unit tests ────────────────────────────────────────────────────

    #[test]
    fn walk_includes_root_and_all_descendants() {
        let root = Command::new("root")
            .subcommand(Command::new("child-a").subcommand(Command::new("grandchild")))
            .subcommand(Command::new("child-b"));

        let entries = walk(&root);
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

        assert_eq!(entries.len(), 4);
        assert!(paths.contains(&"root"));
        assert!(paths.contains(&"root child-a"));
        assert!(paths.contains(&"root child-a grandchild"));
        assert!(paths.contains(&"root child-b"));
    }

    #[test]
    fn walk_path_is_space_joined() {
        let root = Command::new("root")
            .subcommand(Command::new("child").subcommand(Command::new("grandchild")));

        let entries = walk(&root);
        let deepest = entries
            .iter()
            .find(|e| e.parts.len() == 3)
            .expect("grandchild entry present");
        assert_eq!(deepest.path, "root child grandchild");
    }

    #[test]
    fn walk_handles_root_with_no_subs() {
        let root = Command::new("leaf");
        let entries = walk(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "leaf");
    }

    #[test]
    fn is_group_only_detects_subcommand_required() {
        let cmd = Command::new("group")
            .subcommand_required(true)
            .subcommand(Command::new("noop"));
        assert!(is_group_only(&cmd));
    }

    #[test]
    fn is_group_only_false_when_user_args_present() {
        let cmd = Command::new("group")
            .subcommand_required(true)
            .subcommand(Command::new("noop"))
            .arg(Arg::new("foo"));
        assert!(!is_group_only(&cmd));
    }

    #[test]
    fn is_group_only_false_when_no_subcommand_required() {
        let cmd = Command::new("leaf");
        assert!(!is_group_only(&cmd));
    }

    // ── TestCmdFilter parity port (ophis config_test.go::TestCmdFilter) ───
    //
    // Dropped row: "no run cmd" — ophis filters cobra commands whose Run field is
    // nil (i.e. navigation-only nodes). clap has no equivalent "run function" field;
    // the group-only rule (is_group_only) covers the navigation-node case.

    #[test]
    fn passing_cmd_not_filtered() {
        let cmd = Command::new("test");
        let cfg = Config::default();
        assert!(!should_filter(&cmd, "test", &cfg));
    }

    #[test]
    fn deprecated_cmd_filtered() {
        let cmd = Command::new("test");
        let cfg = Config::default().deprecate("test");
        assert!(should_filter(&cmd, "test", &cfg));
    }

    #[test]
    fn hidden_cmd_filtered() {
        let cmd = Command::new("test").hide(true);
        let cfg = Config::default();
        assert!(should_filter(&cmd, "test", &cfg));
    }

    #[test]
    fn mcp_cmd_filtered_by_substring() {
        // "mcp" is the default command_name; any path containing "mcp" is filtered.
        let cmd = Command::new("mcp");
        let cfg = Config::default();
        assert!(should_filter(&cmd, "mcp", &cfg));
    }

    #[test]
    fn group_only_cmd_filtered() {
        // brontes-specific case substituting ophis's Run==nil filter.
        let cmd = Command::new("platform")
            .subcommand_required(true)
            .subcommand(Command::new("noop"));
        let cfg = Config::default();
        assert!(should_filter(&cmd, "platform", &cfg));
    }

    // ── TestCmdFilterCustomCommandName parity port ────────────────────────

    #[test]
    fn agent_cmd_filtered_when_command_name_is_agent() {
        let cmd = Command::new("agent");
        let cfg = Config::default().command_name("agent");
        assert!(should_filter(&cmd, "agent", &cfg));
    }

    #[test]
    fn mcp_cmd_passes_when_command_name_is_agent() {
        let cmd = Command::new("mcp");
        let cfg = Config::default().command_name("agent");
        assert!(!should_filter(&cmd, "mcp", &cfg));
    }

    #[test]
    fn normal_cmd_passes_when_command_name_is_agent() {
        let cmd = Command::new("status");
        let cfg = Config::default().command_name("agent");
        assert!(!should_filter(&cmd, "status", &cfg));
    }

    // ── command_name default ──────────────────────────────────────────────

    #[test]
    fn command_name_defaults_to_mcp_when_unset() {
        let cfg = Config::default();
        assert_eq!(cfg.command_name.as_deref().unwrap_or("mcp"), "mcp");
    }

    // ── Substring quirk: false positives on similar names ──────────────────

    #[test]
    fn substring_filter_catches_inner_help_token() {
        // Pin the substring-not-exact-match quirk: a command named "helpful" is
        // filtered because "help" appears in its path.
        let cmd = Command::new("helpful");
        let cfg = Config::default();
        assert!(
            should_filter(&cmd, "myapp helpful", &cfg),
            "substring 'help' matches inside 'helpful' — intentional quirk"
        );
    }

    #[test]
    fn substring_filter_catches_inner_completion_token() {
        let cmd = Command::new("completionish");
        let cfg = Config::default();
        assert!(
            should_filter(&cmd, "myapp completionish", &cfg),
            "substring 'completion' matches inside 'completionish' — intentional quirk"
        );
    }
}
