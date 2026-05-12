//! `mcp start` — boot the MCP server over stdio.
//!
//! Mirrors ophis `start.go`. The flag surface is exactly `--log-level
//! <LEVEL>` and inherits the group's help layout.

use clap::{Arg, ArgMatches, Command};
use tracing::Level;

use crate::Result;
use crate::config::Config;

/// Build the `mcp start` clap subcommand.
pub(crate) fn build() -> Command {
    Command::new("start")
        .about("Start the MCP server")
        .long_about("Start stdio server to expose CLI commands to AI assistants")
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Log level (trace, debug, info, warn, error)"),
        )
}

/// Run `mcp start` against the supplied CLI tree.
///
/// `matches` is the `ArgMatches` for the `start` subcommand; `cli` is the
/// full user CLI (cloned by the caller); `cfg` is the optional user
/// configuration.
pub(crate) async fn run(matches: &ArgMatches, cli: Command, cfg: Option<Config>) -> Result<()> {
    let log_level = parse_log_level(matches);
    crate::server::stdio::serve_stdio(cli, cfg, log_level).await
}

/// Parse the `--log-level` flag into a [`Level`] when present.
///
/// Invalid values return `None` (i.e., fall through to `Config::log_level`
/// or `RUST_LOG`); a `tracing::warn!` records the offending value so users
/// notice the typo at startup rather than wondering why their level had
/// no effect.
fn parse_log_level(matches: &ArgMatches) -> Option<Level> {
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

    #[test]
    fn start_subcommand_has_log_level_flag() {
        let cmd = build();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "log-level")
            .expect("--log-level flag must be present");
        assert_eq!(arg.get_long(), Some("log-level"));
    }

    #[test]
    fn parse_log_level_recognises_common_values() {
        for (raw, expected) in [
            ("trace", Level::TRACE),
            ("debug", Level::DEBUG),
            ("info", Level::INFO),
            ("warn", Level::WARN),
            ("warning", Level::WARN),
            ("error", Level::ERROR),
            ("INFO", Level::INFO),
        ] {
            let matches = Command::new("start")
                .arg(Arg::new("log-level").long("log-level"))
                .try_get_matches_from(["start", "--log-level", raw])
                .expect("parses");
            assert_eq!(parse_log_level(&matches), Some(expected), "raw={raw}");
        }
    }

    #[test]
    fn parse_log_level_unknown_returns_none() {
        let matches = Command::new("start")
            .arg(Arg::new("log-level").long("log-level"))
            .try_get_matches_from(["start", "--log-level", "verbose"])
            .expect("parses");
        assert!(parse_log_level(&matches).is_none());
    }

    #[test]
    fn parse_log_level_absent_returns_none() {
        let matches = Command::new("start")
            .arg(Arg::new("log-level").long("log-level"))
            .try_get_matches_from(["start"])
            .expect("parses");
        assert!(parse_log_level(&matches).is_none());
    }
}
