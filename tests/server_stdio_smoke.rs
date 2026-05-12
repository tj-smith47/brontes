//! Stdio MCP smoke test.
//!
//! Drives a real `BrontesServer` over rmcp's transport layer using an
//! in-memory duplex channel (rmcp doesn't require literal `tokio::io::stdin`
//! handles — any `AsyncRead + AsyncWrite` pair works). Sends a `tools/list`
//! request through a client peer and asserts the server returns the walked
//! tool list. Also asserts that calling an unknown tool errors cleanly,
//! and that cancellation tears down both halves without a panic.
//!
//! This is the v0.1.0 acceptance gate for Task #1: a consumer's clap tree
//! becomes a working MCP server. Subprocess `call_tool` execution is
//! covered by the unit tests in `src/exec.rs`; this file proves the
//! protocol wire-up against the actual `BrontesServer`.

use clap::Command;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RoleClient;
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::BrontesServer;

/// Minimal client handler — the smoke test only needs the client peer
/// to drive RPCs, so the handler returns the default info and never gets
/// any callbacks from the server (brontes does not initiate prompts,
/// resource subscriptions, or sampling).
#[derive(Clone)]
struct NoopClient;

impl rmcp::handler::client::ClientHandler for NoopClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        rmcp::model::ClientInfo::default()
    }
}

/// Build a tiny CLI with two leaves so the walker has something to surface.
fn fixture_cli() -> Command {
    Command::new("brontes-smoke")
        .version("0.0.1")
        .subcommand(Command::new("greet").about("Say hi"))
        .subcommand(Command::new("status").about("Show status"))
}

#[tokio::test]
async fn stdio_tools_list_returns_walked_tree() {
    let (client_io, server_io) = duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let cancel = CancellationToken::new();

    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), brontes::Config::default());
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

    // tools/list — the wire test.
    let list_result = client
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list succeeds");
    let names: Vec<String> = list_result
        .tools
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "brontes-smoke_greet"),
        "missing greet tool; got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "brontes-smoke_status"),
        "missing status tool; got {names:?}"
    );

    // Server identity travelled across the boundary.
    let peer_info = client.peer_info().expect("server peer info available");
    assert_eq!(peer_info.server_info.name, "brontes-smoke");
    assert_eq!(peer_info.server_info.version, "0.0.1");

    // Clean shutdown: cancel both halves, await the server task.
    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

#[tokio::test]
async fn stdio_call_tool_unknown_name_is_error() {
    let (client_io, server_io) = duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let cancel = CancellationToken::new();

    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), brontes::Config::default());
            let running = server
                .serve_with_ct((server_read, server_write), cancel)
                .await
                .expect("server start");
            let _ = running.waiting().await;
        })
    };

    let client: rmcp::service::RunningService<RoleClient, NoopClient> = NoopClient
        .serve_with_ct((client_read, client_write), cancel.clone())
        .await
        .expect("client start");

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("does-not-exist"))
        .await;
    assert!(result.is_err(), "calling unknown tool must error");

    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

#[tokio::test]
async fn cancellation_token_terminates_server_loop() {
    // Validate the SIGTERM-equivalent shutdown path: cancelling the token
    // tears down the server task within a bounded window.
    let (client_io, server_io) = duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let cancel = CancellationToken::new();
    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), brontes::Config::default());
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
    let _ = client.peer().list_tools(None).await;

    cancel.cancel();
    let _ = client.cancel().await;

    let joined = tokio::time::timeout(std::time::Duration::from_secs(2), server_task).await;
    assert!(
        joined.is_ok(),
        "server task did not exit within 2s of cancellation"
    );
}
