//! Integration tests for every `tracing::warn!` site brontes emits.
//!
//! Each `tracing::warn!` site in `src/` encodes a user-facing behavior
//! contract — see `tests/support/mod.rs` for the rationale. The tests in
//! this file assert the warns actually fire with the documented field
//! names and values, so silently deleting a warn (or changing its
//! field names) is caught by CI.
//!
//! Coverage map (warn site → test fn):
//!
//! - `src/subcommands/start.rs::parse_log_level` (unknown level)
//!   → [`start_unknown_log_level_warns`]
//! - `src/subcommands/stream.rs::parse_log_level` (same shape, separate
//!   surface) → [`stream_unknown_log_level_warns`]
//! - `src/exec.rs::append_flag` object-with-nested
//!   → [`flag_object_with_nested_object_warns`],
//!   [`flag_object_with_nested_array_warns`]
//! - `src/exec.rs::append_scalar_flag` array-with-nested
//!   → [`flag_array_with_nested_object_warns`],
//!   [`flag_array_with_nested_array_warns`]
//! - `src/command.rs` 64-char tool-name warn
//!   → [`tool_name_over_64_chars_warns_once`]
//! - `src/command.rs` selector substring no-match warn
//!   → [`selector_substring_no_match_warns`]
//! - `src/exec.rs::read_capped` stdout/stderr `OUTPUT_CAP_BYTES` exhaustion
//!   → [`read_capped_stdout_emits_one_warn`],
//!   [`read_capped_stderr_emits_one_warn`]
//!
//! - `src/server/http.rs::serve_http_with` accept-loop failure
//!   (faulty `Acceptor`) → [`http_accept_failure_emits_continuing_warn`]
//! - `src/server/http.rs::serve_http_with` grace-window exceeded
//!   (idle TCP client + compressed grace) →
//!   [`http_grace_window_exceeded_emits_warn`]
//!
//! Uncovered (surfaced as SUGGESTs in the implementer report, not in CI):
//!
//! - `src/subcommands/signal.rs::spawn_signal_listener_with` SIGINT-
//!   register failure (faulty `SignalSource`) →
//!   [`signal_sigint_register_failure_emits_warn`]
//! - `src/subcommands/signal.rs::spawn_signal_listener_with` SIGTERM-
//!   register failure (faulty `SignalSource`) →
//!   [`signal_sigterm_register_failure_emits_warn`]

mod support;

use brontes::{Config, Selector, selectors};
use clap::Command;
use serde_json::json;

use support::{assert_contains_all, capture_warns, capture_warns_async, count_occurrences};

// ---------------------------------------------------------------------------
// unrecognized `--log-level`
// ---------------------------------------------------------------------------

#[test]
fn start_unknown_log_level_warns() {
    let cmd = brontes::__test_internal::start_subcommand();
    let matches = cmd
        .try_get_matches_from(["start", "--log-level", "foobar"])
        .expect("clap parses --log-level even when the value is unknown");

    let (result, captured) =
        capture_warns(|| brontes::__test_internal::parse_start_log_level(&matches));
    assert!(
        result.is_none(),
        "unknown level must fall through to default (None)"
    );
    assert_contains_all(
        &captured,
        &["WARN", "unrecognized --log-level", "value=foobar"],
    );
}

#[test]
fn stream_unknown_log_level_warns() {
    let cmd = brontes::__test_internal::stream_subcommand();
    let matches = cmd
        .try_get_matches_from(["stream", "--log-level", "verbose"])
        .expect("clap parses --log-level even when the value is unknown");

    let (result, captured) =
        capture_warns(|| brontes::__test_internal::parse_stream_log_level(&matches));
    assert!(
        result.is_none(),
        "unknown level must fall through to default (None)"
    );
    assert_contains_all(
        &captured,
        &["WARN", "unrecognized --log-level", "value=verbose"],
    );
}

#[test]
fn start_known_log_level_does_not_warn() {
    // Negative test: a recognized level must NOT trip the warn. Guards
    // against accidental over-firing if the match arms drift.
    let cmd = brontes::__test_internal::start_subcommand();
    let matches = cmd
        .try_get_matches_from(["start", "--log-level", "debug"])
        .expect("parses");
    let (result, captured) =
        capture_warns(|| brontes::__test_internal::parse_start_log_level(&matches));
    assert_eq!(result, Some(tracing::Level::DEBUG));
    assert!(
        !captured.contains("unrecognized --log-level"),
        "must not warn on a recognized level; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// flag-value nested-container handling
// ---------------------------------------------------------------------------

#[test]
fn flag_object_with_nested_object_warns() {
    // `{ "label": { "k": { "nested": "object" } } }` → nested Object value
    // at key "k" triggers the "object-valued flag contained a non-scalar
    // value; skipping" warn. The remaining scalar pair (none in this case)
    // is rendered; here every pair is skipped so argv is empty.
    let value = json!({"k": {"nested": "object"}});
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("label", &value, "myapp_sub"));
    assert!(
        argv.is_empty(),
        "nested-object value must be skipped; argv = {argv:?}"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "object-valued flag contained a non-scalar value; skipping",
            "tool=myapp_sub",
            "flag=label",
            "key=k",
        ],
    );
}

#[test]
fn flag_object_with_nested_array_warns() {
    // Same code path, array-valued inner pair.
    let value = json!({"items": ["a", "b"]});
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("label", &value, "myapp_sub"));
    assert!(
        argv.is_empty(),
        "nested-array value must be skipped; argv = {argv:?}"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "object-valued flag contained a non-scalar value; skipping",
            "tool=myapp_sub",
            "flag=label",
            "key=items",
        ],
    );
}

#[test]
fn flag_array_with_nested_object_warns() {
    // `["scalar", {"x": 1}]` → first item renders, second item trips the
    // "nested non-scalar flag value; skipping" warn from
    // `append_scalar_flag`.
    let value = json!(["scalar", {"x": 1}]);
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("tag", &value, "myapp_sub"));
    assert_eq!(
        argv,
        vec!["--tag".to_string(), "scalar".to_string()],
        "only the scalar item renders; the object item is skipped"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "nested non-scalar flag value; skipping",
            "tool=myapp_sub",
            "flag=tag",
        ],
    );
}

#[test]
fn flag_array_with_nested_array_warns() {
    let value = json!([["nested"], "scalar"]);
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("tag", &value, "myapp_sub"));
    assert_eq!(
        argv,
        vec!["--tag".to_string(), "scalar".to_string()],
        "only the scalar item renders; the array item is skipped"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "nested non-scalar flag value; skipping",
            "tool=myapp_sub",
            "flag=tag",
        ],
    );
}

#[test]
fn flag_object_all_scalar_pairs_no_warn() {
    // Negative test: scalar-only object map must NOT trip the warn.
    let value = json!({"env": "prod", "version": 7});
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("label", &value, "myapp_sub"));
    // Two pairs, two `--label` flags.
    assert_eq!(count_occurrences(&format!("{argv:?}"), "--label"), 2);
    assert!(
        !captured.contains("WARN"),
        "scalar-only object must not warn; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// 64-char tool-name warn
// ---------------------------------------------------------------------------

#[test]
fn tool_name_over_64_chars_warns_once() {
    // Build a clap tree where the resulting tool name exceeds 64 chars.
    // Prefix `myapp` + `_` + a single subcommand whose name is 70 chars:
    //   myapp_aaaaaaaaaa... (5 + 1 + 70 = 76 chars).
    let long_leaf = "a".repeat(70);
    let root =
        Command::new("myapp").subcommand(Command::new(long_leaf.clone()).about("Long-named leaf"));

    let cfg = Config::default();
    let (tools, captured) =
        capture_warns(|| brontes::generate_tools(&root, &cfg).expect("generate_tools succeeds"));

    let expected_name = format!("myapp_{long_leaf}");
    assert!(
        tools.iter().any(|t| t.name.as_ref() == expected_name),
        "expected the long-named tool to be present"
    );

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "MCP tool name exceeds 64 characters",
            // Field assertions: name and len must be present.
            &format!("name={expected_name}"),
            &format!("len={}", expected_name.len()),
        ],
    );

    // Spec says "once per offending tool" — assert exactly one fire for
    // this name, not two.
    assert_eq!(
        count_occurrences(&captured, "MCP tool name exceeds 64 characters"),
        1,
        "64-char warn must fire exactly once per offending tool; captured:\n{captured}"
    );
}

// ---------------------------------------------------------------------------
// Selector substring no-match warn
// ---------------------------------------------------------------------------

#[test]
fn selector_substring_no_match_warns() {
    // CLI has two commands: `myapp greet` and `myapp status`. Selector
    // substring `xyz-nothing-matches` matches neither path; the warn
    // must fire with `needle = "xyz-nothing-matches"`.
    let root = Command::new("myapp")
        .subcommand(Command::new("greet").about("Greet"))
        .subcommand(Command::new("status").about("Status"));

    let cfg = Config::default().selector(Selector {
        cmd: Some(selectors::allow_cmds_containing(["xyz-nothing-matches"])),
        ..Default::default()
    });

    let (_tools, captured) = capture_warns(|| {
        brontes::generate_tools(&root, &cfg).expect("generate_tools succeeds (warn is non-fatal)")
    });

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "Selector substring matches no walked command path",
            "needle=xyz-nothing-matches",
        ],
    );
}

#[test]
fn selector_substring_matching_no_warn() {
    // Negative test: a substring that does match a path must NOT warn.
    let root = Command::new("myapp")
        .subcommand(Command::new("greet").about("Greet"))
        .subcommand(Command::new("status").about("Status"));

    let cfg = Config::default().selector(Selector {
        cmd: Some(selectors::allow_cmds_containing(["status"])),
        ..Default::default()
    });

    let (_tools, captured) =
        capture_warns(|| brontes::generate_tools(&root, &cfg).expect("generate_tools succeeds"));

    assert!(
        !captured.contains("Selector substring matches no walked command path"),
        "matching substring must not warn; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// OUTPUT_CAP_BYTES exhaustion — stdout and stderr each fire one warn
// ---------------------------------------------------------------------------

#[test]
fn read_capped_stdout_emits_one_warn() {
    // Build a reader that yields cap + 1 MiB of bytes; assert the
    // truncation warn fires exactly once with `stream = "stdout"`,
    // `tool = "long-tool"`, and `limit_bytes = <cap>`.
    let total = brontes::__test_internal::OUTPUT_CAP_BYTES + (1024 * 1024);
    let source = vec![0u8; total];

    let (retained, captured) = futures::executor::block_on(async move {
        let mut output: Option<Vec<u8>> = None;
        let ((), log) = capture_warns_async(async {
            let r = brontes::__test_internal::drain_capped(
                std::io::Cursor::new(source),
                "stdout",
                "long-tool".to_string(),
            )
            .await;
            output = Some(r);
        })
        .await;
        (output.expect("drain_capped produced output"), log)
    });

    assert_eq!(
        retained.len(),
        brontes::__test_internal::OUTPUT_CAP_BYTES,
        "retained bytes must equal the cap"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "tool output exceeded soft cap; further output truncated",
            "tool=long-tool",
            "stream=stdout",
            &format!("limit_bytes={}", brontes::__test_internal::OUTPUT_CAP_BYTES),
        ],
    );
    assert_eq!(
        count_occurrences(&captured, "tool output exceeded soft cap"),
        1,
        "warn must fire exactly once per stream; captured:\n{captured}"
    );
}

#[test]
fn read_capped_stderr_emits_one_warn() {
    let total = brontes::__test_internal::OUTPUT_CAP_BYTES + (512 * 1024);
    let source = vec![0u8; total];

    let captured = futures::executor::block_on(async move {
        let ((), log) = capture_warns_async(async {
            brontes::__test_internal::drain_capped(
                std::io::Cursor::new(source),
                "stderr",
                "noisy-tool".to_string(),
            )
            .await;
        })
        .await;
        log
    });

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "tool output exceeded soft cap; further output truncated",
            "tool=noisy-tool",
            "stream=stderr",
        ],
    );
    assert_eq!(
        count_occurrences(&captured, "tool output exceeded soft cap"),
        1,
    );
}

#[test]
fn read_capped_under_cap_no_warn() {
    // Negative test: below-cap input must NOT warn.
    let payload = b"hello world".to_vec();
    let captured = futures::executor::block_on(async move {
        let ((), log) = capture_warns_async(async {
            brontes::__test_internal::drain_capped(
                std::io::Cursor::new(payload),
                "stdout",
                "quiet-tool".to_string(),
            )
            .await;
        })
        .await;
        log
    });
    assert!(
        !captured.contains("tool output exceeded soft cap"),
        "below-cap must not warn; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// HTTP transport — `accept failed; continuing` and grace-window warns
//
// Both tests drive [`serve_http_with`] via the `__test_internal` re-export so
// the production accept loop is exercised end-to-end. The grace test uses a
// compressed `shutdown_grace` so the suite stays fast.
// ---------------------------------------------------------------------------

use std::future::{Future, pending};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::{Acceptor, TokioIo, serve_http_with};

/// Faulty acceptor for the `accept failed; continuing` warn-fire path.
///
/// First call: returns `Err(io::ErrorKind::Other)` — production warns and
/// loops. Subsequent calls: `pending().await` forever, so the outer
/// `cancel.cancelled()` branch wins once the test fires the token. This
/// avoids a tight-spin between the faulty acceptor and the warn site.
struct FaultyAcceptor {
    calls: AtomicUsize,
}

impl FaultyAcceptor {
    const fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl Acceptor for FaultyAcceptor {
    fn accept(&self) -> impl Future<Output = io::Result<(TokioIo<TcpStream>, SocketAddr)>> + Send {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        async move {
            if n == 0 {
                Err(io::Error::other("synthetic accept failure"))
            } else {
                // Never resolve; the outer `cancel.cancelled()` branch
                // of the select! in `serve_http_with` wins instead.
                pending::<io::Result<(TokioIo<TcpStream>, SocketAddr)>>().await
            }
        }
    }
}

#[test]
fn http_accept_failure_emits_continuing_warn() {
    let cancel = CancellationToken::new();
    let inner_cancel = cancel.clone();

    let captured = futures::executor::block_on(async move {
        let ((), log) = capture_warns_async(async move {
            let acceptor = FaultyAcceptor::new();
            let cli = clap::Command::new("brontes-warn-accept")
                .version("0.0.1")
                .subcommand(clap::Command::new("greet").about("Say hi"));

            let server = tokio::spawn(async move {
                serve_http_with(
                    cli,
                    Config::default(),
                    acceptor,
                    inner_cancel,
                    vec![],
                    // Short grace so even if drain stalls the test stays fast.
                    Duration::from_millis(50),
                )
                .await
                .expect("serve_http_with returns Ok after cancellation");
            });

            // Give the accept loop time to consume the first Err and
            // emit the warn before we cancel.
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();

            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("serve_http_with exits within 5s of cancellation")
                .expect("server task did not panic");
        })
        .await;
        log
    });

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "accept failed; continuing",
            // Production renders the synthetic message via `error = %e`.
            "synthetic accept failure",
        ],
    );
    // Belt and braces: the faulty acceptor returns the error once, so the
    // warn must fire exactly once. Multiple fires would indicate the loop
    // re-polled the same already-consumed error, which would be a tight
    // spin in production.
    assert_eq!(
        count_occurrences(&captured, "accept failed; continuing"),
        1,
        "accept-failure warn must fire exactly once per error; captured:\n{captured}"
    );
}

#[test]
fn http_grace_window_exceeded_emits_warn() {
    // Compressed grace + an idle (HTTP-silent) TCP client keeps the
    // per-connection task in hyper's read loop past the 50ms grace so
    // `tracker.wait()` times out and the warn fires.
    let captured = futures::executor::block_on(async move {
        let ((), log) = capture_warns_async(async move {
            // Bind a real listener at 127.0.0.1:0 (production acceptor
            // path — this exercises `TokioTcpAcceptor` itself).
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind ephemeral port");
            let addr: SocketAddr = listener.local_addr().expect("local_addr");
            let acceptor = brontes::__test_internal::TokioTcpAcceptor::new(listener);

            let cancel = CancellationToken::new();
            let server_cancel = cancel.clone();
            let cli = clap::Command::new("brontes-warn-grace")
                .version("0.0.1")
                .subcommand(clap::Command::new("greet").about("Say hi"));

            let server = tokio::spawn(async move {
                serve_http_with(
                    cli,
                    Config::default(),
                    acceptor,
                    server_cancel,
                    vec![],
                    Duration::from_millis(50),
                )
                .await
                .expect("serve_http_with returns Ok after cancellation");
            });

            // Open a TCP connection and send a partial HTTP request line
            // (no `\r\n\r\n` terminator). hyper stays in the request-line
            // read loop waiting for more bytes; `graceful_shutdown` flips
            // the no-new-keepalive flag but cannot abort the in-flight
            // read, so the per-connection task hangs past the 50ms grace.
            let mut client = TcpStream::connect(addr)
                .await
                .expect("connect to test server");
            // Half a request line — enough that hyper sees bytes and
            // commits to parsing, not so much that hyper completes parsing.
            client
                .write_all(b"GET /")
                .await
                .expect("write partial request");
            // Yield to let the server task accept the connection and
            // enter the hyper read loop before we cancel.
            tokio::time::sleep(Duration::from_millis(50)).await;

            cancel.cancel();

            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .expect("serve_http_with exits within 5s of cancellation")
                .expect("server task did not panic");

            // Keep the client alive until after the server has exited so
            // hyper does not see the socket close before the grace window
            // elapses; drop it now.
            drop(client);
        })
        .await;
        log
    });

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "did not drain within",
            // Compressed grace renders via `Debug` of `Duration` — the
            // production format string is `{shutdown_grace:?}`, which for
            // 50ms is `50ms`.
            "50ms",
        ],
    );
}

// ---------------------------------------------------------------------------
// Signal-listener — SIGINT / SIGTERM register-failure warns
//
// `cfg(unix)` matches the production gate on
// `src/subcommands/signal.rs::spawn_signal_listener_with`. Each test
// injects a faulty `SignalSource` so the documented `tracing::warn!`
// fires deterministically without the host process actually being out
// of signal-handler resources.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod signal_warns {
    use std::future::pending;
    use std::io;

    use tokio_util::sync::CancellationToken;

    use super::{assert_contains_all, capture_warns_async};
    use brontes::__test_internal::{SignalSource, spawn_signal_listener_with};

    /// `SignalSource` whose `register_sigint` always returns an
    /// `io::Error`. Drives the
    /// `"could not install SIGINT handler"` warn path.
    struct SigintFails;

    impl SignalSource for SigintFails {
        type Signal = ();

        fn register_sigint(&self) -> io::Result<Self::Signal> {
            Err(io::Error::other("synthetic SIGINT registration failure"))
        }

        fn register_sigterm(&self) -> io::Result<Self::Signal> {
            // Unreachable in the warn-fire path — SIGINT fails first
            // and the listener returns before touching SIGTERM. Returns
            // `Ok(())` for completeness so the trait contract holds.
            Ok(())
        }

        async fn next_signal(_sig: &mut Self::Signal) -> Option<()> {
            pending().await
        }
    }

    /// `SignalSource` whose `register_sigint` succeeds but
    /// `register_sigterm` returns an `io::Error`. Drives the
    /// `"could not install SIGTERM handler"` warn path.
    struct SigtermFails;

    impl SignalSource for SigtermFails {
        type Signal = ();

        fn register_sigint(&self) -> io::Result<Self::Signal> {
            Ok(())
        }

        fn register_sigterm(&self) -> io::Result<Self::Signal> {
            Err(io::Error::other("synthetic SIGTERM registration failure"))
        }

        async fn next_signal(_sig: &mut Self::Signal) -> Option<()> {
            pending().await
        }
    }

    #[test]
    fn signal_sigint_register_failure_emits_warn() {
        let captured = futures::executor::block_on(async {
            let ((), log) = capture_warns_async(async {
                let token = CancellationToken::new();
                spawn_signal_listener_with(token, SigintFails);
                // Yield repeatedly to let the spawned task run its
                // register_sigint path and emit the warn before the
                // subscriber scope drops.
                for _ in 0..32 {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            log
        });

        assert_contains_all(
            &captured,
            &[
                "WARN",
                "could not install SIGINT handler",
                // Production renders the synthetic message via `error = %e`.
                "synthetic SIGINT registration failure",
            ],
        );
    }

    #[test]
    fn signal_sigterm_register_failure_emits_warn() {
        let captured = futures::executor::block_on(async {
            let ((), log) = capture_warns_async(async {
                let token = CancellationToken::new();
                spawn_signal_listener_with(token, SigtermFails);
                for _ in 0..32 {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            log
        });

        assert_contains_all(
            &captured,
            &[
                "WARN",
                "could not install SIGTERM handler",
                "synthetic SIGTERM registration failure",
            ],
        );
    }
}
