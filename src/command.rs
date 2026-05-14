//! Public command-tree API: [`generate_tools`] (offline tool-list build) plus
//! the runtime entry points [`command`], [`handle`], and [`run`] that mount
//! and dispatch the `mcp` subtree.
//!
//! # Quick start — tool-list only
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
//!
//! # Quick start — full MCP server (two lines)
//!
//! ```no_run
//! use clap::Command;
//!
//! #[tokio::main]
//! async fn main() -> brontes::Result<()> {
//!     let cli = Command::new("my-cli")
//!         .version("0.1.0")
//!         .subcommand(Command::new("greet").about("Say hi"))
//!         .subcommand(brontes::command(None));          // [1] mount
//!
//!     let matches = cli.clone().get_matches();
//!     match matches.subcommand() {
//!         Some(("mcp", sub)) => brontes::handle(sub, &cli, None).await,  // [2] dispatch
//!         Some(("greet", _)) => { println!("hi"); Ok(()) }
//!         _ => Ok(()),
//!     }
//! }
//! ```

use std::collections::HashSet;

use clap::Command;
use rmcp::model::Tool;

use crate::Result;
use crate::config::Config;
use crate::selector::{FlagMatcher, Middleware};

/// A walked-and-filtered command paired with the runtime data the MCP server
/// needs to dispatch a tool call: the MCP [`Tool`] descriptor, the optional
/// [`Middleware`] claimed by the matching selector, and the space-joined clap
/// path (handy for diagnostic messages even though [`crate::exec::run_tool`]
/// reconstructs argv from the tool name).
///
/// This type is internal: it is the cache shape held by
/// [`crate::server::BrontesServer`] so that
/// [`generate_tools_with_middleware`] runs exactly once at server
/// construction. Downstream consumers continue to use [`generate_tools`],
/// which projects this struct down to a plain `Vec<Tool>` for offline
/// inspection.
pub struct ResolvedTool {
    /// The MCP tool descriptor handed back from `tools/list`.
    pub tool: Tool,
    /// Middleware from the selector that claimed this command, if any.
    /// `None` means the exec step runs unwrapped.
    pub middleware: Option<Middleware>,
    /// Space-joined clap command path (e.g. `"my-cli deploy prod"`).
    /// Included in MCP tool-error messages so operators can see which
    /// underlying CLI command failed. Argv construction lives in
    /// [`crate::exec::build_command_args`] which keys off the MCP tool name.
    pub command_path: String,
}

/// Walk `root`, apply safety filters, apply first-match-wins selectors,
/// and produce the MCP tool list ready to register with a server.
///
/// Returns <code>Err([crate::Error::Config])</code> if any of these path-keyed
/// [`Config`] entries names a command path that does not appear in the
/// walked tree (after safety filtering):
///
/// - [`Config::annotations`] keys
/// - [`Config::deprecated_commands`] entries
/// - [`Config::flag_schemas`] and [`Config::flag_type_overrides`] keys
///   (both the `cmd_path` component AND the flag name on that command)
/// - [`Config::descriptions`] keys (additionally, the override text must be
///   non-empty after `trim` — an empty description is rejected outright)
/// - [`Config::description_modes`] keys
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
/// # Substring filter
///
/// Commands whose space-joined path contains any of `command_name` /
/// `"help"` / `"completion"` are excluded. The substring is matched
/// permissively — a command named `"helpful"` is also excluded because
/// `"help"` appears as a substring. See [`Config::command_name`] for
/// the rename escape hatch.
///
/// # Errors
///
/// Returns [`crate::Error::Config`] if any path-keyed [`Config`] entry
/// references a command path or flag name that does not exist in the walked
/// tree, if a [`Config::descriptions`] entry holds empty/whitespace-only
/// text, or if an introspectable selector factory captured an unknown path.
pub fn generate_tools(root: &Command, cfg: &Config) -> Result<Vec<Tool>> {
    Ok(generate_tools_with_middleware(root, cfg)?
        .into_iter()
        .map(|r| r.tool)
        .collect())
}

/// Same walk + selector pass as [`generate_tools`], but retains each tool's
/// claimed [`Middleware`] (if any) and the clap command path alongside the
/// MCP [`Tool`] descriptor.
///
/// This is the runtime feed for [`crate::server::BrontesServer`]: when a
/// `tools/call` request arrives, the server needs to invoke the middleware
/// chain claimed by the same selector that produced the tool descriptor.
/// Building both halves in one pass keeps the selector-evaluation logic
/// single-sourced.
///
/// # Errors
///
/// Same conditions as [`generate_tools`].
pub fn generate_tools_with_middleware(root: &Command, cfg: &Config) -> Result<Vec<ResolvedTool>> {
    // clap propagates `.global(true)` args lazily on `Command::build()`.
    // Clone-then-build ensures every walked command's `get_arguments()`
    // includes inherited globals, so path validation and schema building
    // see a consistent view of which flags exist on each command.
    let mut built = root.clone();
    built.build();

    // 1. Walk the (now-built) tree.
    let resolved = crate::walk::walk(&built);

    // 2. Build-time path validation.
    validate_paths(&resolved, cfg)?;

    // 3. Compute the effective prefix: `tool_name_prefix` override when non-empty,
    //    otherwise fall back to the root command name.
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
            // `None` cmd matcher claims every passing command.
            let found = cfg
                .selectors
                .iter()
                .find(|sel| sel.cmd.as_ref().is_none_or(|m| m(&entry.path)));
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

        // Extract flag matchers and middleware from the claimed selector (if any).
        let local_flag: Option<&FlagMatcher> = matched_selector.and_then(|s| s.local_flag.as_ref());
        let inherited_flag: Option<&FlagMatcher> =
            matched_selector.and_then(|s| s.inherited_flag.as_ref());
        let middleware: Option<Middleware> = matched_selector.and_then(|s| s.middleware.clone());

        let input_schema = crate::schema::build_input_schema_with_matchers(
            entry.cmd,
            cfg,
            &entry.path,
            local_flag,
            inherited_flag,
        );
        let output_schema = crate::schema::build_output_schema();
        let description = crate::schema::build_description(entry.cmd, cfg, &entry.path);

        let annotations = cfg
            .annotations
            .get(&entry.path)
            .and_then(crate::annotations::ToolAnnotations::to_rmcp);

        let mut tool = Tool::new(tool_name, description, input_schema);
        tool.output_schema = Some(output_schema);
        tool.annotations = annotations;
        tools.push(ResolvedTool {
            tool,
            middleware,
            command_path: entry.path.clone(),
        });
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
    // Final pass collapses ALL spaces to underscores, including any in `prefix`.
    // This is a uniform post-process — consumers passing a prefix with spaces
    // get the same treatment as path tokens with embedded spaces.
    body.replace(' ', "_")
}

// ---------------------------------------------------------------------------
// Build-time path validation: every selector/config path must match an existing
// command in the walked tree, so misconfiguration surfaces at startup rather
// than silently no-oping at request time.
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

    // descriptions: path must exist AND the override text must be non-empty
    // after trim. An empty description is poor for LLM tool selection, so
    // reject it at build time rather than silently shipping it.
    for (path, text) in &cfg.descriptions {
        if !valid_paths.contains(path.as_str()) {
            return Err(crate::Error::Config(format!(
                "Config.descriptions references unknown command path {path:?}"
            )));
        }
        if text.trim().is_empty() {
            return Err(crate::Error::Config(format!(
                "description override for command path '{path}' is empty; \
                 description text must be non-empty"
            )));
        }
    }

    // description_modes: path must exist.
    for path in cfg.description_modes.keys() {
        if !valid_paths.contains(path.as_str()) {
            return Err(crate::Error::Config(format!(
                "Config.description_modes references unknown command path {path:?}"
            )));
        }
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
// Public MCP-subtree API: command(), handle(), run().
// ---------------------------------------------------------------------------

/// Default name of the mcp subcommand group; mirrors ophis (`config.go:81`).
const DEFAULT_COMMAND_NAME: &str = "mcp";

/// Build the `mcp` subcommand subtree, ready to mount on a parent CLI.
///
/// `cfg` is the optional brontes configuration. `None` and
/// `Some(&Config::default())` produce identical behavior. The returned
/// [`Command`] has the configured group name ([`Config::command_name`],
/// default `"mcp"`) and registers the `start`, `tools`, and `stream`
/// children.
///
/// Returns a plain [`Command`] (not a `Result`) so the canonical two-line
/// call site stays a single `.subcommand(brontes::command(None))` token.
/// Validation — empty group name, sibling collision with a user-defined
/// subcommand, missing `mcp` mount — happens at [`handle`] time, when
/// the assembled parent CLI tree is in scope and a clean
/// [`crate::Error::Config`] can surface at dispatch.
///
/// An empty `cfg.command_name` defaults back to `"mcp"`; an explicit empty
/// string never reaches `clap::Command::new` from this constructor.
///
/// # Example
///
/// ```rust
/// use clap::Command;
///
/// let cli = Command::new("my-cli")
///     .version("0.1.0")
///     .subcommand(brontes::command(None));
/// assert!(cli.find_subcommand("mcp").is_some());
/// ```
#[must_use]
pub fn command(cfg: Option<&Config>) -> Command {
    let name = cfg
        .and_then(|c| c.command_name.as_deref())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_COMMAND_NAME);
    crate::subcommands::build(name)
}

/// Dispatch an `mcp` subcommand match.
///
/// `matches` is the [`ArgMatches`](clap::ArgMatches) for the `mcp` group
/// (typically obtained via `matches.subcommand()` on the root match).
/// `cli` is the full user CLI (cloned by the caller — clap's
/// `get_matches(self)` consumes the original). `cfg` is the optional
/// brontes configuration.
///
/// Validates that the configured group name is in fact the brontes-minted
/// subtree (sibling collision detection) before invoking the matched leaf.
///
/// # Errors
///
/// - [`crate::Error::Config`] when the configured `command_name` resolves to
///   a sibling subcommand that brontes did not mint (e.g., the user
///   pre-registered a `mcp` subcommand with the same name and forgot to
///   rename ours via [`Config::command_name`]).
/// - Any error returned by the dispatched leaf (`start`, `tools`, or
///   `stream`).
pub async fn handle(matches: &clap::ArgMatches, cli: &Command, cfg: Option<&Config>) -> Result<()> {
    let cfg_owned = cfg.map_or_else(Config::default, Config::clone);
    // An explicit empty `command_name` would have silently re-used "mcp"
    // inside `command()`. Surface it here as a clean config error so a
    // typo in the consumer's config doesn't produce confusing diagnostics
    // later.
    if matches!(cfg.and_then(|c| c.command_name.as_deref()), Some("")) {
        return Err(crate::Error::Config(
            "Config.command_name must not be empty".into(),
        ));
    }
    let group_name = cfg
        .and_then(|c| c.command_name.as_deref())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_COMMAND_NAME);

    // Sibling-collision check: the sibling in the user's CLI named
    // `group_name` must carry our hidden marker. If it does not, the user
    // already had a same-named subcommand and brontes silently lost the
    // mount race — surface that as a clean error.
    let group = cli.find_subcommand(group_name).ok_or_else(|| {
        crate::Error::Config(format!(
            "no subcommand named {group_name:?} found on the CLI; \
             did you forget to mount brontes::command(...)?"
        ))
    })?;
    let has_marker = group
        .get_subcommands()
        .any(|s| s.get_name() == crate::subcommands::MARKER_NAME);
    if !has_marker {
        return Err(crate::Error::Config(format!(
            "subcommand {group_name:?} on the CLI was not minted by brontes \
             (sibling collision); rename via Config::command_name"
        )));
    }

    match matches.subcommand() {
        Some(("start", sub)) => {
            crate::subcommands::start::run(sub, cli.clone(), Some(cfg_owned)).await
        }
        Some(("tools", sub)) => crate::subcommands::tools::run(sub, cli, Some(cfg_owned)),
        Some(("stream", sub)) => {
            crate::subcommands::stream::run(sub, cli.clone(), Some(cfg_owned)).await
        }
        Some(("claude", sub)) => crate::subcommands::editor::claude::run(sub, Some(&cfg_owned)),
        Some(("vscode", sub)) => crate::subcommands::editor::vscode::run(sub, Some(&cfg_owned)),
        Some(("cursor", sub)) => crate::subcommands::editor::cursor::run(sub, Some(&cfg_owned)),
        Some(("zed", sub)) => crate::subcommands::editor::zed::run(sub, Some(&cfg_owned)),
        // Guard the internal marker subcommand: it parses cleanly through the
        // clap surface (because it is registered as a hidden subcommand), but
        // it is implementation detail and is not runnable. Surface a friendly
        // error that does not leak the literal marker name.
        Some((other, _)) if other == crate::subcommands::MARKER_NAME => Err(crate::Error::Config(
            "internal marker subcommand is not a runnable command".into(),
        )),
        Some((other, _)) => Err(crate::Error::Config(format!(
            "unknown mcp subcommand: {other:?}"
        ))),
        None => Err(crate::Error::Config(
            "no mcp subcommand selected; pass --help to see options".into(),
        )),
    }
}

/// One-call sugar: mount the `mcp` subtree, parse `argv`, dispatch.
///
/// Equivalent to writing the two-line ceremony from the crate-level
/// example by hand. Returns an [`crate::Error::Config`] when invoked with a
/// non-mcp subcommand or with no subcommand at all — `run()` is intended
/// for tiny CLIs that have no business logic of their own beyond the
/// MCP subtree.
///
/// # Errors
///
/// - [`crate::Error::Config`] from [`command`] for bad configuration.
/// - [`crate::Error::Config`] if argv selects a non-mcp subcommand.
/// - Any error returned by [`handle`].
///
/// # Example
///
/// ```no_run
/// use clap::Command;
///
/// #[tokio::main]
/// async fn main() -> brontes::Result<()> {
///     brontes::run(Command::new("my-cli").version("0.1.0"), None).await
/// }
/// ```
pub async fn run(cli: Command, cfg: Option<&Config>) -> Result<()> {
    // `std::env::ArgsOs` is `!Send`, so eagerly collect into a Vec<OsString>
    // (which is `Send`) before await-ing `run_from` — otherwise the
    // returned future inherits the !Send bound from the iterator and
    // breaks downstream callers that want a Send future.
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    run_from(cli, cfg, argv).await
}

/// Same as [`run`] but reads argv from an explicit iterator instead of
/// the process environment.
///
/// Production code uses [`run`] (which reads `std::env::args_os()`); the
/// integration test crate uses this to drive `run`'s argv-parsing /
/// dispatch logic with synthetic input — `get_matches` consumes the
/// process argv unconditionally, so the only way to exercise `run` in
/// a test without exec'ing a subprocess is to inject the args directly.
///
/// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
/// `lib.rs` can carry it out; effective visibility is crate-internal.
///
/// # Errors
///
/// Same as [`run`].
pub async fn run_from<I, T>(cli: Command, cfg: Option<&Config>, argv: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let mounted = cli.subcommand(command(cfg));
    let cli_for_dispatch = mounted.clone();
    let matches = mounted.get_matches_from(argv);
    match matches.subcommand() {
        Some((name, sub)) => {
            let group_name = cfg
                .and_then(|c| c.command_name.as_deref())
                .unwrap_or(DEFAULT_COMMAND_NAME);
            if name == group_name {
                handle(sub, &cli_for_dispatch, cfg).await
            } else {
                Err(crate::Error::Config(format!(
                    "brontes::run only dispatches the {group_name:?} subtree; \
                     got subcommand {name:?}. Mount brontes::command() on a \
                     hand-built CLI for multi-subcommand apps."
                )))
            }
        }
        None => Err(crate::Error::Config(format!(
            "no subcommand provided; expected the {:?} subtree",
            cfg.and_then(|c| c.command_name.as_deref())
                .unwrap_or(DEFAULT_COMMAND_NAME)
        ))),
    }
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
        // Canonical case: nested command path with an explicit prefix override.
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

    #[test]
    fn build_tool_name_collapses_spaces_in_prefix() {
        // A prefix with internal spaces gets the same "spaces → underscores"
        // treatment as the rest of the name. This is the deliberate uniform
        // post-process; consumers passing "my prefix" get "my_prefix_sub".
        assert_eq!(build_tool_name("ignored sub", "my prefix"), "my_prefix_sub");
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

    // ── descriptions / description_modes validation ───────────────────────────

    #[test]
    fn validate_paths_rejects_unknown_description_path() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().description("nonexistent path", "some text");
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("descriptions")),
            "expected Config error for unknown description path, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_empty_description_text() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        // Path exists, but description text is empty — must be rejected so
        // MCP clients never see a tool with an empty description.
        let cfg = Config::default().description("myapp list", "");
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("is empty")),
            "expected Config error for empty description, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_whitespace_only_description_text() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        // Whitespace-only text trims to empty — same rejection.
        let cfg = Config::default().description("myapp list", "   \n\t ");
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("is empty")),
            "expected Config error for whitespace-only description, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_unknown_description_mode_path() {
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default()
            .description_mode_for("nonexistent path", crate::config::DescriptionMode::Short);
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("description_modes")),
            "expected Config error for unknown description_mode path, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_unknown_deprecated_path() {
        // A `.deprecate("path")` on a path that doesn't exist must surface
        // as Error::Config at validate time — silently dropping a deprecate
        // would leave the command visible to MCP clients, which is the
        // exact bug the validator is supposed to catch.
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().deprecate("myapp nonexistent");
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("deprecated_commands")),
            "expected Config error for unknown deprecate path, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_unknown_flag_type_override_path() {
        // `.flag_type_override(path, flag, kind)` runs through the same
        // validate_flag_path helper as flag_schemas; the unknown-path arm
        // is distinct from the unknown-flag arm and must be exercised
        // independently.
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().flag_type_override(
            "myapp nonexistent",
            "limit",
            crate::schema::SchemaType::Array,
        );
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("flag_type_overrides")),
            "expected Config error for unknown flag_type_override path, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_unknown_flag_on_known_path_for_flag_type_override() {
        // The known-path / unknown-flag branch of validate_flag_path,
        // routed through flag_type_overrides specifically (the
        // flag_schemas variant is already covered).
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().flag_type_override(
            "myapp list",
            "nonexistent-flag",
            crate::schema::SchemaType::Array,
        );
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("flag_type_overrides") && msg.contains("nonexistent-flag")),
            "expected unknown-flag Config error mentioning flag_type_overrides, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_rejects_selector_referencing_unknown_path() {
        // A factory-built selector like `allow_cmds(["unknown"])` carries
        // its captured paths into the selector machinery; validate_paths
        // introspects via crate::selectors::lookup and rejects when any
        // captured path is not in the walked tree.
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().selector(crate::Selector {
            cmd: Some(crate::selectors::allow_cmds(["myapp nonexistent"])),
            ..Default::default()
        });
        let result = validate_paths(&resolved, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("Selector references unknown command path")),
            "expected Config error for selector referencing unknown path, got {result:?}"
        );
    }

    #[test]
    fn validate_paths_warns_but_accepts_substring_selector_with_no_match() {
        // Substring selectors are permissive (the warn-but-allow arm) —
        // an `allow_cmds_containing(["zzzzzz"])` against a tree that has
        // no path containing "zzzzzz" must NOT error; it just emits a
        // `tracing::warn!`. We can't assert the warn without a subscriber
        // capture here, but we CAN assert validate_paths returns Ok so
        // the warn-vs-error policy stays pinned.
        let root = root_with_list();
        let resolved = crate::walk::walk(&root);
        let cfg = Config::default().selector(crate::Selector {
            cmd: Some(crate::selectors::allow_cmds_containing(["zzzzzz"])),
            ..Default::default()
        });
        assert!(
            validate_paths(&resolved, &cfg).is_ok(),
            "substring selector with no match must warn, not error"
        );
    }

    #[test]
    fn description_with_empty_cmd_path_validation_error() {
        // `Config::description("", "x")` on a tree where "" doesn't bind to
        // any command must surface as Error::Config from generate_tools'
        // path-validation pass.
        let root = root_with_list();
        let cfg = Config::default().description("", "x");
        let result = generate_tools(&root, &cfg);
        assert!(
            matches!(&result, Err(crate::Error::Config(msg)) if msg.contains("descriptions")),
            "expected Config error for empty command-path description, got {result:?}"
        );
    }

    // ── global flag validation ────────────────────────────────────────────────

    #[test]
    fn validate_paths_accepts_global_flag_at_child_path() {
        // A global flag declared on the root is reachable at child paths
        // via clap's global propagation. Annotating it at the child path
        // must NOT trigger an unknown-flag error.
        use clap::ArgAction;
        let root = Command::new("myapp")
            .arg(
                Arg::new("verbose")
                    .long("verbose")
                    .global(true)
                    .action(ArgAction::SetTrue),
            )
            .subcommand(Command::new("sub"));
        let cfg = Config::default().flag_schema(
            "myapp sub",
            "verbose",
            serde_json::json!({"type": "boolean", "description": "override"}),
        );

        // This must succeed — a Config::Error::Config result would mean
        // we're rejecting a valid annotation.
        let tools = generate_tools(&root, &cfg).expect("global flag at child path must validate");
        assert!(!tools.is_empty(), "tree should produce at least one tool");
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
        assert_eq!(
            tools.iter().filter(|t| t.name.contains("status")).count(),
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

    // ── marker-name guard ─────────────────────────────────────────────────────

    /// `mcp __brontes_internal_marker` parses through clap (the subcommand is
    /// hidden but still registered). `handle()` must intercept it and return a
    /// friendly error that does NOT leak the literal marker name.
    #[tokio::test]
    async fn handle_rejects_marker_subcommand_invocation() {
        let cli = Command::new("myapp")
            .version("0.0.1")
            .subcommand(command(None));
        // Parse `myapp mcp <marker>` directly.
        let matches = cli
            .clone()
            .try_get_matches_from(["myapp", "mcp", crate::subcommands::MARKER_NAME])
            .expect("clap parses the hidden marker subcommand");
        let mcp_matches = matches
            .subcommand_matches("mcp")
            .expect("mcp subcommand selected");
        let err = handle(mcp_matches, &cli, None)
            .await
            .expect_err("invoking the marker must surface an error");
        let msg = err.to_string();
        assert!(
            matches!(err, crate::Error::Config(_)),
            "expected Config error, got {err:?}"
        );
        assert!(
            !msg.contains(crate::subcommands::MARKER_NAME),
            "error message must not leak the marker name; got {msg:?}"
        );
        assert!(
            msg.contains("internal marker subcommand is not a runnable command"),
            "error must use the friendly message; got {msg:?}"
        );
    }
}
