//! `mcp tools` — export the generated MCP tool list as JSON.
//!
//! Ports ophis `tools.go`. Writes `./mcp-tools.json` (cwd-relative, truncating
//! create) with 2-space pretty-printed JSON, then prints a one-line summary
//! to stdout. Honors the `--log-level` flag for stderr logging consistency
//! with `mcp start` / `mcp stream`, even though the command itself does not
//! run a server.

use std::path::Path;

use clap::{Arg, ArgMatches, Command};
use tracing::Level;

use crate::Result;
use crate::config::Config;

/// Filename written by `mcp tools`, matching ophis (`tools.go:30`).
const OUTPUT_FILE: &str = "mcp-tools.json";

/// Build the `mcp tools` clap subcommand.
pub(crate) fn build() -> Command {
    Command::new("tools")
        .about("Export tools as JSON")
        .long_about("Export available MCP tools to mcp-tools.json for inspection")
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Log level (trace, debug, info, warn, error)"),
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
pub(crate) fn run(matches: &ArgMatches, cli: &Command, cfg: Option<Config>) -> Result<()> {
    let cfg = cfg.unwrap_or_default();
    init_tracing(parse_log_level(matches).or(cfg.log_level));

    let tools = crate::generate_tools(cli, &cfg)?;

    let json = serde_json::to_string_pretty(&tools)
        .map_err(|e| crate::Error::Schema(format!("failed to serialize tool list: {e}")))?;

    let path = Path::new(OUTPUT_FILE);
    std::fs::write(path, format!("{json}\n")).map_err(|e| crate::Error::Io {
        context: format!("write {OUTPUT_FILE}"),
        source: e,
    })?;

    println!(
        "Successfully exported {} tools to {OUTPUT_FILE}",
        tools.len()
    );
    Ok(())
}

/// Same as [`crate::subcommands::start::parse_log_level`] but local to this
/// module for cohesion — keeping the per-subcommand log-level handling
/// inside the subcommand keeps the surface easy to evolve independently.
fn parse_log_level(matches: &ArgMatches) -> Option<Level> {
    let raw = matches.get_one::<String>("log-level")?;
    match raw.to_ascii_lowercase().as_str() {
        "trace" => Some(Level::TRACE),
        "debug" => Some(Level::DEBUG),
        "info" => Some(Level::INFO),
        "warn" | "warning" => Some(Level::WARN),
        "error" => Some(Level::ERROR),
        _ => None,
    }
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
}
