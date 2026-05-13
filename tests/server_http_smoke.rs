//! Streamable-HTTP MCP smoke test.
//!
//! Boots `serve_http` against a `127.0.0.1:0` ephemeral port, drives a real
//! MCP `initialize` + `tools/list` exchange via raw `reqwest` POSTs, and
//! asserts both that the server speaks MCP and that token cancellation
//! tears the server down within the 5-second graceful-shutdown window.
//!
//! Coverage:
//! - End-to-end HTTP: initialize then tools/list returns the walked tree.
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
        .subcommand(Command::new("greet").about("Say hi"))
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

/// Parse an SSE body assuming rmcp 1.6 / SEP-1699 framing: one priming empty
/// `data:` line, then exactly one JSON payload `data:` line. If rmcp changes
/// this framing shape, the assertion below catches it.
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
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();

    let server_cancel = cancel.clone();
    let server_task = tokio::spawn(async move {
        serve_http(
            fixture_cli(),
            brontes::Config::default(),
            addr,
            server_cancel,
            vec![],
        )
        .await
        .expect("serve_http");
    });

    // Wait briefly for the listener to bind. Polling a successful TCP
    // connect is more reliable than a fixed sleep.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

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
async fn http_cancellation_tears_down_within_grace_window() {
    // No client traffic; just verify the bare accept loop respects the
    // cancellation token within the 5-second SHUTDOWN_GRACE.
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();

    let server_cancel = cancel.clone();
    let server_task = tokio::spawn(async move {
        serve_http(
            fixture_cli(),
            brontes::Config::default(),
            addr,
            server_cancel,
            vec![],
        )
        .await
        .expect("serve_http");
    });

    // Wait for the bind to come up so we know we're testing a running
    // server (not a serve_http that errored at bind time).
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(6), server_task).await;
    assert!(
        joined.is_ok(),
        "server did not exit within 6s of cancellation"
    );
}
