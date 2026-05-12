//! [`generate_tools`]: build the MCP tool list for a clap command tree.
//!
//! Call [`generate_tools`] with the root [`clap::Command`] and a [`Config`]
//! to produce the list of MCP tools ready to register with a server.  This
//! is the primary entry point for brontes consumers.
//!
//! # Quick start
//!
//! ```rust
//! use clap::Command;
//! use brontes::Config;
//!
//! let root = Command::new("myapp")
//!     .subcommand(Command::new("deploy").about("Deploy the app"));
//!
//! let cfg = Config::default();
//! let tools = brontes::generate_tools(&root, &cfg).expect("valid config");
//! assert!(!tools.is_empty());
//! ```

use std::collections::HashSet;

use clap::Command;
use rmcp::model::Tool;

use crate::Result;
use crate::config::Config;
use crate::selector::FlagMatcher;

/// Walk `root`, apply safety filters, apply first-match-wins selectors,
/// and produce the MCP tool list ready to register with a server.
///
/// Returns `Err(`[`crate::Error::Config`]`)` if any of these path-keyed
/// [`Config`] entries names a command path that does not appear in the
/// walked tree (after safety filtering):
///
/// - [`Config::annotations`] keys
/// - [`Config::deprecated_commands`] entries
/// - [`Config::flag_schemas`] and [`Config::flag_type_overrides`] keys
///   (both the `cmd_path` component AND the flag name on that command)
/// - String args captured by the built-in selector factories
///   ([`crate::selectors::allow_cmds`] and friends)
///
/// Hand-rolled `Arc<dyn Fn>` matchers are NOT validated — only matchers
/// built via the introspectable factories in [`crate::selectors`] are
/// checked at build time.
///
/// When [`Config::selectors`] is empty every command that passes the safety
/// filters becomes a tool. When non-empty, a command must be claimed by at
/// least one selector; commands not claimed are excluded.
///
/// # Errors
///
/// Returns [`crate::Error::Config`] if any path-keyed [`Config`] entry
/// references a command path or flag name that does not exist in the walked
/// tree, or if an introspectable selector factory captured an unknown path.
pub fn generate_tools(root: &Command, cfg: &Config) -> Result<Vec<Tool>> {
    // 1. Walk the tree.
    let resolved = crate::walk::walk(root);

    // 2. Build-time path validation.
    validate_paths(&resolved, cfg)?;

    // 3. Compute the effective prefix (PLAN §5.6).
    let prefix = cfg
        .tool_name_prefix
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| root.get_name());

    // 4. Build tools.
    let mut tools = Vec::new();
    for entry in &resolved {
        if crate::walk::should_filter(entry.cmd, &entry.path, cfg) {
            continue;
        }

        // First-match-wins selector evaluation.
        let matched_selector: Option<&crate::selector::Selector> = if cfg.selectors.is_empty() {
            None // no selectors → include unconditionally, no flag filtering
        } else {
            let found = cfg.selectors.iter().find(|sel| match &sel.cmd {
                Some(m) => m(&entry.path),
                None => true, // None cmd matcher claims every passing command
            });
            match found {
                Some(sel) => Some(sel),
                None => continue, // no selector claimed this command
            }
        };

        let tool_name = build_tool_name(&entry.path, prefix);
        if tool_name.len() > 64 {
            tracing::warn!(
                target: "brontes::command",
                name = %tool_name,
                len = tool_name.len(),
                "MCP tool name exceeds 64 characters; consider setting Config.tool_name_prefix"
            );
        }

        // Extract flag matchers from the claimed selector (if any).
        let local_flag: Option<&FlagMatcher> = matched_selector.and_then(|s| s.local_flag.as_ref());
        let inherited_flag: Option<&FlagMatcher> =
            matched_selector.and_then(|s| s.inherited_flag.as_ref());

        let input_schema = crate::schema::build_input_schema_with_matchers(
            entry.cmd,
            cfg,
            &entry.path,
            local_flag,
            inherited_flag,
        );
        let output_schema = crate::schema::build_output_schema();
        let description = crate::schema::build_description(entry.cmd);

        let annotations = cfg
            .annotations
            .get(&entry.path)
            .and_then(crate::annotations::ToolAnnotations::to_rmcp);

        let mut tool = Tool::new(tool_name, description, input_schema);
        tool.output_schema = Some(output_schema);
        tool.annotations = annotations;
        tools.push(tool);
    }

    Ok(tools)
}

/// Build the MCP tool name for a command.
///
/// Replaces only the first space-delimited token (the root command name)
/// with `prefix`, then converts remaining spaces to underscores.
/// Hyphens inside subcommand names are preserved verbatim.
///
/// # Examples
///
/// ```text
/// path "omnistrate-ctl cost by-cell list", prefix "omctl"
///   → "omctl_cost_by-cell_list"
///
/// path "myapp", prefix "myapp"
///   → "myapp"
/// ```
fn build_tool_name(path: &str, prefix: &str) -> String {
    // Preserve everything after the first space (the subcommand portion).
    let after_first = path.find(' ').map_or("", |i| &path[i..]);
    let body = format!("{prefix}{after_first}");
    body.replace(' ', "_")
}

// ---------------------------------------------------------------------------
// Build-time path validation (PLAN §2.7)
// ---------------------------------------------------------------------------

fn validate_paths(resolved: &[crate::walk::ResolvedCmd<'_>], cfg: &Config) -> Result<()> {
    // Validate against the full walked tree (before safety filtering) so
    // that deprecated command paths are still considered valid to name.
    let valid_paths: HashSet<&str> = resolved.iter().map(|r| r.path.as_str()).collect();

    // annotations
    for path in cfg.annotations.keys() {
        if !valid_paths.contains(path.as_str()) {
            return Err(crate::Error::Config(format!(
                "Config.annotations references unknown command path {path:?}"
            )));
        }
    }

    // deprecated_commands
    for path in &cfg.deprecated_commands {
        if !valid_paths.contains(path.as_str()) {
            return Err(crate::Error::Config(format!(
                "Config.deprecated_commands references unknown command path {path:?}"
            )));
        }
    }

    // flag_schemas: both the path and the flag name must exist.
    for (path, flag) in cfg.flag_schemas.keys() {
        validate_flag_path(resolved, &valid_paths, path, flag, "flag_schemas")?;
    }

    // flag_type_overrides: same validation.
    for (path, flag) in cfg.flag_type_overrides.keys() {
        validate_flag_path(resolved, &valid_paths, path, flag, "flag_type_overrides")?;
    }

    // Selector factory captured strings (only introspectable matchers).
    for sel in &cfg.selectors {
        if let Some(matcher) = &sel.cmd
            && let Some(spec) = crate::selectors::lookup(matcher)
        {
            match spec.kind {
                // allow_cmds / exclude_cmds: exact paths must exist.
                crate::selectors::MatcherKind::AllowCmds
                | crate::selectors::MatcherKind::ExcludeCmds => {
                    for s in &spec.args {
                        if !valid_paths.contains(s.as_str()) {
                            return Err(crate::Error::Config(format!(
                                "Selector references unknown command path {s:?}"
                            )));
                        }
                    }
                }
                // Substrings: soft warn — substring intent is permissive.
                crate::selectors::MatcherKind::AllowCmdsContaining
                | crate::selectors::MatcherKind::ExcludeCmdsContaining => {
                    for s in &spec.args {
                        if !valid_paths.iter().any(|p| p.contains(s.as_str())) {
                            tracing::warn!(
                                target: "brontes::command",
                                needle = %s,
                                "Selector substring matches no walked command path"
                            );
                        }
                    }
                }
                _ => {} // flag-matcher kinds not validated here
            }
        }
    }

    Ok(())
}

fn validate_flag_path(
    resolved: &[crate::walk::ResolvedCmd<'_>],
    valid_paths: &HashSet<&str>,
    path: &str,
    flag: &str,
    config_field: &str,
) -> Result<()> {
    if !valid_paths.contains(path) {
        return Err(crate::Error::Config(format!(
            "Config.{config_field} references unknown command path {path:?}"
        )));
    }
    // Verify the flag exists on that command.
    if let Some(r) = resolved.iter().find(|r| r.path == path) {
        let has_flag = r.cmd.get_arguments().any(|a| a.get_id().as_str() == flag);
        if !has_flag {
            return Err(crate::Error::Config(format!(
                "Config.{config_field} references unknown flag {flag:?} on command {path:?}"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clap::{Arg, Command};

    use super::*;
    use crate::annotations::ToolAnnotations;
    use crate::config::Config;

    // ── build_tool_name ───────────────────────────────────────────────────────

    #[test]
    fn build_tool_name_omctl_case() {
        // PLAN §5.6 canonical case.
        let name = build_tool_name("omnistrate-ctl cost by-cell list", "omctl");
        assert_eq!(name, "omctl_cost_by-cell_list");
    }

    #[test]
    fn build_tool_name_single_token() {
        // Root-only path: no spaces, no underscores.
        let name = build_tool_name("myapp", "myapp");
        assert_eq!(name, "myapp");
    }

    #[test]
    fn build_tool_name_preserves_hyphens() {
        // Hyphens inside subcommand names survive.
        let name = build_tool_name("myapp by-cell list", "myapp");
        assert_eq!(name, "myapp_by-cell_list");
    }

    #[test]
    fn build_tool_name_prefix_substitution() {
        // Prefix replaces root name; spaces become underscores.
        let name = build_tool_name("myapp mcp install", "myapp");
        assert_eq!(name, "myapp_mcp_install");
    }

    // ── validate_paths ────────────────────────────────────────────────────────

    fn root_with_list() -> Command {
        Command::new("myapp").subcommand(Command::new("list").arg(Arg::new("limit").long("limit")))
    }

    #[test]
    fn validate_paths_rejects_unknown_annotation() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().annotation(
            "nonexistent path",
            ToolAnnotations {
                read_only_hint: Some(true),
                ..Default::default()
            },
        );
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(result, Err(crate::Error::Config(_))),
            "expected Config error, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_accepts_known_path() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().annotation(
            "myapp list",
            ToolAnnotations {
                read_only_hint: Some(true),
                ..Default::default()
            },
        );
        assert!(validate_paths(&resolved, &cfg).is_ok());
    }

    #[test]
    fn validate_paths_rejects_unknown_flag_on_known_path() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg =
            Config::default().flag_schema("myapp list", "nonexistent-flag", serde_json::json!({}));
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(result, Err(crate::Error::Config(_))),
            "expected Config error for unknown flag, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_accepts_known_flag() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().flag_schema(
            "myapp list",
            "limit",
            serde_json::json!({"type": "integer"}),
        );
        assert!(validate_paths(&resolved, &cfg).is_ok());
    }

    // ── first-match-wins ──────────────────────────────────────────────────────

    #[test]
    fn first_match_wins_ordering() {
        // Two selectors both claim the "myapp status" path; only the first
        // should produce a tool (the loop breaks after the first match).
        let root = Command::new("myapp").subcommand(Command::new("status").about("Show status"));
        let cfg = Config::default()
            .selector(crate::selector::Selector {
                cmd: Some(Arc::new(|p: &str| p == "myapp status")),
                ..Default::default()
            })
            .selector(crate::selector::Selector {
                cmd: Some(Arc::new(|p: &str| p == "myapp status")),
                ..Default::default()
            });

        let tools = generate_tools(&root, &cfg).expect("should succeed");
        // With two selectors both accepting "myapp status", there must be
        // exactly one tool (not two).
        let status_tools: Vec<_> = tools.iter().filter(|t| t.name.contains("status")).collect();
        assert_eq!(
            status_tools.len(),
            1,
            "first-match-wins: only one tool per command"
        );
    }

    // ── no selectors → include all ────────────────────────────────────────────

    #[test]
    fn no_selectors_means_include_all() {
        // Three leaf commands; no selectors. All three should appear as tools.
        let root = Command::new("myapp")
            .subcommand(Command::new("list").about("List"))
            .subcommand(Command::new("create").about("Create"))
            .subcommand(Command::new("delete").about("Delete"));

        let cfg = Config::default();
        let tools = generate_tools(&root, &cfg).expect("should succeed");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(
            names.contains(&"myapp_list"),
            "missing myapp_list: {names:?}"
        );
        assert!(
            names.contains(&"myapp_create"),
            "missing myapp_create: {names:?}"
        );
        assert!(
            names.contains(&"myapp_delete"),
            "missing myapp_delete: {names:?}"
        );
    }

    // ── 64-char warn ──────────────────────────────────────────────────────────

    #[test]
    fn tool_long_name_still_generated() {
        // Build a tree where the tool name would exceed 64 chars.
        // The warn is non-fatal; the tool must still be generated.
        let long_sub = "a-very-long-subcommand-name-exceeding-sixty-four-characters-total";
        let root =
            Command::new("myapp").subcommand(Command::new(long_sub).about("Long named command"));

        let cfg = Config::default();
        let tools = generate_tools(&root, &cfg).expect("should succeed");
        let found = tools.iter().any(|t| t.name.contains(long_sub));
        assert!(found, "long-named tool must still be generated");
    }
}
