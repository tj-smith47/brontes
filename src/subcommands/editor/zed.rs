//! `mcp zed {enable, disable, list}` clap surface and dispatch.
//!
//! Mirrors the Cursor / `VSCode` shape verbatim with two structural
//! differences:
//!
//! - The on-disk config carries unrelated editor settings
//!   (`theme`, `font_family`, keymap, etc.) alongside `context_servers`.
//!   The [`crate::manager::zed::ZedConfig`] struct captures every other
//!   top-level key in a flattened pass-through map so `mcp zed enable` /
//!   `disable` never disturbs them on save.
//! - The file is JSONC. The
//!   [`crate::manager::EditorConfig::preprocess`] hook strips line/block
//!   comments and trailing commas before `serde_json` sees the bytes.
//!
//! Path resolution defers to [`crate::manager::paths::zed_config_path`]
//! (user mode) or [`crate::manager::paths::zed_workspace_path`] (when
//! `--workspace` is set); `--config-path` overrides both.

use clap::{Arg, ArgAction, ArgMatches, Command};

use crate::Result;
use crate::config::Config;
use crate::manager::Manager;
use crate::manager::zed::{ZedConfig, ZedServer};

use super::{arg_config_path, arg_env, arg_log_level, arg_server_name, merge_env};

/// Build the `--workspace` clap argument used by all three Zed leaves.
///
/// Flag-only (no value). When present, the config path resolves to
/// `$CWD/.zed/settings.json` instead of the per-OS user-mode default.
/// `--config-path` always wins over both.
fn arg_workspace() -> Arg {
    Arg::new("workspace")
        .long("workspace")
        .help("Use the workspace config ($CWD/.zed/settings.json) instead of the user config")
        .action(ArgAction::SetTrue)
}

/// Build the `zed` subcommand (parent of `enable` / `disable` / `list`).
pub fn build() -> Command {
    Command::new("zed")
        .about("Manage Zed MCP servers")
        .long_about("Manage MCP context_servers configuration for Zed")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("enable")
                .about("Add this CLI as an MCP server in Zed")
                .arg(arg_config_path())
                .arg(arg_server_name())
                .arg(arg_env())
                .arg(arg_log_level())
                .arg(arg_workspace()),
        )
        .subcommand(
            Command::new("disable")
                .about("Remove this CLI from Zed's MCP context_servers")
                .arg(arg_config_path())
                .arg(arg_server_name())
                .arg(arg_workspace()),
        )
        .subcommand(
            Command::new("list")
                .about("List MCP servers configured in Zed")
                .arg(arg_config_path())
                .arg(arg_workspace()),
        )
}

/// Dispatch a parsed `zed` match to the right leaf.
///
/// # Errors
///
/// - [`crate::Error::Config`] when the `--env` flag is malformed or when
///   the leaf is unknown / absent.
/// - [`crate::Error::Io`] when [`std::env::current_exe`] fails.
/// - [`crate::Error::EditorConfigRead`] / `Json` / `Backup` / `Write`
///   when the underlying [`Manager`] hits a filesystem error.
pub fn run(matches: &ArgMatches, cfg: Option<&Config>) -> Result<()> {
    match matches.subcommand() {
        Some(("enable", sub)) => run_enable(sub, cfg),
        Some(("disable", sub)) => run_disable(sub),
        Some(("list", sub)) => run_list(sub),
        Some((other, _)) => Err(crate::Error::Config(format!(
            "unknown mcp zed subcommand: {other:?}"
        ))),
        None => Err(crate::Error::Config(
            "no mcp zed subcommand selected; pass --help to see options".into(),
        )),
    }
}

fn run_enable(matches: &ArgMatches, cfg: Option<&Config>) -> Result<()> {
    let path = resolve_config_path(matches);

    let user_pairs: Vec<String> = matches
        .get_many::<String>("env")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();
    let default_env = cfg.map(|c| c.default_env.clone()).unwrap_or_default();
    let env = merge_env(&default_env, &user_pairs)?;

    let exe = crate::exec::current_executable()?;
    let server_name = resolve_server_name(matches, &exe);

    let mut args: Vec<String> = vec!["mcp".to_string(), "start".to_string()];
    if let Some(level) = matches.get_one::<String>("log-level") {
        args.push("--log-level".to_string());
        args.push(level.clone());
    }

    let server = ZedServer {
        command: exe.to_string_lossy().into_owned(),
        args: Some(args),
        env,
        url: None,
        headers: None,
    };

    let mut manager: Manager<ZedConfig> = Manager::load(path)?;
    manager.enable_server(&server_name, server)
}

fn run_disable(matches: &ArgMatches) -> Result<()> {
    let path = resolve_config_path(matches);
    let exe = crate::exec::current_executable()?;
    let server_name = resolve_server_name(matches, &exe);
    let mut manager: Manager<ZedConfig> = Manager::load(path)?;
    manager.disable_server(&server_name)
}

fn run_list(matches: &ArgMatches) -> Result<()> {
    let path = resolve_config_path(matches);
    let manager: Manager<ZedConfig> = Manager::load(path)?;
    manager.print();
    Ok(())
}

/// Resolve the config path with the three-way precedence:
///
/// 1. `--config-path <PATH>` if set.
/// 2. `--workspace` flag → [`crate::manager::paths::zed_workspace_path`].
/// 3. Per-OS user-mode default from
///    [`crate::manager::paths::zed_config_path`].
fn resolve_config_path(matches: &ArgMatches) -> std::path::PathBuf {
    if let Some(p) = matches.get_one::<String>("config-path") {
        return std::path::PathBuf::from(p);
    }
    if matches.get_flag("workspace") {
        return crate::manager::paths::zed_workspace_path();
    }
    crate::manager::paths::zed_config_path()
}

/// Resolve the server name: `--server-name` override if present, otherwise
/// the executable-derived name from
/// [`crate::manager::paths::derive_server_name`].
fn resolve_server_name(matches: &ArgMatches, exe: &std::path::Path) -> String {
    matches
        .get_one::<String>("server-name")
        .cloned()
        .unwrap_or_else(|| crate::manager::paths::derive_server_name(exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_exposes_three_leaves() {
        let cmd = build();
        let names: Vec<&str> = cmd.get_subcommands().map(Command::get_name).collect();
        assert!(names.contains(&"enable"), "got {names:?}");
        assert!(names.contains(&"disable"), "got {names:?}");
        assert!(names.contains(&"list"), "got {names:?}");
    }

    #[test]
    fn enable_has_expected_flags_including_workspace() {
        let cmd = build();
        let enable = cmd.find_subcommand("enable").expect("enable");
        let flags: Vec<&str> = enable
            .get_arguments()
            .map(|a| a.get_id().as_str())
            .collect();
        for needed in &[
            "config-path",
            "server-name",
            "env",
            "log-level",
            "workspace",
        ] {
            assert!(flags.contains(needed), "missing {needed:?}, have {flags:?}");
        }
    }

    #[test]
    fn disable_has_workspace_flag_and_no_env() {
        // --workspace is accepted on disable; --env is enable-only.
        let cmd = build();
        let disable = cmd.find_subcommand("disable").expect("disable");
        let flags: Vec<&str> = disable
            .get_arguments()
            .map(|a| a.get_id().as_str())
            .collect();
        assert!(flags.contains(&"workspace"), "got {flags:?}");
        assert!(!flags.contains(&"env"), "disable must not carry --env");
        assert!(
            !flags.contains(&"log-level"),
            "disable must not carry --log-level"
        );
    }

    #[test]
    fn list_has_workspace_flag_and_no_server_name() {
        // list takes --config-path + --workspace only; no --server-name.
        let cmd = build();
        let list = cmd.find_subcommand("list").expect("list");
        let flags: Vec<&str> = list.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert!(flags.contains(&"workspace"), "got {flags:?}");
        assert!(flags.contains(&"config-path"), "got {flags:?}");
        assert!(!flags.contains(&"server-name"));
    }

    #[test]
    fn env_flag_is_repeatable_on_enable() {
        // Parser-level: -e KEY=VAL must accept multiple occurrences.
        let cmd = build();
        let parsed = cmd
            .try_get_matches_from([
                "zed",
                "enable",
                "--config-path",
                "/tmp/x.json",
                "-e",
                "A=1",
                "-e",
                "B=2",
            ])
            .expect("parses");
        let enable = parsed.subcommand_matches("enable").expect("enable");
        let vals: Vec<String> = enable
            .get_many::<String>("env")
            .expect("env")
            .cloned()
            .collect();
        assert_eq!(vals, vec!["A=1", "B=2"]);
    }
}
