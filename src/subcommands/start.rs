//! `mcp start` — boot the MCP server over stdio.
//!
//! Mirrors ophis `start.go`. The flag surface is exactly `--log-level
//! <LEVEL>` and inherits the group's help layout.

use clap::{Arg, ArgMatches, Command};
use tracing::Level;

use crate::Result;
use crate::config::Config;

/// Build the `mcp start` clap subcommand.
pub fn build() -> Command {
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
pub async fn run(matches: &ArgMatches, cli: Command, cfg: Option<Config>) -> Result<()> {
    let log_level = parse_log_level(matches);
    crate::server::stdio::serve_stdio(cli, cfg, log_level).await
}

/// Test-only proxy for [`parse_log_level`]. Exposed via
/// [`crate::__test_internal::parse_start_log_level`] so the warn-fire
/// test crate can assert the §11 #9 unrecognized-`--log-level`
/// `tracing::warn!` fires without driving the full `serve_stdio` runtime.
pub fn parse_log_level_for_test(matches: &ArgMatches) -> Option<Level> {
    parse_log_level(matches)
}

/// Test-only proxy for [`build`]. Exposed via
/// [`crate::__test_internal::start_subcommand`].
pub fn build_for_test() -> Command {
    build()
}

fn parse_log_level(matches: &ArgMatches) -> Option<Level> {
    super::common::parse_log_level(matches)
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
