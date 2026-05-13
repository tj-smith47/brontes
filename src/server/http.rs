//! `mcp stream` runtime: serve [`BrontesServer`] over rmcp's streamable HTTP transport.
//!
//! This module wires together:
//!
//! 1. The rmcp `StreamableHttpService` (under
//!    `rmcp::transport::streamable_http_server`) backed by a
//!    `LocalSessionManager` for in-memory MCP sessions.
//! 2. A hyper per-connection accept loop driven by an injectable
//!    [`Acceptor`] (production: [`TokioTcpAcceptor`] wrapping
//!    [`tokio::net::TcpListener`]; tests: a faulty acceptor that
//!    exercises the `accept failed; continuing` warn-fire).
//! 3. A cancellation-token driven graceful shutdown with a 5-second
//!    timeout (parity with ophis `config.go::serveStreamableHTTP`,
//!    `config.go:101-110`).
//!
//! Argv parsing, signal-listener install, and the startup log line live
//! one layer up in [`crate::subcommands::stream`].
//!
//! Mirrors ophis `config.go::serveStreamableHTTP` (`config.go:97-126`).

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::Command;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::{TcpListener, TcpStream};
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
///
/// Production callers (the `mcp stream` subcommand) pass this value to
/// [`serve_http_with`] verbatim; the `__test_internal` surface accepts
/// an override so the warn-fire test crate can compress the grace
/// window without waiting five real seconds.
// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
// `lib.rs` can carry it out. Effective visibility is crate-internal
// because the parent `server::http` module is `pub(crate)`.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Source of incoming TCP connections for the streamable-HTTP server.
///
/// Production uses [`TokioTcpAcceptor`] wrapping
/// [`tokio::net::TcpListener`]; tests inject a faulty acceptor that
/// returns [`io::Error`] from `accept()` to exercise the
/// `accept failed; continuing` warn-fire path inside [`serve_http_with`].
///
/// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
/// `lib.rs` can carry it out to the integration test crate; effective
/// visibility is crate-internal because the parent `server::http`
/// module is `pub(crate)`.
pub trait Acceptor: Send + Sync + 'static {
    /// Wait for the next incoming connection and return either the
    /// bridged hyper-compatible IO + peer address, or an `io::Error`
    /// that the accept loop logs and continues past.
    fn accept(&self) -> impl Future<Output = io::Result<(TokioIo<TcpStream>, SocketAddr)>> + Send;
}

/// Production [`Acceptor`] backed by [`tokio::net::TcpListener`].
///
/// Construct via [`bind_default_acceptor`] from a [`SocketAddr`].
// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
// `lib.rs` can carry it out; effective visibility is crate-internal.
pub struct TokioTcpAcceptor {
    listener: TcpListener,
}

impl TokioTcpAcceptor {
    /// Wrap a bound [`TcpListener`] as an [`Acceptor`]. The listener
    /// must already be bound; [`bind_default_acceptor`] is the
    /// production constructor that handles the bind.
    ///
    // `pub` (not `pub(crate)`) so the `__test_internal` re-export in
    // `lib.rs` can carry it out; the warn-fire test crate constructs one
    // around its own `TcpListener` to drive the grace-window warn path.
    // Effective visibility is crate-internal because the parent
    // `server::http` module is `pub(crate)`.
    pub const fn new(listener: TcpListener) -> Self {
        Self { listener }
    }
}

impl Acceptor for TokioTcpAcceptor {
    async fn accept(&self) -> io::Result<(TokioIo<TcpStream>, SocketAddr)> {
        let (stream, peer) = self.listener.accept().await?;
        Ok((TokioIo::new(stream), peer))
    }
}

/// Bind a [`TcpListener`] at `addr` and wrap it as a [`TokioTcpAcceptor`].
///
/// This is the production constructor used by [`serve_http`]; the bind
/// ceremony is factored out so the warn-fire test crate can inject a
/// faulty acceptor without taking a real port.
///
/// # Errors
///
/// [`crate::Error::Io`] when the underlying [`TcpListener::bind`] fails
/// (port in use, insufficient privileges, etc.).
// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
// `lib.rs` can carry it out; effective visibility is crate-internal.
pub async fn bind_default_acceptor(addr: SocketAddr) -> Result<TokioTcpAcceptor> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| crate::Error::Io {
            context: format!("bind streamable HTTP listener on {addr}"),
            source: e,
        })?;
    Ok(TokioTcpAcceptor::new(listener))
}

/// Serve a [`BrontesServer`] over the rmcp streamable-HTTP transport.
///
/// Binds a TCP listener at `addr`, wraps the host CLI in
/// `StreamableHttpService`, and accepts hyper HTTP/1.1 connections until
/// `cancel` fires. After cancellation, in-flight connections get up to
/// [`SHUTDOWN_GRACE`] (5 seconds) to drain; any still-pending join handles
/// are abandoned and the function returns.
///
/// The factory closure clones `cli` and `cfg` per request — the walk is
/// microseconds and the resulting per-session `BrontesServer` is
/// independent. Constructing one eagerly outside the closure surfaces
/// schema/config errors at startup rather than at first-request time.
///
/// This is the production entry point; internally it delegates to
/// [`serve_http_with`] after binding a [`TokioTcpAcceptor`] at `addr`.
/// The warn-fire test crate uses [`serve_http_with`] directly so it can
/// inject a faulty acceptor and a compressed shutdown grace.
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
    extra_allowed_hosts: Vec<String>,
) -> Result<()> {
    let acceptor = bind_default_acceptor(addr).await?;
    serve_http_with(
        cli,
        cfg,
        acceptor,
        cancel,
        extra_allowed_hosts,
        SHUTDOWN_GRACE,
    )
    .await
}

/// Generic core of [`serve_http`]: serve [`BrontesServer`] over the
/// supplied [`Acceptor`] with a caller-chosen `shutdown_grace`.
///
/// Production code reaches this via [`serve_http`] (which passes
/// [`TokioTcpAcceptor`] + [`SHUTDOWN_GRACE`]); the warn-fire test crate
/// passes a faulty acceptor and a compressed grace window so the two
/// `tracing::warn!` sites (`accept failed; continuing`,
/// `connections did not drain within ...`) can be exercised without
/// waiting five real seconds or breaking a live TCP listener.
///
/// # Errors
///
/// - [`crate::Error::Config`] when [`BrontesServer::new`] surfaces a
///   schema/config error at startup.
// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
// `lib.rs` can carry it out. Effective visibility is crate-internal
// because the parent `server::http` module is `pub(crate)` and the
// [`Acceptor`] trait that bounds `A` is `pub`.
pub async fn serve_http_with<A>(
    cli: Command,
    cfg: Config,
    acceptor: A,
    cancel: CancellationToken,
    extra_allowed_hosts: Vec<String>,
    shutdown_grace: Duration,
) -> Result<()>
where
    A: Acceptor,
{
    // Eager pre-walk: any schema/config bug surfaces here, not on the
    // first inbound HTTP request.
    BrontesServer::new(cli.clone(), cfg.clone())?;

    let factory_cli = cli;
    let factory_cfg = cfg;
    let session_manager = Arc::new(LocalSessionManager::default());
    // `StreamableHttpServerConfig` is `#[non_exhaustive]`; use the
    // default + builder rather than a struct literal so additive field
    // changes in future rmcp releases are no-ops here.
    //
    // rmcp's default allowed-hosts is ["localhost", "127.0.0.1", "::1"]
    // (DNS-rebind guard). `with_allowed_hosts` *replaces* the list, so
    // we start from the default and append any user-supplied hosts.
    let mut allowed_hosts = StreamableHttpServerConfig::default().allowed_hosts;
    allowed_hosts.extend(extra_allowed_hosts);
    let config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancel.clone())
        .with_allowed_hosts(allowed_hosts);

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
            accepted = acceptor.accept() => {
                match accepted {
                    Ok((io, peer)) => {
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
    // the drain at `shutdown_grace`; abandon any laggards.
    tracker.close();
    if tokio::time::timeout(shutdown_grace, tracker.wait())
        .await
        .is_ok()
    {
        tracing::info!("HTTP server drained cleanly");
    } else {
        tracing::warn!(
            grace = ?shutdown_grace,
            "HTTP server connections did not drain within {shutdown_grace:?}; abandoning"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::SHUTDOWN_GRACE;

    #[test]
    fn shutdown_grace_matches_ophis_5_seconds() {
        assert_eq!(SHUTDOWN_GRACE, Duration::from_secs(5));
    }
}
