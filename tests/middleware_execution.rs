//! End-to-end middleware execution tests.
//!
//! These tests drive a real `BrontesServer` over an in-memory duplex
//! transport (same shape as `server_stdio_smoke.rs`) and exercise the
//! middleware wire-up: that `Selector::middleware`, registered on a
//! `Config`, is actually invoked when an MCP client calls a tool, that the
//! `MiddlewareCtx` arrives with the right fields, that a panic inside
//! middleware is contained as a `tool_error` (not an rmcp transport tear-
//! down), and that middleware-driven timeouts return promptly without
//! waiting on the inner exec.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use clap::Command;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RoleClient;
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::BrontesServer;
use brontes::{BoxedNext, Middleware, MiddlewareCtx, Selector, ToolOutput};

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal client handler. The middleware tests only need the client peer
/// to drive RPCs; the server never initiates anything that would invoke a
/// client-side callback.
#[derive(Clone)]
struct NoopClient;

impl rmcp::handler::client::ClientHandler for NoopClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        rmcp::model::ClientInfo::default()
    }
}

/// Build a tiny CLI with one leaf so the walker has something to surface.
fn fixture_cli() -> Command {
    Command::new("brontes-mw")
        .version("0.0.1")
        .subcommand(Command::new("greet").about("Say hi"))
}

/// Wire client and server over an in-memory duplex transport. Returns the
/// running client peer, the cancellation token, and the server task handle.
async fn spin_up(
    cfg: brontes::Config,
) -> (
    rmcp::service::RunningService<RoleClient, NoopClient>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let (client_io, server_io) = duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let cancel = CancellationToken::new();
    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), cfg).expect("construct server");
            let running = server
                .serve_with_ct((server_read, server_write), cancel)
                .await
                .expect("server start");
            let _ = running.waiting().await;
        })
    };

    let client = NoopClient
        .serve_with_ct((client_read, client_write), cancel.clone())
        .await
        .expect("client start");

    (client, cancel, server_task)
}

async fn shutdown(
    client: rmcp::service::RunningService<RoleClient, NoopClient>,
    cancel: CancellationToken,
    server_task: tokio::task::JoinHandle<()>,
) {
    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — Ordering: middleware "before" runs, then exec (via next), then
// middleware "after". Validates that the chain dispatches `next(ctx)` and
// the post-await branch still executes.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn middleware_wraps_exec_before_and_after() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let log_for_mw = log.clone();

    let mw: Middleware = Arc::new(move |ctx: MiddlewareCtx, next: BoxedNext| {
        let log = log_for_mw.clone();
        Box::pin(async move {
            log.lock().unwrap().push("before");
            // `next` invokes `exec::run_tool` against the test binary.
            // The test binary doesn't understand the synthetic argv, so
            // the result will be a non-zero exit (or a spawn-side error).
            // The ordering proof is independent of that outcome — we just
            // need to know the post-await code ran.
            let result = next(ctx).await;
            log.lock().unwrap().push("after");
            result
        })
    });

    let cfg = brontes::Config::default().selector(Selector {
        middleware: Some(mw),
        ..Default::default()
    });

    let (client, cancel, server_task) = spin_up(cfg).await;

    let _ = client
        .peer()
        .call_tool(CallToolRequestParams::new("brontes-mw_greet"))
        .await;

    // Drop the user-visible result; what we care about is the event log.
    let events = log.lock().unwrap().clone();
    assert!(
        events.contains(&"before"),
        "middleware did not record 'before'; got {events:?}"
    );
    assert!(
        events.contains(&"after"),
        "middleware did not record 'after' (post-next branch did not run); got {events:?}"
    );

    let before_idx = events.iter().position(|e| *e == "before").unwrap();
    let after_idx = events.iter().position(|e| *e == "after").unwrap();
    assert!(
        before_idx < after_idx,
        "'before' must precede 'after'; events={events:?}"
    );

    shutdown(client, cancel, server_task).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — Context fields: middleware receives the expected tool_name and
// deserialized ToolInput. Asserts the cancellation token field is wired
// (non-cancelled at first delivery, cancellable downstream).
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct CapturedCtx {
    tool_name: String,
    flag_value: Option<serde_json::Value>,
    cancel_already_fired: bool,
    token_is_cancellable: bool,
}

#[tokio::test]
async fn middleware_ctx_carries_tool_name_input_and_token() {
    let captured: Arc<Mutex<CapturedCtx>> = Arc::new(Mutex::new(CapturedCtx::default()));
    let captured_for_mw = captured.clone();

    let mw: Middleware = Arc::new(move |ctx: MiddlewareCtx, _next: BoxedNext| {
        let captured = captured_for_mw.clone();
        Box::pin(async move {
            // Capture observable ctx fields.
            let already_fired = ctx.cancellation_token.is_cancelled();
            let token_clone = ctx.cancellation_token.clone();
            let flag = ctx.input.flags.get("verbose").cloned();
            *captured.lock().unwrap() = CapturedCtx {
                tool_name: ctx.tool_name.clone(),
                flag_value: flag,
                cancel_already_fired: already_fired,
                // Confirm we can actually cancel the (cloned) token; this
                // exercises the wiring from rmcp's per-request `ct`.
                token_is_cancellable: {
                    token_clone.cancel();
                    token_clone.is_cancelled()
                },
            };
            // Short-circuit: synthesize a clean output so we don't shell
            // out to the test binary.
            Ok(ToolOutput {
                stdout: "captured\n".into(),
                stderr: String::new(),
                exit_code: 0,
            })
        })
    });

    let cfg = brontes::Config::default().selector(Selector {
        middleware: Some(mw),
        ..Default::default()
    });

    let (client, cancel, server_task) = spin_up(cfg).await;

    let mut args = serde_json::Map::new();
    let mut flags = serde_json::Map::new();
    flags.insert("verbose".into(), serde_json::json!(true));
    args.insert("flags".into(), serde_json::Value::Object(flags));
    args.insert("args".into(), serde_json::Value::Array(vec![]));

    let call_result = client
        .peer()
        .call_tool(CallToolRequestParams::new("brontes-mw_greet").with_arguments(args))
        .await
        .expect("call_tool succeeds");
    assert_eq!(call_result.is_error, Some(false), "tool should succeed");

    let snap = captured.lock().unwrap().clone();
    assert_eq!(snap.tool_name, "brontes-mw_greet");
    assert_eq!(snap.flag_value, Some(serde_json::json!(true)));
    assert!(
        !snap.cancel_already_fired,
        "cancellation token must not be pre-fired on a fresh request"
    );
    assert!(
        snap.token_is_cancellable,
        "cloned cancellation token must reflect cancel() calls"
    );

    shutdown(client, cancel, server_task).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3 — Panic isolation: middleware that panics returns a tool_error
// to the client (CallToolResult with is_error: true) and the server keeps
// running so a subsequent request succeeds.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn middleware_panic_returns_tool_error_and_server_survives() {
    let mw: Middleware = Arc::new(|_ctx: MiddlewareCtx, _next: BoxedNext| {
        Box::pin(async move {
            panic!("synthetic middleware panic for test");
        })
    });

    let cfg = brontes::Config::default().selector(Selector {
        middleware: Some(mw),
        ..Default::default()
    });

    let (client, cancel, server_task) = spin_up(cfg).await;

    // First call: middleware panics. Expect a tool_error (Ok with
    // is_error: true), NOT an rmcp protocol-level Err.
    let first = client
        .peer()
        .call_tool(CallToolRequestParams::new("brontes-mw_greet"))
        .await
        .expect("call_tool returned an rmcp Err — server should have caught the panic");
    assert_eq!(
        first.is_error,
        Some(true),
        "panicking middleware must surface as tool_error; got {first:?}"
    );

    // Second call: server must still be alive. Use tools/list because it
    // doesn't go through middleware.
    let list = client
        .peer()
        .list_tools(None)
        .await
        .expect("server still reachable after middleware panic");
    assert!(
        list.tools
            .iter()
            .any(|t| t.name.as_ref() == "brontes-mw_greet"),
        "expected tool still visible after panic; got {list:?}"
    );

    shutdown(client, cancel, server_task).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4 — Middleware-driven timeout: a middleware that wraps a 1s sleep
// inside a 100ms `tokio::time::timeout` returns within ~200ms with a
// tool_error. The inner "long operation" is the middleware itself (not
// `next`) so the test is hermetic — we don't depend on exec actually
// hanging for 1s.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn middleware_timeout_returns_promptly() {
    let mw: Middleware = Arc::new(|_ctx: MiddlewareCtx, _next: BoxedNext| {
        Box::pin(async move {
            // Imagine `next` is a long-running subprocess. We don't
            // actually invoke it — we just simulate the "long operation"
            // here and wrap it in a 100ms timeout to verify the timeout
            // path round-trips as a tool_error.
            let outcome = tokio::time::timeout(
                Duration::from_millis(100),
                tokio::time::sleep(Duration::from_secs(1)),
            )
            .await;
            match outcome {
                Ok(()) => Ok(ToolOutput {
                    stdout: "unexpected".into(),
                    stderr: String::new(),
                    exit_code: 0,
                }),
                Err(_elapsed) => Err(brontes::Error::Io {
                    context: "middleware timeout after 100ms".into(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "middleware-driven timeout",
                    ),
                }),
            }
        })
    });

    let cfg = brontes::Config::default().selector(Selector {
        middleware: Some(mw),
        ..Default::default()
    });

    let (client, cancel, server_task) = spin_up(cfg).await;

    let start = std::time::Instant::now();
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("brontes-mw_greet"))
        .await
        .expect("call_tool returned an rmcp Err — should be Ok(tool_error)");
    let elapsed = start.elapsed();

    assert_eq!(
        result.is_error,
        Some(true),
        "timed-out middleware must produce a tool_error; got {result:?}"
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "middleware timeout must fire well before the 1s inner sleep; elapsed={elapsed:?}"
    );

    shutdown(client, cancel, server_task).await;
}
