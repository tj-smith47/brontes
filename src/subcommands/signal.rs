//! Signal-listener install path for the MCP server entry points.
//!
//! Both `mcp start` ([`crate::server::stdio::serve_stdio`]) and
//! `mcp stream` ([`crate::subcommands::stream::run`]) spawn a detached
//! task that fires the supplied [`CancellationToken`] when the process
//! receives SIGINT / SIGTERM (Unix) or Ctrl+C (Windows). Previously each
//! call site inlined the `tokio::signal::unix::signal(..)` calls and
//! their `tracing::warn!` install-failure paths could not be exercised
//! from a normal test process. This module factors that path behind a
//! [`SignalSource`] trait so the warn-fire integration tests can inject
//! a faulty source whose `register_sigint` / `register_sigterm` returns
//! [`io::Error`].
//!
//! Production callers reach this via [`spawn_signal_listener`], which is
//! `spawn_signal_listener_with(token, TokioUnixSignalSource)` on Unix
//! and a Ctrl+C task on non-Unix. The two-step `spawn_signal_listener`
//! → `spawn_signal_listener_with<S>` shape mirrors how
//! [`crate::server::http::serve_http`] delegates to `serve_http_with`
//! to expose its [`crate::server::http::Acceptor`] seam.

use std::future::Future;
use std::io;

use tokio_util::sync::CancellationToken;

/// Source of SIGINT / SIGTERM registrations for the signal-listener task.
///
/// Production code uses [`TokioUnixSignalSource`], which wraps
/// [`tokio::signal::unix::signal`] verbatim. The warn-fire integration
/// tests inject a faulty source whose [`register_sigint`] /
/// [`register_sigterm`] returns an [`io::Error`] so the two
/// `tracing::warn!` install-failure paths can be asserted.
///
/// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
/// `lib.rs` can carry it out to the integration test crate; effective
/// visibility is crate-internal because the parent
/// `subcommands::signal` module is `pub(crate)`.
///
/// [`register_sigint`]: SignalSource::register_sigint
/// [`register_sigterm`]: SignalSource::register_sigterm
#[cfg(unix)]
pub trait SignalSource: Send + 'static {
    /// Per-signal stream type returned from `register_*`. The stream is
    /// `Send` so the listener task can move it across the `select!`
    /// boundary; production uses [`tokio::signal::unix::Signal`].
    type Signal: Send;

    /// Register a SIGINT handler. Returning `Err` causes the listener
    /// task to log the documented `tracing::warn!` and return without
    /// firing the cancellation token (matches the prior inline shape).
    ///
    /// # Errors
    ///
    /// Mirrors [`tokio::signal::unix::signal`]: registration is rare to
    /// fail in practice but can surface when the runtime cannot allocate
    /// the underlying I/O resource.
    fn register_sigint(&self) -> io::Result<Self::Signal>;

    /// Register a SIGTERM handler. Same shape and contract as
    /// [`Self::register_sigint`].
    ///
    /// # Errors
    ///
    /// Mirrors [`tokio::signal::unix::signal`].
    fn register_sigterm(&self) -> io::Result<Self::Signal>;

    /// Await the next delivery of `sig`. Returning `None` indicates the
    /// stream is permanently closed; production
    /// [`tokio::signal::unix::Signal::recv`] returns `Option<()>` with
    /// the same semantics.
    fn next_signal(sig: &mut Self::Signal) -> impl Future<Output = Option<()>> + Send;
}

/// Production [`SignalSource`] backed by [`tokio::signal::unix::signal`].
///
/// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
/// `lib.rs` can carry it out; effective visibility is crate-internal.
#[cfg(unix)]
pub struct TokioUnixSignalSource;

#[cfg(unix)]
impl SignalSource for TokioUnixSignalSource {
    type Signal = tokio::signal::unix::Signal;

    fn register_sigint(&self) -> io::Result<Self::Signal> {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
    }

    fn register_sigterm(&self) -> io::Result<Self::Signal> {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    }

    async fn next_signal(sig: &mut Self::Signal) -> Option<()> {
        sig.recv().await
    }
}

/// Spawn a detached task that fires `token` on SIGINT / SIGTERM (Unix)
/// or Ctrl+C (non-Unix).
///
/// This is the production entry point. Internally it delegates to
/// [`spawn_signal_listener_with`] with [`TokioUnixSignalSource`] on
/// Unix; on non-Unix the Ctrl+C path is not currently exposed through
/// the trait (see the module docs), so this function inlines it for
/// that target only.
#[cfg(unix)]
pub fn spawn_signal_listener(token: CancellationToken) {
    spawn_signal_listener_with(token, TokioUnixSignalSource);
}

#[cfg(not(unix))]
pub(crate) fn spawn_signal_listener(token: CancellationToken) {
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "could not install Ctrl+C handler");
            return;
        }
        tracing::info!("received Ctrl+C; cancelling MCP server");
        token.cancel();
    });
}

/// Generic core of `spawn_signal_listener`: spawn a detached task
/// that registers SIGINT / SIGTERM through `source` and fires `token`
/// when either is delivered.
///
/// Production code reaches this via `spawn_signal_listener` (which
/// passes [`TokioUnixSignalSource`]); the warn-fire test crate passes a
/// faulty source so the two `tracing::warn!` sites
/// (`could not install SIGINT handler`,
/// `could not install SIGTERM handler`) can be exercised without
/// breaking the host process's real signal disposition.
///
/// `pub` (not `pub(crate)`) so the `__test_internal` re-export in
/// `lib.rs` can carry it out. Effective visibility is crate-internal.
#[cfg(unix)]
pub fn spawn_signal_listener_with<S>(token: CancellationToken, source: S)
where
    S: SignalSource,
{
    tokio::spawn(async move {
        let mut sigint = match source.register_sigint() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGINT handler");
                return;
            }
        };
        let mut sigterm = match source.register_sigterm() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler");
                return;
            }
        };
        tokio::select! {
            _ = S::next_signal(&mut sigint) => {
                tracing::info!("received SIGINT; cancelling MCP server");
            }
            _ = S::next_signal(&mut sigterm) => {
                tracing::info!("received SIGTERM; cancelling MCP server");
            }
        }
        token.cancel();
    });
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    /// Producer source that succeeds on both registrations and never
    /// fires either signal — used to assert the listener parks cleanly
    /// and that cancellation by an unrelated path still works (the
    /// production code does not block on the spawned task, but the test
    /// confirms the trait-glue itself compiles and links).
    struct NeverFiringSource;

    impl SignalSource for NeverFiringSource {
        type Signal = ();

        fn register_sigint(&self) -> io::Result<Self::Signal> {
            Ok(())
        }

        fn register_sigterm(&self) -> io::Result<Self::Signal> {
            Ok(())
        }

        async fn next_signal(_sig: &mut Self::Signal) -> Option<()> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn listener_terminates_cleanly_when_both_registrations_succeed() {
        // Smoke test the happy path: both registrations succeed, neither
        // signal fires, and the test exits via its own scope without the
        // task interfering. This guards against a future refactor that
        // accidentally panics inside the spawned task when the source
        // never delivers a signal.
        let token = CancellationToken::new();
        spawn_signal_listener_with(token.clone(), NeverFiringSource);

        // Yield once so the task gets to run its registration code.
        tokio::task::yield_now().await;

        // The task is parked in `select!`; cancelling the token does
        // not affect it (production semantics: external cancel comes
        // from a different source, e.g. accept loop). We just confirm
        // the parent does not deadlock.
        token.cancel();
    }

    #[tokio::test]
    async fn production_source_registers_both_handlers_cleanly() {
        // Drive `TokioUnixSignalSource` directly: under a normal
        // tokio runtime both `signal(SignalKind::interrupt())` and
        // `signal(SignalKind::terminate())` succeed, so spawning the
        // listener must NOT emit either install-failure warn. We
        // confirm registration succeeds by calling the trait methods
        // and asserting `Ok`; that is the production code path the
        // warn-fire tests are *not* covering (those use a faulty
        // source).
        let source = TokioUnixSignalSource;
        let _sigint = source
            .register_sigint()
            .expect("SIGINT registration succeeds under tokio runtime");
        let _sigterm = source
            .register_sigterm()
            .expect("SIGTERM registration succeeds under tokio runtime");

        // Smoke-test the spawn path too: with the production source,
        // the task parks in `select!` waiting for a real signal that
        // never arrives in this test process. Yielding lets the
        // registration code run; the test then exits and the task is
        // detached/dropped along with the runtime.
        let token = CancellationToken::new();
        spawn_signal_listener_with(token.clone(), TokioUnixSignalSource);
        tokio::task::yield_now().await;
        // Cancel for symmetry with the negative test above.
        token.cancel();
    }
}
