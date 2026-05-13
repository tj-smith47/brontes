//! Scoped `tracing` capture helper for warn-fire integration tests.
//!
//! # Why this exists
//!
//! brontes emits `tracing::warn!` at several sites that encode user-facing
//! behavior contracts:
//!
//! - PLAN §11 #7 (flag-value nested-container handling): when a tool call's
//!   JSON flag value contains a nested object/array that cannot be rendered
//!   as a scalar argv token, brontes warns and skips rather than passing the
//!   value through verbatim like ophis does.
//! - PLAN §11 #9 (unknown `--log-level` policy): brontes warns on an
//!   unrecognized level and falls back to `INFO`; ophis silently maps to
//!   `INFO`. The warn surfaces typos at startup rather than letting the
//!   user wonder why their level had no effect.
//! - PLAN line 537 (Phase 2 acceptance gate): the 64-character MCP tool
//!   name warn fires exactly once per offending tool so consumers know to
//!   set `Config::tool_name_prefix`.
//! - `OUTPUT_CAP_BYTES` soft-cap exhaustion: a runaway tool subprocess
//!   that floods stdout/stderr is silently truncated, with one warn per
//!   stream so operators see the truncation in their logs.
//! - Selector substring no-match: a `selectors::allow_cmds_containing(..)`
//!   needle that matches no walked command path is a misconfiguration the
//!   user wants to know about at startup.
//!
//! Without this infrastructure, every one of these `tracing::warn!` sites
//! could be deleted and CI would stay green. The helper closes that gap.
//!
//! # Design
//!
//! The helper installs a [`tracing_subscriber::fmt`]-style subscriber whose
//! writer is a shared `Mutex<Vec<u8>>` buffer, scoped via
//! [`tracing::subscriber::with_default`] — i.e., the subscriber is the
//! thread-local default for the duration of the supplied closure (or the
//! awaited async future). It is **not** installed globally; parallel
//! `cargo test` threads each get their own subscriber.
//!
//! The captured output is the formatter's standard text representation,
//! which includes both the event message AND its structured fields
//! (`field = value` pairs). Tests assert on substrings of the captured
//! text, so an assertion like `"value = \"verbose\""` exercises the
//! field-renderer path that the production logs would surface to an
//! operator.

#![allow(dead_code)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

/// Buffered writer that owns a `Mutex<Vec<u8>>` so multiple `fmt`
/// subscriber writes during a single `with_default` scope accumulate
/// into one buffer. Cloning shares the buffer (it is `Arc`-backed).
#[derive(Clone, Default)]
pub struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    /// Snapshot the buffer as a UTF-8 `String`. Non-UTF-8 bytes (which
    /// `tracing-subscriber`'s text formatter does not emit) lossy-replace.
    #[must_use]
    pub fn captured(&self) -> String {
        let guard = self.buf.lock().expect("capture buffer poisoned");
        String::from_utf8_lossy(&guard).into_owned()
    }
}

/// `MakeWriter` impl that hands the formatter a `BufHandle` writing into
/// our shared buffer. The handle takes the mutex lock for the lifetime of
/// each write call only; the formatter writes one event at a time so
/// the lock is not held across `await` points.
impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = BufHandle;

    fn make_writer(&'a self) -> Self::Writer {
        BufHandle {
            buf: Arc::clone(&self.buf),
        }
    }
}

/// One-call writer handle. `Drop` is a no-op; `flush` is a no-op (the
/// underlying `Vec<u8>` does not buffer beyond what the lock-guarded
/// `write_all` already commits).
pub struct BufHandle {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Write for BufHandle {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf
            .lock()
            .map_err(|_| {
                io::Error::other(
                    "warn-capture buffer poisoned (a prior test panicked while holding the lock)",
                )
            })?
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run a synchronous closure with a thread-local `WARN`-level
/// subscriber installed. Returns the captured output.
///
/// Use this for code paths that synchronously emit warns
/// (e.g., `parse_log_level`, `generate_tools`).
pub fn capture_warns<R>(body: impl FnOnce() -> R) -> (R, String) {
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_target(true)
        .without_time()
        .finish();
    let result = tracing::subscriber::with_default(subscriber, body);
    (result, writer.captured())
}

/// Run an async future with a thread-local `WARN`-level subscriber
/// installed for the duration of the future's polling.
///
/// `with_default` returns once the inner closure returns; for an async
/// body we drive the future to completion inside the closure via a
/// shared current-thread runtime so the subscriber is in scope for
/// every `poll`. Tests that need a multi-threaded runtime should
/// install the subscriber inside their own runtime block instead.
pub async fn capture_warns_async<F, R>(body: F) -> (R, String)
where
    F: std::future::Future<Output = R>,
{
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_target(true)
        .without_time()
        .finish();
    // `with_default` is sync, but it accepts any closure return — including
    // futures. We poll the future to completion inside the closure by
    // entering a local current-thread runtime; the subscriber is the
    // thread-local default for every poll because the runtime runs on
    // *this* thread.
    let (result, _captured_ref) = tracing::subscriber::with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let r = rt.block_on(body);
        (r, ())
    });
    (result, writer.captured())
}

/// Assert that `haystack` contains every substring in `needles`. On
/// failure, prints the full captured output once so the user sees what
/// actually fired.
pub fn assert_contains_all(haystack: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "expected captured output to contain {needle:?}\n--- captured ---\n{haystack}\n--- end ---"
        );
    }
}

/// Count occurrences of `needle` in `haystack`. Used to assert that a
/// once-per-event warn (e.g., 64-char tool name) fires exactly once.
#[must_use]
pub fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}
