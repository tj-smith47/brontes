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
/// - [`crate::Error::Io`] if the awaited service task panics or the OS
///   signal listener cannot be installed.
pub(crate) async fn serve_stdio(
    cli: Command,
    cfg_opt: Option<Config>,
    log_level_override: Option<Level>,
) -> Result<()> {
    let cfg = cfg_opt.unwrap_or_default();
    init_tracing(log_level_override.or(cfg.log_level));

    // Pre-walk warning pass: surface long tool names once at startup, matching
    // the behavior already in `generate_tools`. We discard the resulting list
    // — `list_tools` will rebuild it on the first client request — but a bad
    // config must surface as a startup failure, not a silent server.
    crate::generate_tools(&cli, &cfg)?;

    let cancel = CancellationToken::new();
    spawn_signal_listener(cancel.clone());

    let server = BrontesServer::new(cli, cfg);
    let running = server
        .serve_with_ct(stdio(), cancel)
        .await
        .map_err(|e| crate::Error::McpInitialize(Box::new(e)))?;

    // `waiting` returns the quit reason; a join error from the underlying
    // task is unusual — treat it as an Io error so callers can surface it.
    running.waiting().await.map_err(|e| crate::Error::Io {
        context: "MCP stdio service join".into(),
        source: std::io::Error::other(e.to_string()),
    })?;
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

/// Spawn a task that cancels `token` on SIGINT/SIGTERM (Unix) or Ctrl+C
/// (Windows). The task is detached; cancellation propagates through the
/// rmcp service's drop guard whether the task or the main service exits
/// first.
#[cfg(unix)]
fn spawn_signal_listener(token: CancellationToken) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGINT handler");
                return;
            }
        };
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler");
                return;
            }
        };
        tokio::select! {
            _ = sigint.recv() => tracing::info!("received SIGINT; cancelling MCP server"),
            _ = sigterm.recv() => tracing::info!("received SIGTERM; cancelling MCP server"),
        }
        token.cancel();
    });
}

#[cfg(not(unix))]
fn spawn_signal_listener(token: CancellationToken) {
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "could not install Ctrl+C handler");
            return;
        }
        tracing::info!("received Ctrl+C; cancelling MCP server");
        token.cancel();
    });
}
