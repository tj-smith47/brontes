//! Multi Round-Trip Requests (SEP-2322) through the middleware boundary.
//!
//! Under the stateless `2026-07-28` revision there is no server-initiated
//! request channel: a server that needs something from its client must answer
//! `tools/call` with an `InputRequiredResult` and be re-called. That makes MRTR
//! the *only* way a brontes middleware can reach the client, so these tests
//! cover the full round trip against a real client peer rather than asserting
//! on the types alone:
//!
//! - a middleware asks for input, the client answers, the retry completes;
//! - the retry carries the client's answers and the echoed `requestState`;
//! - a peer that cannot parse the result gets a tool error instead, on both
//!   the protocol-version and the elicitation-capability paths.

use std::sync::Arc;
use std::sync::Mutex;

use clap::Command;
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientInfo, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationSchema, Implementation, InputRequest, InputRequests,
    InputRequiredResult, ProtocolVersion,
};
use rmcp::service::RoleClient;
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::BrontesServer;
use brontes::{
    BoxedNext, Config, Middleware, MiddlewareCtx, MiddlewareOutcome, Selector, ToolOutput,
};

/// Key the middleware assigns to its one input request.
const CONFIRM_KEY: &str = "confirm";
/// Opaque state the middleware round-trips through the client.
const STATE: &str = "brontes-test-state";

fn fixture_cli() -> Command {
    Command::new("mrtr-cli")
        .version("0.0.1")
        .subcommand(Command::new("deploy").about("Deploy something"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Client peers
// ─────────────────────────────────────────────────────────────────────────────

/// A `2026-07-28` client that declares elicitation and answers every request.
///
/// `RunningService::call_tool` drives the MRTR rounds itself: it routes each
/// `InputRequest` to this handler and re-sends the original call with the
/// answers, so the test exercises the real client half rather than a stub.
/// (`Peer::call_tool` is the single-round form and rejects an
/// `InputRequiredResult` outright.)
#[derive(Clone)]
struct ElicitingClient {
    /// Records that the client was actually asked, and what for.
    asked: Arc<Mutex<Vec<String>>>,
}

impl rmcp::handler::client::ClientHandler for ElicitingClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_elicitation().build(),
            Implementation::new("mrtr-test-client", "0.0.1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: rmcp::service::RequestContext<RoleClient>,
    ) -> Result<ElicitResult, rmcp::ErrorData> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request {
            self.asked.lock().unwrap().push(message.clone());
        }
        Ok(ElicitResult::new(ElicitationAction::Accept)
            .with_content(serde_json::json!({ "proceed": true })))
    }
}

/// A client on the previous revision. It declares elicitation, so a refusal
/// isolates the protocol-version gate rather than the capability gate.
#[derive(Clone)]
struct LegacyClient;

impl rmcp::handler::client::ClientHandler for LegacyClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_elicitation().build(),
            Implementation::new("legacy-test-client", "0.0.1"),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }
}

/// A `2026-07-28` client that never declared elicitation, isolating the
/// capability gate from the version gate.
#[derive(Clone)]
struct NoElicitationClient;

impl rmcp::handler::client::ClientHandler for NoElicitationClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("plain-test-client", "0.0.1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Harness
// ─────────────────────────────────────────────────────────────────────────────

async fn connect<C>(
    client: C,
    cfg: Config,
) -> (
    rmcp::service::RunningService<RoleClient, C>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
)
where
    C: rmcp::handler::client::ClientHandler,
{
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

    let client = client
        .serve_with_ct(client_io, cancel.clone())
        .await
        .expect("client start");

    (client, cancel, server_task)
}

async fn shutdown<C>(
    client: rmcp::service::RunningService<RoleClient, C>,
    cancel: CancellationToken,
    server_task: tokio::task::JoinHandle<()>,
) where
    C: rmcp::handler::client::ClientHandler,
{
    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

/// One elicitation request plus the opaque state to echo back.
fn confirmation_request() -> InputRequiredResult {
    let mut requests = InputRequests::new();
    requests.insert(
        CONFIRM_KEY.to_owned(),
        InputRequest::Elicitation(rmcp::model::ElicitRequest::new(
            ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: "Confirm the deploy?".to_owned(),
                requested_schema: ElicitationSchema::new(std::collections::BTreeMap::new()),
            },
        )),
    );
    InputRequiredResult::new(Some(requests), Some(STATE.to_owned()))
}

/// What the middleware saw on the retry, if a retry happened at all.
#[derive(Debug, Default, Clone)]
struct RetryObservation {
    rounds: usize,
    retry_state: Option<String>,
    retry_answer: Option<serde_json::Value>,
    exec_runs: usize,
}

/// Middleware that asks for confirmation on the first attempt and completes on
/// the retry, without ever shelling out. Records what each round observed.
fn confirming_middleware(seen: Arc<Mutex<RetryObservation>>) -> Middleware {
    Arc::new(move |ctx: MiddlewareCtx, _next: BoxedNext| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            {
                let mut s = seen.lock().unwrap();
                s.rounds += 1;
            }

            // `input_responses` is the only signal distinguishing a retry from
            // a first attempt — MCP re-sends the whole call rather than
            // resuming one.
            let Some(answers) = ctx.input_responses.clone() else {
                return Ok(MiddlewareOutcome::InputRequired(Box::new(
                    confirmation_request(),
                )));
            };

            {
                let mut s = seen.lock().unwrap();
                s.retry_state.clone_from(&ctx.request_state);
                s.retry_answer = answers.get(CONFIRM_KEY).cloned();
                s.exec_runs += 1;
            }

            Ok(ToolOutput {
                stdout: "deployed after confirmation\n".into(),
                stderr: String::new(),
                exit_code: 0,
            }
            .into())
        })
    })
}

fn cfg_with(mw: Middleware) -> Config {
    Config::default().selector(Selector {
        middleware: Some(mw),
        ..Default::default()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_middleware_can_ask_the_client_for_input_and_finish_on_the_retry() {
    let seen = Arc::new(Mutex::new(RetryObservation::default()));
    let asked = Arc::new(Mutex::new(Vec::new()));

    let (client, cancel, server_task) = connect(
        ElicitingClient {
            asked: Arc::clone(&asked),
        },
        cfg_with(confirming_middleware(Arc::clone(&seen))),
    )
    .await;

    let result = client
        .call_tool(CallToolRequestParams::new("mrtr-cli_deploy"))
        .await
        .expect("the MRTR round trip must complete");

    assert_eq!(result.is_error, Some(false), "the retry must succeed");
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect();
    assert!(
        text.contains("deployed after confirmation"),
        "the completed result must come from the retry: {text}"
    );

    // The client really was asked, with the middleware's message.
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked, vec!["Confirm the deploy?".to_string()]);

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.rounds, 2, "exactly one ask and one retry: {seen:?}");
    assert_eq!(
        seen.retry_state.as_deref(),
        Some(STATE),
        "the client must echo requestState back verbatim"
    );
    assert!(
        seen.retry_answer.is_some(),
        "the retry must carry the client's answer under the middleware's key: {seen:?}"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn the_first_attempt_alone_never_runs_the_command() {
    // The point of asking before acting: nothing must execute until the answer
    // is in. Proven by the exec counter, which only the retry branch bumps.
    let seen = Arc::new(Mutex::new(RetryObservation::default()));
    let (client, cancel, server_task) = connect(
        ElicitingClient {
            asked: Arc::new(Mutex::new(Vec::new())),
        },
        cfg_with(confirming_middleware(Arc::clone(&seen))),
    )
    .await;

    let _ = client
        .call_tool(CallToolRequestParams::new("mrtr-cli_deploy"))
        .await
        .expect("round trip");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.exec_runs, 1,
        "the command must run once, on the retry only: {seen:?}"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn a_legacy_peer_gets_a_tool_error_rather_than_a_protocol_error() {
    // rmcp turns an InputRequiredResult bound for a pre-2026 peer into a
    // JSON-RPC -32600, which would break brontes' invariant that a tool call
    // always answers with a result. brontes must catch it first.
    let seen = Arc::new(Mutex::new(RetryObservation::default()));
    let (client, cancel, server_task) = connect(
        LegacyClient,
        cfg_with(confirming_middleware(Arc::clone(&seen))),
    )
    .await;

    let result = client
        .call_tool(CallToolRequestParams::new("mrtr-cli_deploy"))
        .await
        .expect("must answer with a result, not a JSON-RPC error");

    assert_eq!(result.is_error, Some(true));
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect();
    assert!(
        text.contains("2026-07-28"),
        "the error must explain the version requirement: {text}"
    );

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.rounds, 1, "there must be no retry");
    assert_eq!(seen.exec_runs, 0, "and the command must never run");

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn a_client_without_the_elicitation_capability_gets_a_tool_error() {
    // rmcp capability-checks the Tasks extension but not MRTR, so without this
    // guard the elicitation would reach a client with no way to answer it.
    let seen = Arc::new(Mutex::new(RetryObservation::default()));
    let (client, cancel, server_task) = connect(
        NoElicitationClient,
        cfg_with(confirming_middleware(Arc::clone(&seen))),
    )
    .await;

    let result = client
        .call_tool(CallToolRequestParams::new("mrtr-cli_deploy"))
        .await
        .expect("must answer with a result");

    assert_eq!(result.is_error, Some(true));
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect();
    assert!(
        text.contains("elicitation capability"),
        "the error must name the missing capability: {text}"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn a_middleware_that_never_asks_still_completes_normally() {
    // Disconfirming direction for the three tests above: with the same client
    // and harness, a plain middleware must produce an ordinary result, so a
    // failure there cannot be blamed on the transport or the fixture.
    let mw: Middleware = Arc::new(|_ctx: MiddlewareCtx, _next: BoxedNext| {
        Box::pin(async move {
            Ok(ToolOutput {
                stdout: "no confirmation needed\n".into(),
                stderr: String::new(),
                exit_code: 0,
            }
            .into())
        })
    });

    let (client, cancel, server_task) = connect(
        ElicitingClient {
            asked: Arc::new(Mutex::new(Vec::new())),
        },
        cfg_with(mw),
    )
    .await;

    let result = client
        .call_tool(CallToolRequestParams::new("mrtr-cli_deploy"))
        .await
        .expect("plain call");
    assert_eq!(result.is_error, Some(false));

    shutdown(client, cancel, server_task).await;
}
