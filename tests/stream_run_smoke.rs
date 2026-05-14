//! Drives `subcommands::stream::run_with_cancel` against a real bind on
//! `127.0.0.1:0` with a pre-cancelled token so `serve_http` returns
//! immediately after the listener comes up.
//!
//! Existing tests in `tests/server_http_smoke.rs` cover the HTTP runtime
//! end-to-end via `serve_http` directly. The gap this file closes is the
//! `stream::run` body itself: clap argv → log-level parse → host/port
//! resolution → `SocketAddr` build → `init_tracing` → listening-log →
//! delegate to `serve_http`. A pre-cancelled token short-circuits the
//! serve loop so the test exits in milliseconds without needing to
//! coordinate a SIGINT.

use clap::Command;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::stream_run_with_cancel;

fn build_stream_matches(argv: &[&str]) -> clap::ArgMatches {
    let cmd = brontes::__test_internal::stream_subcommand();
    let mut full: Vec<&str> = vec!["stream"];
    full.extend_from_slice(argv);
    cmd.try_get_matches_from(full).expect("parses")
}

fn build_cli() -> Command {
    Command::new("streamer-cli")
        .version("0.0.1")
        .subcommand(Command::new("greet"))
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_cancel_returns_when_token_is_pre_cancelled() {
    // Pre-cancellation: serve_http binds the listener, sees the token
    // already cancelled in its accept loop's `select!`, drains the
    // (empty) connection set and returns Ok. This exercises every
    // line in stream::run_with_cancel up through the serve_http call.
    let matches = build_stream_matches(&["--host", "127.0.0.1", "--port", "0"]);
    let cli = build_cli();

    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = stream_run_with_cancel(&matches, cli, None, cancel).await;
    result.expect("run_with_cancel returns Ok after pre-cancel");
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_cancel_threads_extra_allow_hosts() {
    // The `--allow-host` flag is repeatable; run_with_cancel collects
    // the list and threads it through to serve_http (which extends rmcp's
    // DNS-rebind allow-list). With pre-cancellation the value never
    // affects an actual HTTP request, but the parse + threading code
    // (lines that pull `get_many::<String>("allow-host")`) runs.
    let matches = build_stream_matches(&[
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--allow-host",
        "foo.local",
        "--allow-host",
        "bar.local",
        "--log-level",
        "warn",
    ]);
    let cli = build_cli();

    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = stream_run_with_cancel(&matches, cli, None, cancel).await;
    result.expect("run_with_cancel with allow-host + log-level returns Ok");
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_cancel_surfaces_invalid_host_as_config_error() {
    // A non-empty host that fails SocketAddr parse must surface as
    // `Error::Config`, NOT a panic. Pre-cancellation never engages
    // because the SocketAddr build fails first.
    let matches = build_stream_matches(&["--host", "not a valid host", "--port", "0"]);
    let cli = build_cli();

    let cancel = CancellationToken::new();
    let err = stream_run_with_cancel(&matches, cli, None, cancel)
        .await
        .expect_err("malformed host must error");
    assert!(
        matches!(err, brontes::Error::Config(_)),
        "expected Config error, got: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("invalid --host/--port combination"),
        "got: {msg}"
    );
}
