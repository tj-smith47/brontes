//! `mcp start` runtime: serve [`BrontesServer`] over rmcp's stdio transport.
//!
//! This module wires together:
//!
//! 1. The tracing subscriber (always to stderr — stdout is reserved for
//!    MCP protocol frames).
//! 2. The rmcp stdio transport ([`rmcp::transport::stdio`]).
//! 3. A signal listener (SIGINT/SIGTERM on Unix; Ctrl+C on Windows) that
//!    cancels the running service for graceful shutdown.
//!
//! Mirrors ophis `config.go::serveStdio` (`config.go:88-95`).

use clap::Command;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tokio_util::sync::CancellationToken;
use tracing::Level;

use crate::Result;
use crate::config::Config;
use crate::server::BrontesServer;

/// Run the MCP server over stdio until the connected client disconnects,
/// the process receives a termination signal, or the cancellation token
/// fires.
///
/// `cli` is the user's full clap tree (cloned by the caller — clap's
/// `get_matches(self)` consumes the original). `cfg_opt` is the optional
/// user configuration; `None` and `Some(Config::default())` produce
/// identical behavior. `log_level_override` is the value of the
/// `--log-level` CLI flag — when set it wins over [`Config::log_level`].
///
/// stdout is reserved for MCP protocol frames; tracing output is always
/// directed to stderr.
///
/// # Errors
///
/// - [`crate::Error::McpInitialize`] if the rmcp transport fails to
///   negotiate the MCP handshake.
/// - [`crate::Error::Panic`] if the awaited rmcp service task panics
///   (the underlying tokio `JoinError`).
pub async fn serve_stdio(
    cli: Command,
    cfg_opt: Option<Config>,
    log_level_override: Option<Level>,
) -> Result<()> {
    let cfg = cfg_opt.unwrap_or_default();
    init_tracing(log_level_override.or(cfg.log_level));

    let cancel = CancellationToken::new();
    crate::subcommands::signal::spawn_signal_listener(cancel.clone());

    // `BrontesServer::new` walks the tree and caches the tool list at
    // construction; a bad config surfaces as a startup failure here rather
    // than as a silent server that fails the first `tools/list` request.
    let server = BrontesServer::new(cli, cfg)?;
    let running = server.serve_with_ct(stdio(), cancel).await?;

    // `waiting` returns the quit reason; a join error from the underlying
    // task represents a panic-in-task, not an I/O failure — surface it as
    // [`crate::Error::Panic`] so the category matches the cause.
    running
        .waiting()
        .await
        .map_err(|e| crate::Error::Panic(format!("MCP stdio service join: {e}")))?;
    Ok(())
}

/// Install a `tracing_subscriber` pointed at stderr.
///
/// Precedence: explicit override > `Config::log_level` > `RUST_LOG`
/// environment > `INFO`. The call is idempotent — `try_init` returns an
/// error if a subscriber is already installed (e.g., by a host binary's
/// test harness), which we silently ignore.
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
