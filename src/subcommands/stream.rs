//! `mcp stream` — streamable HTTP MCP server (clap surface only).
//!
//! Task #1 ships the clap surface so the consumer CLI tree is shape-complete;
//! Task #3 wires the rmcp streamable-HTTP transport into [`run`]. The current
//! body returns [`crate::Error::Config`] so consumers calling it accidentally
//! get a clean message rather than a silent fallthrough.

use clap::{Arg, ArgMatches, Command, value_parser};

use crate::Result;
use crate::config::Config;

/// Build the `mcp stream` clap subcommand.
///
/// Flag surface is fixed at this layer because the CLI shape must be stable
/// for editor-config writers (Task #4) regardless of when the transport is
/// wired in.
pub(crate) fn build() -> Command {
    Command::new("stream")
        .about("Start the MCP server over streamable HTTP")
        .long_about(
            "Start HTTP server to expose CLI commands to AI assistants \
             (streamable transport)",
        )
        .arg(
            Arg::new("host")
                .long("host")
                .value_name("HOST")
                .default_value("")
                .help("Host to bind (empty → 0.0.0.0)"),
        )
        .arg(
            Arg::new("port")
                .long("port")
                .value_name("PORT")
                .value_parser(value_parser!(u16))
                .default_value("8080")
                .help("TCP port to bind"),
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Log level (trace, debug, info, warn, error)"),
        )
}

/// Placeholder runtime for `mcp stream`.
///
/// Returns [`crate::Error::Config`] with a Task-#3 marker message. Task #3
/// replaces this body with the rmcp streamable-HTTP server wiring; the
/// signature stays sync because the body has no awaits, and Task #3 will
/// flip it to `async fn` when it actually awaits transport setup.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn run(_matches: &ArgMatches, _cli: Command, _cfg: Option<Config>) -> Result<()> {
    Err(crate::Error::Config(
        "mcp stream not yet wired — Task #3".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_subcommand_has_full_flag_surface() {
        let cmd = build();
        let names: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert!(names.contains(&"host"), "missing --host: {names:?}");
        assert!(names.contains(&"port"), "missing --port: {names:?}");
        assert!(
            names.contains(&"log-level"),
            "missing --log-level: {names:?}"
        );
    }

    #[test]
    fn run_returns_task3_marker_error() {
        let cmd = build();
        let matches = cmd.clone().try_get_matches_from(["stream"]).expect("parse");
        let result = run(&matches, cmd, None);
        match result {
            Err(crate::Error::Config(msg)) => assert!(
                msg.contains("Task #3"),
                "expected Task #3 marker in message, got: {msg}"
            ),
            other => panic!("expected Config error, got {other:?}"),
        }
    }
}
