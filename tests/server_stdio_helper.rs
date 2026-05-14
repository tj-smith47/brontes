//! Drives the production `serve_stdio_with` helper end-to-end with an
//! in-memory duplex transport.
//!
//! `tests/server_stdio_smoke.rs` already exercises `BrontesServer` over
//! a duplex pair, but it bypasses `serve_stdio_with` (the function that
//! wraps `BrontesServer::new` + `serve_with_ct` + `waiting()` join-error
//! handling). This test closes that gap: it calls the real
//! `serve_stdio_with` through the `__test_internal` re-export, so the
//! function body — including the `BrontesServer::new` propagation and
//! the `Error::Panic` mapping on join-error — runs against actual
//! traffic from a real client peer.

use clap::Command;
use rmcp::ServiceExt;
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::serve_stdio_with;

#[derive(Clone)]
struct NoopClient;

impl rmcp::handler::client::ClientHandler for NoopClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        rmcp::model::ClientInfo::default()
    }
}

fn fixture_cli() -> Command {
    Command::new("stdio-helper-cli")
        .version("0.0.1")
        .subcommand(Command::new("hello"))
}

#[tokio::test]
async fn serve_stdio_with_responds_to_tools_list_then_exits_on_cancel() {
    let (client_io, server_io) = duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let cancel = CancellationToken::new();

    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            serve_stdio_with(
                fixture_cli(),
                brontes::Config::default(),
                (server_read, server_write),
                cancel,
            )
            .await
        })
    };

    let client = NoopClient
        .serve_with_ct((client_read, client_write), cancel.clone())
        .await
        .expect("client start");

    let list = client.peer().list_tools(None).await.expect("tools/list");
    let names: Vec<String> = list.tools.iter().map(|t| t.name.to_string()).collect();
    assert!(
        names.iter().any(|n| n == "stdio-helper-cli_hello"),
        "missing hello tool; got {names:?}"
    );

    let _ = client.cancel().await;
    cancel.cancel();

    // serve_stdio_with must return Ok after cancellation. A panic in the
    // service task would surface as Error::Panic (the mapping line we're
    // pinning); a clean cancel yields Ok.
    let join = server_task.await.expect("server task joins");
    join.expect("serve_stdio_with returns Ok on cancel");
}

#[tokio::test]
async fn serve_stdio_with_surfaces_config_error_at_startup() {
    // BrontesServer::new fails when Config references a path the walker
    // can't resolve. The error must propagate out of serve_stdio_with
    // BEFORE the transport handshake — the surface that this helper
    // exposes (vs. silently returning a server that fails every tools/
    // call) is what we're pinning.
    let (_client_io, server_io) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_io);
    let cancel = CancellationToken::new();

    let cfg = brontes::Config::default().annotation(
        "stdio-helper-cli nonexistent",
        brontes::ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        },
    );

    let err = serve_stdio_with(fixture_cli(), cfg, (server_read, server_write), cancel)
        .await
        .expect_err("annotation on unknown path must surface");
    assert!(
        matches!(err, brontes::Error::Config(_)),
        "expected Config, got: {err}"
    );
}
