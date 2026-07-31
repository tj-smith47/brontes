//! Shared helpers reused by multiple `mcp` leaves.
//!
//! Hosts [`parse_log_level`], which both `mcp start` and `mcp stream` need
//! with byte-identical semantics — including the `tracing::warn!` on
//! unrecognized values that is a deliberate divergence from ophis (which
//! silently maps unknown levels to `Info`) — and the tool-selection flags,
//! which all three leaves carry so that `mcp tools` can preview exactly what
//! `mcp start` and `mcp stream` would serve.

use clap::{Arg, ArgAction, ArgMatches, Command};
use tracing::Level;

use crate::config::Config;
use crate::toolset::ToolFilter;

/// Clap ids for the six tool-selection flags, paired with their long names.
///
/// Kept as one table so a leaf cannot pick up `--group` without also picking
/// up `--hide-group`: a half-present matrix is the kind of drift that makes
/// two of these commands answer differently for the same arguments.
const SELECTION_FLAGS: [(&str, &str, &str, &str); 6] = [
    (
        "group",
        "group",
        "NAME",
        "Expose only this group of commands (repeat for several)",
    ),
    (
        "command",
        "command",
        "PATH",
        "Expose only this command and its subcommands, e.g. --command \"cli release\" (repeat for several)",
    ),
    (
        "tool",
        "tool",
        "NAME",
        "Expose only this MCP tool, by its generated name (repeat for several)",
    ),
    (
        "hide-group",
        "hide-group",
        "NAME",
        "Remove this group from the tool list (repeat for several)",
    ),
    (
        "hide-command",
        "hide-command",
        "PATH",
        "Remove this command and its subcommands (repeat for several)",
    ),
    (
        "hide-tool",
        "hide-tool",
        "NAME",
        "Remove this MCP tool, by its generated name (repeat for several)",
    ),
];

/// Attach the tool-selection flags to an `mcp` leaf.
///
/// The set is deliberately a 2×3 matrix — expose or hide, by group, command,
/// or tool — so a user who learns one flag has learned all six.
pub fn with_selection_flags(mut cmd: Command) -> Command {
    for (id, long, value_name, help) in SELECTION_FLAGS {
        cmd = cmd.arg(
            Arg::new(id)
                .long(long)
                .action(ArgAction::Append)
                .value_name(value_name)
                .help(help),
        );
    }
    cmd
}

/// Fold the tool-selection flags from `matches` onto `cfg`.
///
/// The flags merge with whatever the CLI's author already pinned in
/// [`Config::tool_filter`] rather than replacing it, so a developer who ships
/// a hidden command keeps it hidden no matter what the launch line asks for.
pub fn apply_selection_flags(cfg: Config, matches: &ArgMatches) -> Config {
    let values = |id: &str| -> std::collections::BTreeSet<String> {
        matches
            .get_many::<String>(id)
            .map(|vals| vals.cloned().collect())
            .unwrap_or_default()
    };

    let from_flags = ToolFilter {
        groups: values("group"),
        commands: values("command"),
        tools: values("tool"),
        hidden_groups: values("hide-group"),
        hidden_commands: values("hide-command"),
        hidden_tools: values("hide-tool"),
        ..Default::default()
    };

    let mut cfg = cfg;
    cfg.tool_filter = cfg.tool_filter.clone().merged_with(&from_flags);
    cfg
}

/// Parse the `--log-level` flag into a [`Level`] when present.
///
/// Invalid values return `None` (i.e., fall through to `Config::log_level`
/// or `RUST_LOG`); a `tracing::warn!` records the offending value so users
/// notice the typo at startup rather than wondering why their level had
/// no effect. This is a deliberate divergence from ophis's silent
/// fallback to `Info`.
///
/// Shared between `mcp start` (stdio transport) and `mcp stream`
/// (streamable-HTTP transport) so the two surfaces cannot drift in their
/// `--log-level` parsing semantics. Each surface's `parse_log_level`
/// wrapper exists to keep the call sites in their own module but
/// delegates here without modification.
pub fn parse_log_level(matches: &ArgMatches) -> Option<Level> {
    let raw = matches.get_one::<String>("log-level")?;
    match raw.to_ascii_lowercase().as_str() {
        "trace" => Some(Level::TRACE),
        "debug" => Some(Level::DEBUG),
        "info" => Some(Level::INFO),
        "warn" | "warning" => Some(Level::WARN),
        "error" => Some(Level::ERROR),
        other => {
            tracing::warn!(value = %other, "unrecognized --log-level; falling back to default");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> ArgMatches {
        with_selection_flags(Command::new("leaf"))
            .try_get_matches_from(argv)
            .expect("parses")
    }

    #[test]
    fn every_selection_flag_repeats() {
        let matches = parse(&[
            "leaf",
            "--group",
            "release",
            "--group",
            "inspect",
            "--command",
            "cli publish",
            "--tool",
            "cli_status",
            "--hide-group",
            "dangerous",
            "--hide-command",
            "cli secrets",
            "--hide-tool",
            "cli_secrets_get",
        ]);
        let cfg = apply_selection_flags(Config::default(), &matches);

        assert_eq!(cfg.tool_filter.groups.len(), 2);
        assert!(cfg.tool_filter.commands.contains("cli publish"));
        assert!(cfg.tool_filter.tools.contains("cli_status"));
        assert!(cfg.tool_filter.hidden_groups.contains("dangerous"));
        assert!(cfg.tool_filter.hidden_commands.contains("cli secrets"));
        assert!(cfg.tool_filter.hidden_tools.contains("cli_secrets_get"));
    }

    #[test]
    fn no_flags_leaves_the_filter_untouched() {
        let matches = parse(&["leaf"]);
        let cfg = apply_selection_flags(Config::default(), &matches);
        assert!(cfg.tool_filter.is_empty());
    }

    #[test]
    fn flags_merge_onto_a_developer_pinned_filter() {
        let matches = parse(&["leaf", "--group", "release"]);
        let cfg = apply_selection_flags(Config::default().hide_command("cli secrets"), &matches);

        assert!(cfg.tool_filter.groups.contains("release"));
        assert!(
            cfg.tool_filter.hidden_commands.contains("cli secrets"),
            "a launch flag must not drop what the CLI's author hid"
        );
    }
}
