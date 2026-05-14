//! `mcp stream` — streamable HTTP MCP server.
//!
//! Mirrors ophis `start.go::runStreamableHTTPServer` (`start.go:95-126`)
//! and `config.go::serveStreamableHTTP`. The clap surface is `--host
//! <HOST>`, `--port <PORT>`, `--log-level <LEVEL>`; an empty `--host` is
//! mapped to `0.0.0.0` (Go-parity bind-all) inside `run`.
//!
//! The runtime body lives in [`crate::server::http::serve_http`]; this
//! module owns argv translation, signal-listener install, and the
//! startup log line.

use std::net::SocketAddr;

use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use tokio_util::sync::CancellationToken;
use tracing::Level;

use crate::Result;
use crate::config::Config;

/// Build the `mcp stream` clap subcommand.
///
/// Flag surface (`--host`, `--port`, `--log-level`) is stable per
/// ophis-parity; the editor-config writer derives the JSON snippet for
/// MCP clients from this surface.
pub fn build() -> Command {
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
        .arg(
            Arg::new("allow-host")
                .long("allow-host")
                .action(ArgAction::Append)
                .value_name("HOST")
                .help(
                    "Add a hostname to rmcp's DNS-rebind allow-list (repeat for multiple). \
                     Defaults to localhost + 127.0.0.1 + ::1. Specify e.g. \
                     --allow-host myhost.local for LAN access.",
                ),
        )
}

/// Run `mcp stream` against the supplied CLI tree.
///
/// `matches` is the [`ArgMatches`] for the `stream` subcommand; `cli` is
/// the full user CLI (cloned by the caller); `cfg` is the optional user
/// configuration.
///
/// # Errors
///
/// - [`crate::Error::Config`] when `--host`/`--port` produce an invalid
///   `SocketAddr` (this is rare since clap already validates `--port` as
///   `u16`; the host string parse is the remaining failure mode).
/// - Any error surfaced by [`crate::server::http::serve_http`] (bind
///   failure, schema/config error from the pre-walk, transport panic).
pub async fn run(matches: &ArgMatches, cli: Command, cfg: Option<Config>) -> Result<()> {
    // Production entry: mint a fresh cancel token, wire the signal
    // listener into it, then delegate to `run_with_cancel` for the
    // argv-parsing + serve-http body. The split lets the integration
    // test crate drive the body with a pre-cancelled token (which makes
    // `serve_http` return immediately after bind) without firing real
    // SIGINT/SIGTERM at the test process.
    let cancel = CancellationToken::new();
    super::signal::spawn_signal_listener(cancel.clone());
    run_with_cancel(matches, cli, cfg, cancel).await
}

/// Same body as [`run`] without the implicit signal-listener install.
///
/// Production callers use [`run`]; the integration test crate reaches
/// this via [`crate::__test_internal::stream_run_with_cancel`] to drive
/// the post-parse, pre-bind logic with a token it controls. Mirrors how
/// [`crate::server::http::serve_http`] delegates to
/// [`crate::server::http::serve_http_with`].
///
/// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
/// `lib.rs` can carry it out; effective visibility is crate-internal.
///
/// # Errors
///
/// Same as [`run`].
pub async fn run_with_cancel(
    matches: &ArgMatches,
    cli: Command,
    cfg: Option<Config>,
    cancel: CancellationToken,
) -> Result<()> {
    let cfg = cfg.unwrap_or_default();
    let log_level = parse_log_level(matches);
    init_tracing(log_level.or(cfg.log_level));

    let raw_host = matches.get_one::<String>("host").map_or("", String::as_str);
    let port = matches.get_one::<u16>("port").copied().unwrap_or(8080);
    let extra_allowed_hosts: Vec<String> = matches
        .get_many::<String>("allow-host")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();

    let host = resolve_bind_host(raw_host);
    let addr: SocketAddr = format!("{host}:{port}").parse().map_err(|e| {
        crate::Error::Config(format!(
            "invalid --host/--port combination {host:?}:{port}: {e}"
        ))
    })?;

    // Startup log line matches ophis `config.go:124`:
    // `fmt.Sprintf("MCP server listening on address %q", addr)`. The
    // `%q` verb yields a Go-quoted string; we reproduce that with a
    // literal `"{addr}"` (no escaping needed for SocketAddr Display).
    tracing::info!("MCP server listening on address \"{addr}\"");

    crate::server::http::serve_http(cli, cfg, addr, cancel, extra_allowed_hosts).await
}

/// Translate the user-supplied `--host` to the literal address `serve_http`
/// binds. Empty string → bind-all (`"0.0.0.0"`); anything else passes through
/// unchanged.
///
/// Matches ophis `mcp stream`'s `net.Listen("tcp", ":port")` semantics:
/// Go's listener accepts a missing host but Rust's [`SocketAddr`] parser
/// does not, so the bind-all translation is explicit on the Rust side.
pub const fn resolve_bind_host(raw: &str) -> &str {
    if raw.is_empty() { "0.0.0.0" } else { raw }
}

/// Test-only proxy for [`parse_log_level`]. Exposed via
/// [`crate::__test_internal::parse_stream_log_level`] so the warn-fire
/// test crate can assert the unrecognized-`--log-level`
/// `tracing::warn!` fires for the `mcp stream` surface independently
/// of `mcp start`.
pub fn parse_log_level_for_test(matches: &ArgMatches) -> Option<Level> {
    parse_log_level(matches)
}

/// Test-only proxy for [`build`]. Exposed via
/// [`crate::__test_internal::stream_subcommand`].
pub fn build_for_test() -> Command {
    build()
}

fn parse_log_level(matches: &ArgMatches) -> Option<Level> {
    super::common::parse_log_level(matches)
}

/// Install a `tracing_subscriber` pointed at stderr.
///
/// Precedence: explicit override > [`Config::log_level`] > `RUST_LOG`
/// environment > `INFO`. Idempotent (silently ignores re-init).
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
    fn stream_subcommand_has_full_flag_surface() {
        let cmd = build();
        let names: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert!(names.contains(&"host"), "missing --host: {names:?}");
        assert!(names.contains(&"port"), "missing --port: {names:?}");
        assert!(
            names.contains(&"log-level"),
            "missing --log-level: {names:?}"
        );
        assert!(
            names.contains(&"allow-host"),
            "missing --allow-host: {names:?}"
        );
    }

    #[test]
    fn allow_host_flag_parses_multiple_values() {
        let matches = build()
            .try_get_matches_from([
                "stream",
                "--allow-host",
                "foo.local",
                "--allow-host",
                "bar.local",
            ])
            .expect("parses");
        let hosts: Vec<String> = matches
            .get_many::<String>("allow-host")
            .expect("allow-host present")
            .cloned()
            .collect();
        assert_eq!(hosts, vec!["foo.local", "bar.local"]);
    }

    #[test]
    fn empty_host_translates_to_bind_all() {
        // Exercises the SUT directly — `resolve_bind_host` is the production
        // helper invoked by `run`. If the translation regresses (e.g. someone
        // changes the sentinel to "::" or removes the empty-string branch),
        // this assertion fires.
        assert_eq!(resolve_bind_host(""), "0.0.0.0");
        let addr: SocketAddr = format!("{}:{}", resolve_bind_host(""), 8080_u16)
            .parse()
            .expect("parse");
        assert_eq!(addr.port(), 8080);
        assert!(addr.ip().is_unspecified(), "0.0.0.0 must be unspecified");
    }

    #[test]
    fn non_empty_host_passes_through() {
        assert_eq!(resolve_bind_host("127.0.0.1"), "127.0.0.1");
        let addr: SocketAddr = format!("{}:{}", resolve_bind_host("127.0.0.1"), 8081_u16)
            .parse()
            .expect("parse");
        assert_eq!(addr.port(), 8081);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
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
        ] {
            let matches = Command::new("stream")
                .arg(Arg::new("log-level").long("log-level"))
                .try_get_matches_from(["stream", "--log-level", raw])
                .expect("parses");
            assert_eq!(parse_log_level(&matches), Some(expected), "raw={raw}");
        }
    }
}
