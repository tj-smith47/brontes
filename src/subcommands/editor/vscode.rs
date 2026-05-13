//! `mcp vscode {enable, disable, list}` clap surface and dispatch.
//!
//! Mirrors ophis `internal/cfgmgr/cmd/vscode/{root,enable,disable,list}.go`
//! with the following divergences:
//!
//! - No emoji prefix on the existing-server / disable-missing warnings.
//! - JSON server map is a [`BTreeMap`] for byte-stable on-disk output.
//! - `--workspace` is accepted on ALL THREE leaves (enable AND disable AND
//!   list).
//!
//! Path resolution defers to [`crate::manager::paths::vscode_config_path`]
//! (user mode) or [`crate::manager::paths::vscode_workspace_path`]
//! (when `--workspace` is set), and `--config-path` overrides both. The
//! captured executable path is the cached [`crate::exec::current_executable`]
//! value so the same binary path that tool calls spawn is the one written
//! into `VSCode`'s config.
//!
//! # Group long-about asymmetry
//!
//! The `vscode` group's `long_about` uses the FULL form
//! `"Visual Studio Code"`; every `vscode` leaf's `long_about` uses the
//! abbreviation `"VSCode"`. This asymmetry is pinned verbatim from ophis;
//! the inline test `group_long_uses_full_visual_studio_code_form` locks
//! the surface against future "normalization" drift.

use clap::{Arg, ArgAction, ArgMatches, Command};

use crate::Result;
use crate::config::Config;
use crate::manager::Manager;
use crate::manager::vscode::{VSCodeConfig, VSCodeServer};

use super::{arg_config_path, arg_env, arg_log_level, arg_server_name, merge_env};

/// Build the `--workspace` clap argument used by all three `VSCode` leaves.
///
/// Flag-only (no value). When present, the editor's config path resolves
/// to `$CWD/.vscode/mcp.json` instead of the per-OS user-mode default.
/// `--config-path` always wins over both.
fn arg_workspace() -> Arg {
    Arg::new("workspace")
        .long("workspace")
        .help("Use the workspace config ($CWD/.vscode/mcp.json) instead of the user config")
        .action(ArgAction::SetTrue)
}

/// Build the `vscode` subcommand (parent of `enable` / `disable` / `list`).
///
/// Registered under the `mcp` group by [`crate::subcommands::build`]; the
/// dispatcher in [`crate::command::handle`] routes the matched leaf into
/// [`run`].
///
/// The group's `long_about` uses the FULL form `"Visual Studio Code"`,
/// matching ophis `cmd/vscode/root.go`. The leaf
/// `long_about` strings use the abbreviation `"VSCode"`.
pub fn build() -> Command {
    Command::new("vscode")
        .about("Manage VSCode MCP servers")
        .long_about("Manage MCP server configuration for Visual Studio Code")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("enable")
                .about("Add server to VSCode config")
                .long_about("Add this application as an MCP server in VSCode")
                .arg(arg_config_path())
                .arg(arg_server_name())
                .arg(arg_env())
                .arg(arg_log_level())
                .arg(arg_workspace()),
        )
        .subcommand(
            Command::new("disable")
                .about("Remove server from VSCode config")
                .long_about("Remove this application from VSCode MCP servers")
                .arg(arg_config_path())
                .arg(arg_server_name())
                .arg(arg_workspace()),
        )
        .subcommand(
            Command::new("list")
                .about("Show VSCode MCP servers")
                .long_about("Show all MCP servers configured in VSCode")
                .arg(arg_config_path())
                .arg(arg_workspace()),
        )
}

/// Dispatch a parsed `vscode` match to the right leaf.
///
/// `matches` is the `ArgMatches` of the `vscode` subcommand itself; the
/// dispatcher inspects the next level (`enable` / `disable` / `list`)
/// internally. `cfg` carries [`Config::default_env`] which is merged with
/// any `--env` flags at enable time.
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
            "unknown mcp vscode subcommand: {other:?}"
        ))),
        None => Err(crate::Error::Config(
            "no mcp vscode subcommand selected; pass --help to see options".into(),
        )),
    }
}

fn run_enable(matches: &ArgMatches, cfg: Option<&Config>) -> Result<()> {
    let path = resolve_config_path(matches);

    // Build the env map: start with default_env, overlay --env KEY=VAL.
    let user_pairs: Vec<String> = matches
        .get_many::<String>("env")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();
    let default_env = cfg.map(|c| c.default_env.clone()).unwrap_or_default();
    let env = merge_env(&default_env, &user_pairs)?;

    // Resolve the executable path once, cached via OnceLock in exec.rs.
    let exe = crate::exec::current_executable()?;
    let server_name = resolve_server_name(matches, &exe);

    // Construct the argv tail that VSCode will spawn:
    // `<exe> mcp start [--log-level LEVEL]`.
    let mut args: Vec<String> = vec!["mcp".to_string(), "start".to_string()];
    if let Some(level) = matches.get_one::<String>("log-level") {
        args.push("--log-level".to_string());
        args.push(level.clone());
    }

    let server = VSCodeServer {
        kind: "stdio".to_string(),
        command: exe.to_string_lossy().into_owned(),
        args: Some(args),
        env,
        url: None,
        headers: None,
    };

    let mut manager: Manager<VSCodeConfig> = Manager::load(path)?;
    manager.enable_server(&server_name, server)
}

fn run_disable(matches: &ArgMatches) -> Result<()> {
    let path = resolve_config_path(matches);
    let exe = crate::exec::current_executable()?;
    let server_name = resolve_server_name(matches, &exe);
    let mut manager: Manager<VSCodeConfig> = Manager::load(path)?;
    manager.disable_server(&server_name)
}

fn run_list(matches: &ArgMatches) -> Result<()> {
    let path = resolve_config_path(matches);
    let manager: Manager<VSCodeConfig> = Manager::load(path)?;
    manager.print();
    Ok(())
}

/// Resolve the config path with the three-way precedence:
///
/// 1. `--config-path <PATH>` if set (overrides everything).
/// 2. `--workspace` flag → [`crate::manager::paths::vscode_workspace_path`]
///    (`$CWD/.vscode/mcp.json`).
/// 3. Per-OS user-mode default from
///    [`crate::manager::paths::vscode_config_path`].
fn resolve_config_path(matches: &ArgMatches) -> std::path::PathBuf {
    if let Some(p) = matches.get_one::<String>("config-path") {
        return std::path::PathBuf::from(p);
    }
    if matches.get_flag("workspace") {
        return crate::manager::paths::vscode_workspace_path();
    }
    crate::manager::paths::vscode_config_path()
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
        assert!(
            names.contains(&"enable"),
            "enable leaf present, got {names:?}"
        );
        assert!(
            names.contains(&"disable"),
            "disable leaf present, got {names:?}"
        );
        assert!(names.contains(&"list"), "list leaf present, got {names:?}");
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
            assert!(
                flags.contains(needed),
                "enable missing flag {needed:?}; have {flags:?}"
            );
        }
    }

    #[test]
    fn disable_has_workspace_flag() {
        // --workspace is accepted on ALL three leaves.
        let cmd = build();
        let disable = cmd.find_subcommand("disable").expect("disable");
        let flags: Vec<&str> = disable
            .get_arguments()
            .map(|a| a.get_id().as_str())
            .collect();
        assert!(flags.contains(&"config-path"));
        assert!(flags.contains(&"server-name"));
        assert!(
            flags.contains(&"workspace"),
            "disable must accept --workspace, got {flags:?}"
        );
        assert!(!flags.contains(&"env"), "disable must not carry --env");
    }

    #[test]
    fn list_has_workspace_flag() {
        // --workspace is accepted on ALL three leaves, including list.
        let cmd = build();
        let list = cmd.find_subcommand("list").expect("list");
        let flags: Vec<&str> = list.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert!(flags.contains(&"config-path"));
        assert!(
            flags.contains(&"workspace"),
            "list must accept --workspace, got {flags:?}"
        );
        assert!(!flags.contains(&"server-name"));
    }

    #[test]
    fn env_flag_is_repeatable() {
        let cmd = build();
        let parsed = cmd
            .try_get_matches_from([
                "vscode",
                "enable",
                "--config-path",
                "/tmp/x.json",
                "--env",
                "A=1",
                "--env",
                "B=2",
            ])
            .expect("parses");
        let enable = parsed.subcommand_matches("enable").expect("enable matches");
        let vals: Vec<String> = enable
            .get_many::<String>("env")
            .expect("env values")
            .cloned()
            .collect();
        assert_eq!(vals, vec!["A=1".to_string(), "B=2".to_string()]);
    }

    #[test]
    fn group_long_uses_full_visual_studio_code_form() {
        // The `vscode` group's `long_about` uses the FULL form
        // "Visual Studio Code"; every `vscode` leaf's `long_about`
        // uses the abbreviation "VSCode". This asymmetry is pinned
        // verbatim — this test locks the surface against future
        // "normalization" drift.
        let cmd = build();

        // Group long_about: full form.
        let group_long = cmd
            .get_long_about()
            .map(ToString::to_string)
            .expect("group long_about");
        assert!(
            group_long.contains("Visual Studio Code"),
            "group long_about must use full form, got {group_long:?}"
        );

        // Each leaf long_about: abbreviation, NOT full form.
        for leaf_name in &["enable", "disable", "list"] {
            let leaf = cmd
                .find_subcommand(leaf_name)
                .unwrap_or_else(|| panic!("{leaf_name} subcommand"));
            let leaf_long = leaf
                .get_long_about()
                .map_or_else(|| panic!("{leaf_name} long_about"), ToString::to_string);
            assert!(
                leaf_long.contains("VSCode"),
                "leaf {leaf_name} long_about must use VSCode abbreviation, got {leaf_long:?}"
            );
            assert!(
                !leaf_long.contains("Visual Studio Code"),
                "leaf {leaf_name} long_about must NOT use full form, got {leaf_long:?}"
            );
        }
    }
}
