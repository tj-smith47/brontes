//! `mcp stream` runtime: serve [`BrontesServer`] over rmcp's streamable HTTP transport.
//!
//! This module wires together:
//!
//! 1. The rmcp `StreamableHttpService` (under
//!    `rmcp::transport::streamable_http_server`) backed by a
//!    `LocalSessionManager` for in-memory MCP sessions.
//! 2. A hyper per-connection accept loop driven by `tokio::TcpListener`,
//!    bridging tokio I/O to hyper via [`hyper_util::rt::TokioIo`].
//! 3. A cancellation-token driven graceful shutdown with a 5-second
//!    timeout (parity with ophis `config.go::serveStreamableHTTP`,
//!    `config.go:101-110`).
//!
//! Argv parsing, signal-listener install, and the startup log line live
//! one layer up in [`crate::subcommands::stream`].
//!
//! Mirrors ophis `config.go::serveStreamableHTTP` (`config.go:97-126`).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::Command;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
// Bring the trait into scope under a non-shadowing alias so
// `service.call(req)` resolves on `StreamableHttpService` (which only
// implements `tower_service::Service`, not the inherent method).
use tower_service::Service as TowerService;

use crate::Result;
use crate::config::Config;
use crate::server::BrontesServer;

/// Grace window for in-flight HTTP connections to drain after the
/// cancellation token fires. Matches ophis `config.go:101-110`.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Serve a [`BrontesServer`] over the rmcp streamable-HTTP transport.
///
/// Binds a TCP listener at `addr`, wraps the host CLI in
/// `StreamableHttpService`, and accepts hyper HTTP/1.1 connections until
/// `cancel` fires. After cancellation, in-flight connections get up to
/// `SHUTDOWN_GRACE` (5 seconds) to drain; any still-pending join handles
/// are abandoned and the function returns.
///
/// The factory closure clones `cli` and `cfg` per request — the walk is
/// microseconds and the resulting per-session `BrontesServer` is
/// independent. Constructing one eagerly outside the closure surfaces
/// schema/config errors at startup rather than at first-request time.
///
/// # Errors
///
/// - [`crate::Error::Config`] when [`BrontesServer::new`] surfaces a
///   schema/config error at startup.
/// - [`crate::Error::Io`] when the TCP listener cannot bind.
// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
// `lib.rs` can carry it out to the integration test crate. The parent
// `server::http` module is `pub(crate)`, so this item is effectively
// crate-visible — only the `#[doc(hidden)]` `__test_internal` surface
// can reach it from outside.
pub async fn serve_http(
    cli: Command,
    cfg: Config,
    addr: SocketAddr,
    cancel: CancellationToken,
) -> Result<()> {
    // Eager pre-walk: any schema/config bug surfaces here, not on the
    // first inbound HTTP request.
    BrontesServer::new(cli.clone(), cfg.clone())?;

    let listener: TcpListener = TcpListener::bind(addr)
        .await
        .map_err(|e| crate::Error::Io {
            context: format!("bind streamable HTTP listener on {addr}"),
            source: e,
        })?;

    let factory_cli = cli;
    let factory_cfg = cfg;
    let session_manager = Arc::new(LocalSessionManager::default());
    // `StreamableHttpServerConfig` is `#[non_exhaustive]`; use the
    // default + builder rather than a struct literal so additive field
    // changes in future rmcp releases are no-ops here.
    let config = StreamableHttpServerConfig::default().with_cancellation_token(cancel.clone());

    let service: StreamableHttpService<BrontesServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                BrontesServer::new(factory_cli.clone(), factory_cfg.clone()).map_err(|e| {
                    // The factory must return std::io::Error; render the
                    // brontes Error via Display so the operator sees the
                    // same message they would have seen at startup.
                    std::io::Error::other(format!("brontes server construction: {e}"))
                })
            },
            session_manager,
            config,
        );

    // Accept loop: each successful accept spawns an isolated hyper
    // connection task. Both halves observe `cancel` — `accept()` exits
    // on cancel; spawned connections finish their current request.
    let tracker = TaskTracker::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                tracing::info!("cancellation token fired; stopping accept loop");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let io = TokioIo::new(stream);
                        let conn_service = service.clone();
                        let conn_cancel = cancel.clone();
                        tracker.spawn(async move {
                            // Adapt the cloneable rmcp `tower_service::Service`
                            // (only `FnMut`-style `call`) to a hyper
                            // `service_fn` (which wants `Fn`) by cloning per
                            // request — `StreamableHttpService` is internally
                            // `Arc`-backed so the clone is cheap.
                            let svc = service_fn(move |req: hyper::Request<Incoming>| {
                                let mut per_call = conn_service.clone();
                                let fut = TowerService::call(&mut per_call, req);
                                async move {
                                    // rmcp's `BoxResponse` is `Response<BoxBody<Bytes,
                                    // Infallible>>`; that body already implements
                                    // `http_body::Body` so hyper accepts it as-is.
                                    let resp = match fut.await {
                                        Ok(r) => r,
                                        Err(never) => match never {},
                                    };
                                    Ok::<_, Infallible>(resp)
                                }
                            });
                            let conn = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, svc);
                            tokio::pin!(conn);
                            tokio::select! {
                                res = conn.as_mut() => {
                                    if let Err(e) = res {
                                        tracing::debug!(error = %e, peer = %peer, "connection ended with error");
                                    }
                                }
                                () = conn_cancel.cancelled() => {
                                    // Trigger graceful shutdown on the connection;
                                    // hyper drains in-flight requests then returns.
                                    conn.as_mut().graceful_shutdown();
                                    if let Err(e) = conn.as_mut().await {
                                        tracing::debug!(error = %e, peer = %peer, "connection shutdown error");
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed; continuing");
                    }
                }
            }
        }
    }

    // Cancellation has fired. Close the tracker (no new tasks) and bound
    // the drain at SHUTDOWN_GRACE; abandon any laggards.
    tracker.close();
    if tokio::time::timeout(SHUTDOWN_GRACE, tracker.wait())
        .await
        .is_ok()
    {
        tracing::info!("HTTP server drained cleanly");
    } else {
        tracing::warn!(
            "HTTP server connections did not drain within {SHUTDOWN_GRACE:?}; abandoning"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SHUTDOWN_GRACE;
    use std::time::Duration;

    #[test]
    fn shutdown_grace_matches_ophis_5_seconds() {
        assert_eq!(SHUTDOWN_GRACE, Duration::from_secs(5));
    }
}
