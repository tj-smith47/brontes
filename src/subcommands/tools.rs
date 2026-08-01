//! `mcp tools` — export the generated MCP tool list as JSON.
//!
//! Ports ophis `tools.go`. Writes `./mcp-tools.json` (cwd-relative, truncating
//! create) with 2-space pretty-printed JSON, then prints a one-line summary
//! to stdout. Honors the `--log-level` flag for stderr logging consistency
//! with `mcp start` / `mcp stream`, even though the command itself does not
//! run a server.

use std::io::Write;
use std::path::Path;

use clap::{Arg, ArgMatches, Command};
use tempfile::NamedTempFile;
use tracing::Level;

use crate::Result;
use crate::config::Config;

/// Filename written by `mcp tools`, matching ophis (`tools.go:30`).
const OUTPUT_FILE: &str = "mcp-tools.json";

/// Build the `mcp tools` clap subcommand.
pub fn build() -> Command {
    super::common::with_selection_flags(
        Command::new("tools")
            .about("Export tools as JSON")
            .long_about("Export available MCP tools to mcp-tools.json for inspection")
            .arg(
                Arg::new("log-level")
                    .long("log-level")
                    .value_name("LEVEL")
                    .help("Log level (trace, debug, info, warn, error)"),
            )
            .arg(
                Arg::new("groups")
                    .long("groups")
                    .action(clap::ArgAction::SetTrue)
                    // `--groups` describes the CLI rather than one server's
                    // subset, so it ignores a selection. Conflicting rather
                    // than ignoring silently keeps `--groups --group x` from
                    // looking like it filtered the listing.
                    .conflicts_with_all([
                        "group",
                        "command",
                        "tool",
                        "hide-group",
                        "hide-command",
                        "hide-tool",
                        super::common::ALL_FLAG,
                    ])
                    .help(
                        "List the command groups this CLI defines, then exit \
                         (cannot be combined with the selection flags)",
                    ),
            ),
    )
}

/// Run `mcp tools` against the supplied CLI tree.
///
/// Resolves the tool list via [`crate::generate_tools`], serializes it as
/// pretty JSON, and writes it to `./mcp-tools.json` in the current working
/// directory.
///
/// # Errors
///
/// - [`crate::Error::Config`] / [`crate::Error::Schema`] surfaced through
///   `generate_tools` for invalid configuration.
/// - [`crate::Error::Io`] if the output file cannot be written.
pub fn run(matches: &ArgMatches, cli: &Command, cfg: Option<Config>) -> Result<()> {
    let cfg =
        super::common::apply_selection_flags(cfg.unwrap_or_default(), cli.get_name(), matches);
    init_tracing(super::common::parse_log_level(matches).or(cfg.log_level));

    if matches.get_flag("groups") {
        return list_groups(cli, &cfg);
    }

    let tools = crate::generate_tools(cli, &cfg)?;

    let json = serde_json::to_string_pretty(&tools)
        .map_err(|e| crate::Error::Schema(format!("failed to serialize tool list: {e}")))?;

    let path = Path::new(OUTPUT_FILE);
    write_atomic(path, json.as_bytes())?;

    println!(
        "Successfully exported {} tools to {OUTPUT_FILE}",
        tools.len()
    );
    Ok(())
}

/// Print the groups this CLI defines, with how many tools each one carries.
///
/// The count is resolved against the walked tree rather than against the
/// group's member list, so a group naming a parent command reports the number
/// of tools an end user would actually get from `--group <NAME>`.
///
/// # Errors
///
/// Same conditions as [`crate::generate_tools`] — the walk still runs, because
/// a group whose members no longer exist should be reported as the
/// configuration error it is rather than listed with a count of zero.
fn list_groups(cli: &Command, cfg: &Config) -> Result<()> {
    // Counted against the untrimmed list. `--groups` answers "what does this
    // CLI define", which does not change because one server was started with
    // a subset — and a `Config`-pinned filter would otherwise make the listing
    // report zeroes, or refuse to run at all.
    let cfg = crate::command::normalize_command_paths(cfg, cli.get_name());
    let mut unfiltered = cfg.clone().into_owned();
    unfiltered.tool_filter = crate::ToolFilter::default();
    let tools = crate::command::generate_tools_with_middleware(cli, &unfiltered)?;

    if cfg.groups.is_empty() {
        println!("This CLI defines no command groups.");
        return Ok(());
    }

    let rows: Vec<(&String, String, &str)> = cfg
        .groups
        .iter()
        .map(|(name, group)| {
            let count = tools
                .iter()
                .filter(|t| {
                    group
                        .commands
                        .iter()
                        .any(|m| crate::toolset::covers(m, &t.command_path))
                })
                .count();
            let plural = if count == 1 { "tool" } else { "tools" };
            (
                name,
                format!("{count} {plural}"),
                group.description.as_deref().unwrap_or(""),
            )
        })
        .collect();

    // Column widths in characters, not bytes: `{:<width$}` pads by character
    // count, so a byte length would over-pad any group name outside ASCII.
    let name_width = rows
        .iter()
        .map(|(n, ..)| n.chars().count())
        .max()
        .unwrap_or(0);
    let count_width = rows
        .iter()
        .map(|(_, c, _)| c.chars().count())
        .max()
        .unwrap_or(0);
    for (name, count, about) in rows {
        println!("{name:<name_width$}  {count:<count_width$}  {about}");
    }
    Ok(())
}

/// Atomically write `bytes` (plus a trailing newline) to `path`.
///
/// Creates a [`NamedTempFile`] in `path`'s parent directory (`"."` when the
/// path is bare), writes the payload, syncs, then [`NamedTempFile::persist`]
/// renames into place. The rename is atomic on POSIX filesystems, so a crash
/// mid-write cannot leave a half-written `mcp-tools.json` masquerading as a
/// valid export.
///
/// Logs at `info` when `path` already exists (overwrite case).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = parent.unwrap_or_else(|| Path::new("."));

    if path.exists() {
        tracing::info!(
            target: "brontes::tools",
            path = %path.display(),
            "overwriting existing file"
        );
    }

    let mut tmp = NamedTempFile::new_in(dir).map_err(|e| crate::Error::Io {
        context: format!("create temp file in {}", dir.display()),
        source: e,
    })?;
    tmp.write_all(bytes).map_err(|e| crate::Error::Io {
        context: format!("write temp file for {}", path.display()),
        source: e,
    })?;
    tmp.write_all(b"\n").map_err(|e| crate::Error::Io {
        context: format!("write temp file for {}", path.display()),
        source: e,
    })?;
    tmp.as_file_mut().sync_all().map_err(|e| crate::Error::Io {
        context: format!("fsync temp file for {}", path.display()),
        source: e,
    })?;
    tmp.persist(path).map_err(|e| crate::Error::Io {
        context: format!("persist temp file to {}", path.display()),
        source: e.error,
    })?;
    Ok(())
}

fn init_tracing(level: Option<Level>) {
    use tracing_subscriber::EnvFilter;
    let filter = level.map_or_else(
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        |lvl| EnvFilter::new(lvl.to_string()),
    );
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_subcommand_has_log_level_flag() {
        let cmd = build();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "log-level")
            .expect("--log-level flag must be present");
        assert_eq!(arg.get_long(), Some("log-level"));
    }

    #[test]
    fn output_file_is_cwd_relative() {
        // Pin the constant — a refactor that introduces an absolute path
        // or changes the filename would break parity with ophis.
        assert_eq!(OUTPUT_FILE, "mcp-tools.json");
    }

    #[test]
    fn write_atomic_first_write_creates_file_with_trailing_newline() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("out.json");
        write_atomic(&path, b"hello").expect("first write");
        let got = std::fs::read(&path).expect("read back");
        assert_eq!(got, b"hello\n");
    }

    #[test]
    fn write_atomic_overwrite_replaces_existing_content() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("out.json");
        std::fs::write(&path, b"stale\n").expect("seed");
        write_atomic(&path, b"fresh").expect("overwrite");
        let got = std::fs::read(&path).expect("read back");
        assert_eq!(got, b"fresh\n");
    }

    #[test]
    fn write_atomic_returns_io_error_when_parent_dir_missing() {
        // NamedTempFile::new_in fails when the parent directory does not
        // exist; the error must surface as `Error::Io` carrying a
        // context that mentions the failed directory — that mapping is
        // the contract `mcp tools` exposes to its caller.
        let path = std::path::PathBuf::from("/this/path/does/not/exist/out.json");
        let err = write_atomic(&path, b"x").expect_err("must fail");
        match err {
            crate::Error::Io { context, .. } => {
                assert!(
                    context.contains("create temp file in")
                        && context.contains("/this/path/does/not/exist"),
                    "context must name the failed dir, got: {context}"
                );
            }
            other => panic!("expected Error::Io, got: {other}"),
        }
    }
}
