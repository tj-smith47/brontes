//! W3C Trace Context (SEP-414) reaching a real child process.
//!
//! Every other test of this path stops at the [`tokio::process::Command`]
//! brontes hands the OS. That leaves the last hop — an actual `fork`/`exec`
//! with an inherited environment — unproven, which is exactly where a wiring
//! mistake would be silent: the tool call still succeeds, the trace just never
//! joins the caller's span, and nothing fails.
//!
//! This test closes it by being both halves at once. brontes resolves the tool
//! binary with [`std::env::current_exe`], which inside an integration test is
//! this very binary, so running with `harness = false` lets `main` dispatch:
//! given the subcommand name it behaves as the wrapped CLI and dumps its
//! environment; otherwise it runs the MCP server and asserts on what the child
//! printed.

use std::collections::BTreeMap;

use brontes::__test_internal::BrontesServer;
use brontes::Config;
use clap::Command;
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, Implementation,
    ProtocolVersion, RequestMetaObject,
};
use rmcp::service::RoleClient;
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

/// The wrapped CLI's one subcommand, and therefore also this binary's
/// child-mode marker.
const ECHO_SUBCOMMAND: &str = "echo-env";

/// Variables the child prints, one `NAME=value` line each.
const REPORTED_VARS: &[&str] = &["TRACEPARENT", "TRACESTATE", "BAGGAGE", "DEPLOY_ENVIRONMENT"];

/// A real traceparent: sampled, version `00`, non-zero ids.
const TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";
const TRACESTATE: &str = "vendorA=t61rcWkgMzE,vendorB=00f067aa0ba902b7";
const BAGGAGE: &str = "userId=alice,serverNode=DF%2028";

fn main() {
    // Child mode: brontes spawned this binary as the wrapped CLI.
    if std::env::args().nth(1).as_deref() == Some(ECHO_SUBCOMMAND) {
        for name in REPORTED_VARS {
            let value = std::env::var(name).unwrap_or_default();
            println!("{name}={value}");
        }
        return;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async {
        a_traced_call_reaches_the_child_with_all_three_headers().await;
        an_untraced_call_leaves_the_child_untraced().await;
        propagation_can_be_turned_off_without_touching_default_env().await;
    });

    println!("trace_propagation_e2e: 3 checks passed");
}

fn fixture_cli() -> Command {
    Command::new("trace-cli")
        .version("0.0.1")
        .subcommand(Command::new(ECHO_SUBCOMMAND).about("Print the inherited trace environment"))
}

#[derive(Clone)]
struct TracingClient;

impl rmcp::handler::client::ClientHandler for TracingClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("trace-e2e-client", "0.0.1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }
}

async fn connect(
    cfg: Config,
) -> (
    rmcp::service::RunningService<RoleClient, TracingClient>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let (client_io, server_io) = duplex(64 * 1024);
    let cancel = CancellationToken::new();

    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), cfg).expect("server construction");
            if let Ok(running) = server.serve_with_ct(server_io, cancel).await {
                let _ = running.waiting().await;
            }
        })
    };

    let client = TracingClient
        .serve_with_ct(client_io, cancel.clone())
        .await
        .expect("client start");

    (client, cancel, server_task)
}

/// `_meta` carrying the stateless revision's identity fields, optionally with
/// trace context attached.
fn call_meta(traced: bool) -> RequestMetaObject {
    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
    meta.set_client_info(Implementation::new("trace-e2e-client", "0.0.1"));
    meta.set_client_capabilities(ClientCapabilities::default());
    if traced {
        meta.set_traceparent(TRACEPARENT);
        meta.set_tracestate(TRACESTATE);
        meta.set_baggage(BAGGAGE);
    }
    meta
}

/// Call the echo tool and parse the child's `NAME=value` lines.
async fn child_env(cfg: Config, traced: bool) -> BTreeMap<String, String> {
    let (client, cancel, server_task) = connect(cfg).await;

    let mut params = CallToolRequestParams::new(format!("trace-cli_{ECHO_SUBCOMMAND}"));
    params.meta = Some(call_meta(traced));
    let result: CallToolResult = client.call_tool(params).await.expect("tool call");

    assert_eq!(
        result.is_error,
        Some(false),
        "the child must exit cleanly: {result:?}"
    );

    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect();

    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;

    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

async fn a_traced_call_reaches_the_child_with_all_three_headers() {
    let env = child_env(Config::default(), true).await;

    assert_eq!(
        env.get("TRACEPARENT").map(String::as_str),
        Some(TRACEPARENT),
        "the sampled traceparent must reach the child verbatim: {env:?}"
    );
    assert_eq!(
        env.get("TRACESTATE").map(String::as_str),
        Some(TRACESTATE),
        "vendor state travels with the traceparent: {env:?}"
    );
    assert_eq!(
        env.get("BAGGAGE").map(String::as_str),
        Some(BAGGAGE),
        "baggage is propagated independently of tracestate: {env:?}"
    );
}

async fn an_untraced_call_leaves_the_child_untraced() {
    // The disconfirming direction: with the same binary, same tool, and same
    // transport, an untraced call must leave the child with nothing. Without
    // this, the assertions above would also pass if the values leaked in from
    // the test runner's own environment.
    let env = child_env(Config::default(), false).await;

    for name in ["TRACEPARENT", "TRACESTATE", "BAGGAGE"] {
        assert_eq!(
            env.get(name).map(String::as_str),
            Some(""),
            "an untraced request must not fabricate {name}: {env:?}"
        );
    }
}

async fn propagation_can_be_turned_off_without_touching_default_env() {
    // Opting out is a real setting, not a documented intention, and it must be
    // narrow: `default_env` entries still reach the child.
    let cfg = Config::default()
        .propagate_trace_context(false)
        .default_env("DEPLOY_ENVIRONMENT", "staging");
    let env = child_env(cfg, true).await;

    assert_eq!(
        env.get("TRACEPARENT").map(String::as_str),
        Some(""),
        "propagate_trace_context(false) must stop the traceparent at the boundary: {env:?}"
    );
    assert_eq!(
        env.get("DEPLOY_ENVIRONMENT").map(String::as_str),
        Some("staging"),
        "opting out of tracing must not disturb default_env: {env:?}"
    );
}
