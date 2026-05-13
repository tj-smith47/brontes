//! Iterative depth-first walker for a `clap::Command` tree.
//!
//! Produces a flat list of [`ResolvedCmd`] entries — each carrying a
//! reference to the underlying [`clap::Command`] and the full space-joined
//! path from the root. Path-keyed `Config` lookups (annotations, deprecated
//! commands, flag-schema overrides) use the `path` string built here, since
//! clap commands have no parent pointer.

use clap::Command;

use crate::config::Config;

/// A clap command with the path brontes derives by walking from the root.
///
/// Consumed by `generate_tools` to apply selectors and assemble the MCP tool
/// list.
#[derive(Debug)]
pub struct ResolvedCmd<'a> {
    /// The clap command this entry refers to.
    pub cmd: &'a Command,
    /// Space-joined path from the root command (e.g. `"my-cli mcp claude enable"`).
    pub path: String,
}

/// Walk the clap tree depth-first, producing a flat `Vec` of resolved
/// entries. The root command is included as the first entry.
///
/// Subcommands are visited in reverse registration order (the iterative DFS
/// pushes them onto a stack and pops). Order is deterministic across runs.
pub fn walk(root: &Command) -> Vec<ResolvedCmd<'_>> {
    let mut out = Vec::new();
    let mut stack: Vec<(&Command, Vec<&str>)> = vec![(root, vec![root.get_name()])];
    while let Some((cmd, parts)) = stack.pop() {
        let path = parts.join(" ");
        for sub in cmd.get_subcommands() {
            let mut p = parts.clone();
            p.push(sub.get_name());
            stack.push((sub, p));
        }
        out.push(ResolvedCmd { cmd, path });
    }
    out
}

/// Per the filter order: hidden → deprecated → group-only → segment-match.
/// Returns `true` if `cmd` should be EXCLUDED from the tool list.
pub fn should_filter(cmd: &Command, path: &str, cfg: &Config) -> bool {
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

    // 4. Segment-equality filter: remove the brontes subtree itself, the
    //    auto-injected `help` command, and any shell-completion command. The
    //    match is against space-delimited segments of the joined path so that
    //    `myapp mcp install` is caught (the middle segment equals `mcp`) but a
    //    consumer CLI named `make-mcp` or `my-mcp-tool` is NOT — its root
    //    segment merely *contains* `"mcp"` as a substring; no segment is
    //    exactly equal to it.
    let command_name = cfg.command_name.as_deref().unwrap_or("mcp");
    // Segment-equality (not substring). Earlier brontes ports used the ophis
    // substring shape, but the substring rule mis-filters consumer CLIs whose
    // root name happens to contain one of the needles (`make-mcp`, `helpful`,
    // `completionish`). Segment equality keeps the original intent — drop any
    // path that traverses through a `command_name`, `help`, or `completion`
    // node — without false positives on similarly-named roots.
    let needles = [command_name, "help", "completion"];
    if path.split(' ').any(|segment| needles.contains(&segment)) {
        return true;
    }

    false
}

/// A group-only command requires a subcommand AND defines no LOCAL
/// user-facing arguments.
///
/// Excluded from the count:
/// - clap's auto-injected `help` / `version` ids.
/// - Args propagated from an ancestor via `.global(true)`. `clap::Command::build()`
///   copies global args onto every descendant, so an intermediate group with no
///   args of its own can still report inherited globals from `get_arguments()`.
///   Counting those would mean a single root-level `--verbose` flag would
///   prevent every group node in the tree from being filtered.
///
/// Note: `is_global_set()` is also `true` for the arg on the command that
/// originally declared `.global(true)`. A leaf whose only declared arg is itself
/// `.global(true)` is therefore counted as having no local user args — but
/// leaves don't typically set `subcommand_required(true)`, so the `requires_sub`
/// gate keeps that case from being misclassified as group-only.
fn is_group_only(cmd: &Command) -> bool {
    let requires_sub = cmd.is_subcommand_required_set();
    let user_args_count = cmd
        .get_arguments()
        .filter(|a| {
            let id = a.get_id().as_str();
            id != "help" && id != "version" && !a.is_global_set()
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
            .find(|e| e.path.split(' ').count() == 3)
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

    #[test]
    fn is_group_only_excludes_globally_marked_args() {
        // clap propagates `.global(true)` args from a parent onto every
        // descendant when `Command::build()` runs. is_group_only must not
        // count those propagated copies as "local user args" — otherwise a
        // single root-level global flag would prevent every intermediate
        // group node in the tree from being filtered. `Arg::is_global_set()`
        // is true both for the originally-declared arg and for the
        // propagated copies, so filtering on it covers both shapes.
        let cmd = Command::new("group")
            .subcommand_required(true)
            .subcommand(Command::new("noop"))
            .arg(Arg::new("verbose").long("verbose").global(true));
        assert!(is_group_only(&cmd));
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
    fn mcp_cmd_filtered_by_segment() {
        // "mcp" is the default command_name; any path whose segment equals
        // "mcp" exactly is filtered.
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

    // ── Segment-equality: similar-named segments are NOT filtered ──────────
    //
    // Earlier brontes ports used a substring rule (matching ophis's
    // `AllowCmdsContaining`-flavoured filter). That misfired on consumer CLIs
    // whose root name happened to contain one of the needles (`make-mcp`,
    // `helpful`, `completionish`) — every tool in those trees was filtered
    // because the root segment substring-matched. Segment equality keeps the
    // intent (drop nodes whose path traverses through `command_name`, `help`,
    // or `completion`) without those false positives.

    #[test]
    fn segment_filter_skips_helpful_when_no_segment_equals_help() {
        // "helpful" merely contains "help" — no segment equals "help" exactly.
        let cmd = Command::new("helpful");
        let cfg = Config::default();
        assert!(
            !should_filter(&cmd, "myapp helpful", &cfg),
            "segment-equality rule must not filter 'helpful'"
        );
    }

    #[test]
    fn segment_filter_skips_completionish_when_no_segment_equals_completion() {
        let cmd = Command::new("completionish");
        let cfg = Config::default();
        assert!(
            !should_filter(&cmd, "myapp completionish", &cfg),
            "segment-equality rule must not filter 'completionish'"
        );
    }

    #[test]
    fn segment_filter_filters_exact_help_segment() {
        let cmd = Command::new("help");
        let cfg = Config::default();
        assert!(
            should_filter(&cmd, "myapp help", &cfg),
            "segment equal to 'help' must be filtered"
        );
    }

    #[test]
    fn segment_filter_filters_exact_completion_segment() {
        let cmd = Command::new("completion");
        let cfg = Config::default();
        assert!(
            should_filter(&cmd, "myapp completion", &cfg),
            "segment equal to 'completion' must be filtered"
        );
    }

    #[test]
    fn segment_filter_catches_mcp_segment_in_middle() {
        // `foo mcp install` — middle segment equals the command_name.
        let cmd = Command::new("install");
        let cfg = Config::default();
        assert!(
            should_filter(&cmd, "foo mcp install", &cfg),
            "interior segment equal to 'mcp' must be filtered"
        );
    }

    // ── make-mcp-style consumer root: substring-not-segment safety ─────────

    #[test]
    fn segment_filter_allows_make_mcp_root_and_descendants() {
        // Root name "make-mcp" contains the substring "mcp" but no segment
        // equals "mcp". Both the root and a leaf under it must survive.
        let cmd_root = Command::new("make-mcp");
        let cfg = Config::default();
        assert!(
            !should_filter(&cmd_root, "make-mcp", &cfg),
            "make-mcp root must not be filtered by the segment rule"
        );
        let cmd_leaf = Command::new("build");
        assert!(
            !should_filter(&cmd_leaf, "make-mcp build", &cfg),
            "make-mcp leaf must not be filtered by the segment rule"
        );
    }

    #[test]
    fn segment_filter_catches_mcp_segment_inside_make_mcp_tree() {
        // The make-mcp consumer attaches `brontes::command(...)` as the `mcp`
        // subtree; THAT subtree should still be filtered because its second
        // segment equals "mcp" exactly.
        let cmd = Command::new("mcp");
        let cfg = Config::default();
        assert!(
            should_filter(&cmd, "make-mcp mcp", &cfg),
            "make-mcp's nested `mcp` subtree must be filtered"
        );
        let cmd_leaf = Command::new("tools");
        assert!(
            should_filter(&cmd_leaf, "make-mcp mcp tools", &cfg),
            "make-mcp's `mcp tools` leaf must be filtered"
        );
    }

    // ── group-only rule (brontes substitute) ────────────────────────────────
    //
    // ophis filters any cobra.Command with `Run==nil && RunE==nil &&
    // PreRun==nil && PreRunE==nil`. clap has no `Run` field — every clap
    // command is dispatched by the user's `match` arm, and the library
    // cannot introspect dispatch intent. brontes ports the **subset** of
    // ophis's filter that survives the model gap: group-only (subcommand
    // required AND no user args).
    //
    // The cases below pin the current behaviour so future drift is caught:
    //
    // 1. group: subcommand_required, no user args            → FILTERED
    // 2. group with user args: subcommand_required + arg     → NOT filtered
    // 3. degenerate leaf: no subcommands, no user args       → NOT filtered
    //
    // Case (3) is the inverse of ophis's `Run == nil` leaf filter. clap
    // cannot detect "user forgot to wire this leaf into a match arm" — every
    // attached leaf is presumed intended. Filtering case (3) would silently
    // drop legitimate dispatch-by-name leaves (e.g. `mycli ping` with no
    // flags or subcommands). The non-port stands.

    #[test]
    fn pin_group_only_subcommand_required_no_args_is_filtered() {
        let cmd = Command::new("group")
            .subcommand_required(true)
            .subcommand(Command::new("leaf"));
        let cfg = Config::default();
        assert!(should_filter(&cmd, "myapp group", &cfg));
    }

    #[test]
    fn pin_leaf_with_user_args_is_not_filtered() {
        let cmd = Command::new("leaf").arg(Arg::new("name").long("name"));
        let cfg = Config::default();
        assert!(!should_filter(&cmd, "myapp leaf", &cfg));
    }

    #[test]
    fn pin_degenerate_leaf_no_args_no_subs_is_not_filtered() {
        // Inverse of ophis's `Run == nil` leaf filter. clap cannot
        // distinguish "intentional dispatch-by-name leaf" from "accidentally
        // wired stub", so this leaf survives. Locks the non-port against
        // future drift.
        let cmd = Command::new("ping");
        let cfg = Config::default();
        assert!(!should_filter(&cmd, "myapp ping", &cfg));
    }
}
