//! The Tasks extension (SEP-2663, `io.modelcontextprotocol/tasks`).
//!
//! A brontes tool is a subprocess, and the commands worth wrapping — a
//! release, a build, a deploy — outlive the patience of a blocking
//! `tools/call`. Detaching one hands the client a handle it can poll, answer,
//! and cancel, so these tests cover the whole handle lifecycle against a real
//! client peer:
//!
//! - a detached command returns a handle and settles with the command's result;
//! - the mode and the client's capability both gate detaching, in both
//!   directions;
//! - an input request raised inside a task is answered by `tasks/update` and
//!   the chain re-enters with the answer;
//! - `tasks/cancel` reaches the running command's cancellation token;
//! - a middleware that asks forever is stopped rather than left spinning.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use clap::Command;
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ClientCapabilities,
    ClientInfo, ElicitRequestParams, ElicitationSchema, GetTaskParams, Implementation,
    InputRequest, InputRequests, InputRequiredResult, InputResponses, ProtocolVersion, TaskPayload,
    TaskStatus, UpdateTaskParams,
};
use rmcp::service::{RoleClient, RunningService};
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::BrontesServer;
use brontes::{
    BoxedNext, Config, Middleware, MiddlewareCtx, MiddlewareOutcome, Selector, TaskMode, ToolOutput,
};

/// The one command this fixture exposes, as a clap path and as an MCP name.
const DEPLOY_PATH: &str = "task-cli deploy";
const DEPLOY_TOOL: &str = "task-cli_deploy";
/// Key the confirming middleware assigns to its input request.
const CONFIRM_KEY: &str = "confirm";
/// Opaque state the middleware round-trips through `tasks/update`.
const STATE: &str = "brontes-task-state";
/// Poll budget for a task to reach the status a test is waiting for. Generous
/// because a loaded CI box schedules the task's own tokio task late, not
/// because any step is slow.
const POLL_ATTEMPTS: usize = 200;
const POLL_INTERVAL: Duration = Duration::from_millis(25);

fn fixture_cli() -> Command {
    Command::new("task-cli")
        .version("0.0.1")
        .subcommand(Command::new("deploy").about("Deploy something"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Client peers
// ─────────────────────────────────────────────────────────────────────────────

/// A `2026-07-28` client that declares the tasks extension and elicitation.
#[derive(Clone)]
struct TasksClient;

impl rmcp::handler::client::ClientHandler for TasksClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder()
                .enable_tasks()
                .enable_elicitation()
                .build(),
            Implementation::new("tasks-test-client", "0.0.1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }
}

/// A `2026-07-28` client that never declared the tasks extension. Everything
/// else about it matches [`TasksClient`], so a difference in behavior isolates
/// the capability gate.
#[derive(Clone)]
struct PlainClient;

impl rmcp::handler::client::ClientHandler for PlainClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_elicitation().build(),
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
    RunningService<RoleClient, C>,
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
    client: RunningService<RoleClient, C>,
    cancel: CancellationToken,
    server_task: tokio::task::JoinHandle<()>,
) where
    C: rmcp::handler::client::ClientHandler,
{
    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

/// Config that detaches the fixture's one command and installs `mw`.
fn detached_cfg(mw: Middleware) -> Config {
    Config::default()
        .task_mode_for(DEPLOY_PATH, TaskMode::Detached)
        .task_poll_interval(Duration::from_millis(10))
        .selector(Selector {
            middleware: Some(mw),
            ..Default::default()
        })
}

/// Call the tool and insist the server answered with a task handle.
async fn call_expecting_task<C>(client: &RunningService<RoleClient, C>) -> String
where
    C: rmcp::handler::client::ClientHandler,
{
    let response = client
        .call_tool_once(CallToolRequestParams::new(DEPLOY_TOOL))
        .await
        .expect("the call must be answered");

    match response {
        CallToolResponse::Task(create) => create.task.task_id,
        other => panic!("expected a task handle, got {other:?}"),
    }
}

/// Poll `tasks/get` until `wanted` is reached, returning the payload seen then.
async fn poll_until<C>(
    client: &RunningService<RoleClient, C>,
    task_id: &str,
    wanted: TaskStatus,
) -> TaskPayload
where
    C: rmcp::handler::client::ClientHandler,
{
    let mut last = None;
    for _ in 0..POLL_ATTEMPTS {
        let info = client
            .peer()
            .get_task(GetTaskParams::new(task_id.to_owned()))
            .await
            .expect("tasks/get must be answered");
        if info.task.status() == wanted {
            return info.task.payload;
        }
        last = Some(info.task.status());
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("task never reached {wanted:?}; last status was {last:?}");
}

/// The `CallToolResult` a completed task settled with.
fn completed_result(payload: TaskPayload) -> CallToolResult {
    match payload {
        TaskPayload::Completed { result } => {
            serde_json::from_value(serde_json::Value::Object(result))
                .expect("a completed task must carry a CallToolResult")
        }
        other => panic!("expected a completed payload, got {other:?}"),
    }
}

fn joined_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Middleware fixtures
// ─────────────────────────────────────────────────────────────────────────────

/// Completes immediately without shelling out.
fn finishing_middleware(text: &'static str) -> Middleware {
    Arc::new(move |_ctx: MiddlewareCtx, _next: BoxedNext| {
        Box::pin(async move {
            Ok(ToolOutput {
                stdout: text.to_owned(),
                stderr: String::new(),
                exit_code: 0,
            }
            .into())
        })
    })
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

/// What the middleware saw across the rounds of one detached call.
#[derive(Debug, Default, Clone)]
struct RoundObservation {
    rounds: usize,
    retry_state: Option<String>,
    retry_answer: Option<serde_json::Value>,
    cancelled: bool,
}

/// Asks for confirmation on the first round and completes on the second,
/// recording what the second round received.
fn confirming_middleware(seen: Arc<Mutex<RoundObservation>>) -> Middleware {
    Arc::new(move |ctx: MiddlewareCtx, _next: BoxedNext| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.lock().unwrap().rounds += 1;

            let Some(answers) = ctx.input_responses.clone() else {
                return Ok(MiddlewareOutcome::InputRequired(Box::new(
                    confirmation_request(),
                )));
            };

            {
                let mut s = seen.lock().unwrap();
                s.retry_state.clone_from(&ctx.request_state);
                s.retry_answer = answers.get(CONFIRM_KEY).cloned();
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

/// Never completes on its own: waits for the cancellation token brontes hands
/// the exec step, which is the same token `tasks/cancel` must reach.
fn cancellable_middleware(seen: Arc<Mutex<RoundObservation>>) -> Middleware {
    Arc::new(move |ctx: MiddlewareCtx, _next: BoxedNext| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.lock().unwrap().rounds += 1;
            ctx.cancellation_token.cancelled().await;
            seen.lock().unwrap().cancelled = true;
            Err(brontes::Error::Config("cancelled by the client".into()))
        })
    })
}

/// Asks for input on every round and never completes.
fn insatiable_middleware(seen: Arc<Mutex<RoundObservation>>) -> Middleware {
    Arc::new(move |_ctx: MiddlewareCtx, _next: BoxedNext| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            let round = {
                let mut s = seen.lock().unwrap();
                s.rounds += 1;
                s.rounds
            };
            // Keys must be unique over the lifetime of a task, so each round
            // asks under its own.
            let mut requests = InputRequests::new();
            requests.insert(
                format!("{CONFIRM_KEY}-{round}"),
                InputRequest::Elicitation(rmcp::model::ElicitRequest::new(
                    ElicitRequestParams::FormElicitationParams {
                        meta: None,
                        message: format!("Round {round}?"),
                        requested_schema: ElicitationSchema::new(std::collections::BTreeMap::new()),
                    },
                )),
            );
            Ok(MiddlewareOutcome::InputRequired(Box::new(
                InputRequiredResult::new(Some(requests), None),
            )))
        })
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_detached_command_hands_back_a_handle_and_settles_with_its_result() {
    let (client, cancel, server_task) = connect(
        TasksClient,
        detached_cfg(finishing_middleware("deployed\n")),
    )
    .await;

    let task_id = call_expecting_task(&client).await;
    let payload = poll_until(&client, &task_id, TaskStatus::Completed).await;
    let result = completed_result(payload);

    assert_eq!(
        result.is_error,
        Some(false),
        "the command succeeded, so the settled result must not be an error"
    );
    assert!(
        joined_text(&result).contains("deployed"),
        "the task must settle with the command's own output, got {:?}",
        joined_text(&result)
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn a_client_without_the_tasks_capability_gets_the_blocking_result() {
    // The disconfirming direction for detaching: same config, a client that
    // never declared the extension. A handle here would be unparseable.
    let (client, cancel, server_task) = connect(
        PlainClient,
        detached_cfg(finishing_middleware("deployed inline\n")),
    )
    .await;

    let response = client
        .call_tool_once(CallToolRequestParams::new(DEPLOY_TOOL))
        .await
        .expect("the call must be answered");

    match response {
        CallToolResponse::Complete(result) => assert!(
            joined_text(&result).contains("deployed inline"),
            "the blocking path must still run the command"
        ),
        other => panic!("expected a blocking result, got {other:?}"),
    }

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn a_blocking_command_stays_blocking_for_a_tasks_capable_client() {
    // The other half of the gate: capability alone must not detach anything.
    let cfg = Config::default().selector(Selector {
        middleware: Some(finishing_middleware("deployed inline\n")),
        ..Default::default()
    });
    let (client, cancel, server_task) = connect(TasksClient, cfg).await;

    let response = client
        .call_tool_once(CallToolRequestParams::new(DEPLOY_TOOL))
        .await
        .expect("the call must be answered");

    assert!(
        matches!(response, CallToolResponse::Complete(_)),
        "TaskMode::Blocking is the default and must not detach, got {response:?}"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn the_tasks_capability_is_advertised_only_when_a_command_is_detached() {
    let (client, cancel, server_task) =
        connect(TasksClient, detached_cfg(finishing_middleware("ok\n"))).await;
    assert!(
        client
            .peer_info()
            .is_some_and(|info| info.capabilities.supports_tasks()),
        "a server with a detached command must advertise the tasks extension"
    );
    shutdown(client, cancel, server_task).await;

    let cfg = Config::default().selector(Selector {
        middleware: Some(finishing_middleware("ok\n")),
        ..Default::default()
    });
    let (client, cancel, server_task) = connect(TasksClient, cfg).await;
    assert!(
        !client
            .peer_info()
            .is_some_and(|info| info.capabilities.supports_tasks()),
        "a server that detaches nothing must not advertise an extension whose \
         methods would answer with 'no such task' forever"
    );
    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn an_input_request_raised_inside_a_task_is_answered_by_tasks_update() {
    let seen = Arc::new(Mutex::new(RoundObservation::default()));
    let (client, cancel, server_task) = connect(
        TasksClient,
        detached_cfg(confirming_middleware(Arc::clone(&seen))),
    )
    .await;

    let task_id = call_expecting_task(&client).await;

    // The question leaves through `tasks/get` rather than the call response.
    let payload = poll_until(&client, &task_id, TaskStatus::InputRequired).await;
    let TaskPayload::InputRequired { input_requests } = payload else {
        panic!("expected an input_required payload");
    };
    assert!(
        input_requests.contains_key(CONFIRM_KEY),
        "the middleware's own key must survive to the client, got {:?}",
        input_requests.keys().collect::<Vec<_>>()
    );

    let mut answers = InputResponses::new();
    answers.insert(
        CONFIRM_KEY.to_owned(),
        serde_json::json!({ "action": "accept", "content": { "proceed": true } }),
    );
    client
        .peer()
        .update_task(UpdateTaskParams::new(task_id.clone(), answers))
        .await
        .expect("tasks/update must be accepted");

    let result = completed_result(poll_until(&client, &task_id, TaskStatus::Completed).await);
    assert!(
        joined_text(&result).contains("deployed after confirmation"),
        "the second round must produce the command's result"
    );

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.rounds, 2,
        "an answered input request re-enters the chain from the top exactly once"
    );
    assert_eq!(
        seen.retry_state.as_deref(),
        Some(STATE),
        "the middleware's own requestState must come back on the retry, as it \
         does for a blocking call"
    );
    assert_eq!(
        seen.retry_answer
            .as_ref()
            .and_then(|a| a.get("content"))
            .and_then(|c| c.get("proceed")),
        Some(&serde_json::json!(true)),
        "the client's answer must reach the middleware verbatim"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn cancelling_a_task_reaches_the_running_command() {
    let seen = Arc::new(Mutex::new(RoundObservation::default()));
    let (client, cancel, server_task) = connect(
        TasksClient,
        detached_cfg(cancellable_middleware(Arc::clone(&seen))),
    )
    .await;

    let task_id = call_expecting_task(&client).await;

    // Wait for the command to actually be running, so the cancel cannot be
    // answered by a race that never started the work.
    for _ in 0..POLL_ATTEMPTS {
        if seen.lock().unwrap().rounds > 0 {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        seen.lock().unwrap().rounds,
        1,
        "the command must be running before the cancel is sent"
    );

    client
        .peer()
        .cancel_task(CancelTaskParams::new(task_id.clone()))
        .await
        .expect("tasks/cancel must be acknowledged");

    let payload = poll_until(&client, &task_id, TaskStatus::Cancelled).await;
    assert_eq!(payload, TaskPayload::Cancelled);
    assert!(
        seen.lock().unwrap().cancelled,
        "tasks/cancel must reach the cancellation token the exec step waits on \
         — without that the subprocess outlives the task"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn a_middleware_that_asks_forever_is_stopped_rather_than_left_spinning() {
    let seen = Arc::new(Mutex::new(RoundObservation::default()));
    let (client, cancel, server_task) = connect(
        TasksClient,
        detached_cfg(insatiable_middleware(Arc::clone(&seen))),
    )
    .await;

    let task_id = call_expecting_task(&client).await;

    // Answer every question it asks. A missing round cap shows up here as a
    // test that never terminates rather than one that fails, so the loop is
    // bounded well above the cap.
    let mut answered = 0;
    for _ in 0..64 {
        let info = client
            .peer()
            .get_task(GetTaskParams::new(task_id.clone()))
            .await
            .expect("tasks/get must be answered");
        match info.task.payload {
            TaskPayload::InputRequired { input_requests } => {
                let mut answers = InputResponses::new();
                for key in input_requests.keys() {
                    answers.insert(key.clone(), serde_json::json!({ "action": "accept" }));
                }
                client
                    .peer()
                    .update_task(UpdateTaskParams::new(task_id.clone(), answers))
                    .await
                    .expect("tasks/update must be accepted");
                answered += 1;
            }
            TaskPayload::Working => tokio::time::sleep(POLL_INTERVAL).await,
            _ => break,
        }
    }

    let result = completed_result(poll_until(&client, &task_id, TaskStatus::Completed).await);
    assert_eq!(
        result.is_error,
        Some(true),
        "giving up must be reported as a failed call, not a successful one"
    );
    assert_eq!(
        seen.lock().unwrap().rounds,
        16,
        "the chain must re-enter exactly MAX_TASK_INPUT_ROUNDS times before \
         giving up — fewer means the loop stopped for some other reason"
    );
    assert_eq!(
        answered, 16,
        "every round's question must have been answered, so the cap is what \
         ended the task rather than an unanswered request"
    );

    shutdown(client, cancel, server_task).await;
}
