//! Streamable-HTTP MCP smoke test.
//!
//! Boots `serve_http` against a `127.0.0.1:0` ephemeral port, drives a real
//! MCP `initialize` + `tools/list` exchange via raw `reqwest` POSTs, and
//! asserts both that the server speaks MCP and that token cancellation
//! tears the server down within the 5-second graceful-shutdown window.
//!
//! Coverage:
//! - End-to-end HTTP: initialize then tools/list returns the walked tree.
//!   Retained as the backward-compatibility path — protocol versions before
//!   `2026-07-28` still handshake and still carry `Mcp-Session-Id`.
//! - The stateless `2026-07-28` path: a bare `tools/list` with SEP-2243
//!   standard headers and no handshake, no session id.
//! - A promoted flag's `Mcp-Param-*` header validated against the body, which
//!   is the only place brontes' `x-mcp-header` annotation is observable: the
//!   annotation is worth emitting exactly because a mismatch is rejected.
//! - Cancellation: dropping the token cancels the accept loop and the
//!   serve future resolves within the 5-second `SHUTDOWN_GRACE`.
//!
//! Uses the `__test_internal::serve_http` re-export (mirrors the
//! stdio-side `BrontesServer` re-export) so the integration test crate
//! can drive the same code path the `mcp stream` subcommand uses.

use std::net::SocketAddr;
use std::time::Duration;

use clap::Command;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::serve_http;

/// Build a tiny CLI so the walker has something to surface as a tool.
fn fixture_cli() -> Command {
    Command::new("brontes-http-smoke")
        .version("0.0.1")
        .subcommand(
            Command::new("greet").about("Say hi").arg(
                clap::Arg::new("region")
                    .long("region")
                    .help("Target region"),
            ),
        )
        .subcommand(Command::new("status").about("Show status"))
}

/// Bind a random local TCP port, return the address (the listener is
/// dropped before the server takes the same port — fine on Linux since
/// the kernel won't immediately reassign it).
async fn pick_free_port() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = l.local_addr().expect("local_addr");
    drop(l);
    addr
}

/// Boot `serve_http` on an ephemeral port and wait for the listener to come
/// up, so every test below is exercising a running server rather than a
/// `serve_http` that failed at bind time. Polling a successful TCP connect is
/// more reliable than a fixed sleep.
async fn spawn_server() -> (SocketAddr, CancellationToken, tokio::task::JoinHandle<()>) {
    spawn_server_with(brontes::Config::default()).await
}

async fn spawn_server_with(
    cfg: brontes::Config,
) -> (SocketAddr, CancellationToken, tokio::task::JoinHandle<()>) {
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();

    let server_cancel = cancel.clone();
    let server_task = tokio::spawn(async move {
        serve_http(fixture_cli(), cfg, addr, server_cancel, vec![])
            .await
            .expect("serve_http");
    });

    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    (addr, cancel, server_task)
}

/// Build an MCP `initialize` JSON-RPC request body.
const fn initialize_body() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"brontes-test","version":"0.0.1"}}}"#
}

/// Build a `tools/list` body with the given numeric id.
fn tools_list_body(id: u64) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#)
}

/// Build a `notifications/initialized` JSON-RPC notification body
/// (no id; the MCP spec requires this after the initialize response).
const fn initialized_notification() -> &'static str {
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
}

/// Parse an SSE body assuming SEP-1699 framing: one priming empty `data:`
/// line, then exactly one JSON payload `data:` line. If rmcp changes this
/// framing shape, the assertion below catches it.
fn parse_sse_data(body: &str) -> serde_json::Value {
    let payloads: Vec<&str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        payloads.len(),
        1,
        "expected exactly one non-empty SSE data line, got {} in body:\n{body}",
        payloads.len()
    );
    serde_json::from_str(payloads[0]).expect("payload is valid JSON")
}

#[tokio::test]
async fn http_initialize_then_tools_list_returns_walked_tree() {
    let (addr, cancel, server_task) = spawn_server().await;

    let url = format!("http://{addr}/");
    let client = reqwest::Client::new();

    // 1. initialize.
    let init_resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(initialize_body())
        .send()
        .await
        .expect("initialize send");
    assert_eq!(init_resp.status(), 200, "initialize must return 200");
    let session_id = init_resp
        .headers()
        .get("mcp-session-id")
        .expect("server must mint Mcp-Session-Id in stateful mode")
        .to_str()
        .expect("session id is ascii")
        .to_string();
    let init_body_text = init_resp.text().await.expect("read init body");
    let init_json = parse_sse_data(&init_body_text);
    assert_eq!(init_json["jsonrpc"], "2.0");
    assert!(
        init_json["result"]["serverInfo"]["name"].is_string(),
        "initialize must return serverInfo: {init_json}"
    );

    // 2. notifications/initialized (MCP spec: client confirms readiness).
    let notif_resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .body(initialized_notification())
        .send()
        .await
        .expect("initialized notification send");
    assert!(
        notif_resp.status().is_success() || notif_resp.status() == 202,
        "initialized notification status: {:?}",
        notif_resp.status()
    );

    // 3. tools/list against the same session.
    let list_resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session_id)
        .body(tools_list_body(2))
        .send()
        .await
        .expect("tools/list send");
    assert_eq!(list_resp.status(), 200);
    let list_body_text = list_resp.text().await.expect("read list body");
    let list_json = parse_sse_data(&list_body_text);
    assert_eq!(list_json["id"], 2);
    let tools = list_json["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list must return an array: {list_json}"));
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"brontes-http-smoke_greet"),
        "missing greet tool; got {names:?}"
    );
    assert!(
        names.contains(&"brontes-http-smoke_status"),
        "missing status tool; got {names:?}"
    );

    // Cancel and assert graceful shutdown within the 5s window.
    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}

#[tokio::test]
async fn http_2026_07_28_tools_list_needs_no_handshake_or_session() {
    let (addr, cancel, server_task) = spawn_server().await;

    let url = format!("http://{addr}/");
    let client = reqwest::Client::new();

    // A single POST, no `initialize` and no `Mcp-Session-Id`: SEP-2567
    // removed protocol-level sessions and SEP-2575 removed the handshake.
    // SEP-2243 requires `Mcp-Method` to agree with the body's method for any
    // request declaring 2026-07-28 (`Mcp-Name` applies only to methods that
    // name a target, which `tools/list` does not).
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .body(tools_list_body(1))
        .send()
        .await
        .expect("stateless tools/list send");

    assert_eq!(
        resp.status(),
        200,
        "a bare 2026-07-28 tools/list must be served without a handshake"
    );
    assert!(
        resp.headers().get("mcp-session-id").is_none(),
        "2026-07-28 is always stateless; the server must not mint a session id"
    );

    let body_text = resp.text().await.expect("read stateless body");
    let json = parse_sse_data(&body_text);
    assert!(
        json["error"].is_null(),
        "stateless tools/list must not error: {json}"
    );

    let result = &json["result"];
    // SEP-2322: every 2026-07-28 result carries a resultType discriminator.
    assert_eq!(
        result["resultType"], "complete",
        "expected a complete result: {json}"
    );
    // SEP-2549 cache hints, supplied by brontes rather than rmcp.
    assert_eq!(result["ttlMs"], 300_000, "missing ttlMs hint: {json}");
    assert_eq!(
        result["cacheScope"], "public",
        "missing cacheScope hint: {json}"
    );

    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list must return an array: {json}"))
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"brontes-http-smoke_greet"),
        "missing greet tool; got {names:?}"
    );

    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}

#[tokio::test]
async fn http_2026_07_28_rejects_a_mismatched_standard_header() {
    let (addr, cancel, server_task) = spawn_server().await;

    let url = format!("http://{addr}/");
    let client = reqwest::Client::new();

    // The silent-failure risk in SEP-2243 is a proxy rewriting the body
    // without the headers. brontes must surface that as the spec's
    // HeaderMismatch (-32020), not serve the request anyway.
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .body(tools_list_body(1))
        .send()
        .await
        .expect("mismatched header send");

    let json: serde_json::Value = resp.json().await.expect("error body is JSON");
    assert_eq!(
        json["error"]["code"], -32020,
        "expected HeaderMismatch for a Mcp-Method that disagrees with the body: {json}"
    );

    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}

/// A `tools/call` for the promoted-flag fixture, with `region` at the top
/// level where `Config::promote_flag` puts it.
fn promoted_call_body(region: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "brontes-http-smoke_greet",
            "arguments": { "region": region },
        },
    })
    .to_string()
}

#[tokio::test]
async fn http_2026_07_28_validates_a_promoted_flags_param_header() {
    // Emitting `x-mcp-header` only pays off if the receiving server actually
    // checks the copy against the body — otherwise a proxy that routed on the
    // header could forward a body that says something else entirely, and the
    // command would run with the value nobody routed on.
    let cfg = brontes::Config::default().promote_flag("brontes-http-smoke greet", "region");
    let (addr, cancel, server_task) = spawn_server_with(cfg).await;

    let url = format!("http://{addr}/");
    let client = reqwest::Client::new();

    let agreeing = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "brontes-http-smoke_greet")
        .header("Mcp-Param-region", "us-east-1")
        .body(promoted_call_body("us-east-1"))
        .send()
        .await
        .expect("agreeing header send");

    // A served call answers on the SSE stream; a refused one answers with a
    // plain JSON error body. The command itself is this test binary, so what
    // it exits with is irrelevant — reaching a result at all is the assertion.
    assert_eq!(
        agreeing.status(),
        200,
        "a header agreeing with the body must be served"
    );
    let body_text = agreeing.text().await.expect("read agreeing body");
    let json = parse_sse_data(&body_text);
    assert!(
        json["error"].is_null(),
        "a header agreeing with the body must not be rejected: {json}"
    );

    let disagreeing = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "brontes-http-smoke_greet")
        .header("Mcp-Param-region", "eu-west-1")
        .body(promoted_call_body("us-east-1"))
        .send()
        .await
        .expect("disagreeing header send");

    let json: serde_json::Value = disagreeing.json().await.expect("error body is JSON");
    assert_eq!(
        json["error"]["code"], -32020,
        "a promoted flag whose header and body disagree must be a HeaderMismatch; \
         without the x-mcp-header annotation reaching the schema there is nothing \
         to compare and this request would be served: {json}"
    );

    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}

/// Stateless `_meta` declaring the tasks extension, which is how a
/// `2026-07-28` client says it can accept a task handle.
fn tasks_client_meta() -> serde_json::Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": { "io.modelcontextprotocol/tasks": {} },
        },
    })
}

#[tokio::test]
async fn http_a_task_survives_the_request_that_created_it() {
    // The stateless revision has no session, so every request may be served by
    // a freshly built handler. A task stored on the handler that created it
    // would then be unreachable from the `tasks/get` that follows — tasks
    // would work over stdio and be silently useless over HTTP, which is the
    // transport where a long-running command needs them most.
    let cfg = brontes::Config::default()
        .task_mode_for("brontes-http-smoke status", brontes::TaskMode::Detached);
    let (addr, cancel, server_task) = spawn_server_with(cfg).await;

    let url = format!("http://{addr}/");
    let client = reqwest::Client::new();

    let call_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "brontes-http-smoke_status",
            "arguments": {},
            "_meta": tasks_client_meta(),
        },
    })
    .to_string();

    let created = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "brontes-http-smoke_status")
        .body(call_body)
        .send()
        .await
        .expect("detached tools/call send");

    let json = parse_sse_data(&created.text().await.expect("read create body"));
    assert_eq!(
        json["result"]["resultType"], "task",
        "a detached command must answer a tasks-capable client with a handle: {json}"
    );
    let task_id = json["result"]["taskId"]
        .as_str()
        .expect("a task handle carries a taskId")
        .to_owned();

    let get_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tasks/get",
        "params": { "taskId": task_id, "_meta": tasks_client_meta() },
    })
    .to_string();

    let fetched = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tasks/get")
        // SEP-2243 sources `Mcp-Name` from `params.taskId` for the task methods.
        .header("Mcp-Name", task_id.clone())
        .body(get_body)
        .send()
        .await
        .expect("tasks/get send");

    let json = parse_sse_data(&fetched.text().await.expect("read get body"));
    assert!(
        json["error"].is_null(),
        "the task created by the previous request must still be reachable: {json}"
    );
    assert_eq!(
        json["result"]["taskId"],
        task_id.as_str(),
        "tasks/get must answer for the task that was created: {json}"
    );

    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}

#[tokio::test]
async fn http_cancellation_tears_down_within_grace_window() {
    // No client traffic; just verify the bare accept loop respects the
    // cancellation token within the 5-second SHUTDOWN_GRACE.
    let (_addr, cancel, server_task) = spawn_server().await;

    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}
