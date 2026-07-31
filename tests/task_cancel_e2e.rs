//! `tasks/cancel` (SEP-2663) killing a real child process.
//!
//! Every other test of cancellation stops at brontes' own
//! [`tokio_util::sync::CancellationToken`]. That leaves the hop that actually
//! matters unproven, and it is the one that fails silently: a detached
//! `release` that the client cancelled reports `cancelled`, the client moves
//! on, and the process keeps running — publishing, tagging, uploading — with
//! nobody watching it.
//!
//! This test closes it the same way the trace-context end-to-end test does, by
//! being both halves at once. brontes resolves the tool binary with
//! [`std::env::current_exe`], which inside an integration test is this very
//! binary, so `harness = false` lets `main` dispatch: given the subcommand name
//! it behaves as the wrapped CLI and ticks a file, otherwise it runs the MCP
//! server and asserts on what the child managed to write.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use brontes::__test_internal::BrontesServer;
use brontes::{Config, TaskMode};
use clap::{Arg, Command};
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CancelTaskParams, ClientCapabilities, ClientInfo,
    GetTaskParams, Implementation, ProtocolVersion, TaskStatus,
};
use rmcp::service::{RoleClient, RunningService};
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

/// The wrapped CLI's one subcommand, and therefore also this binary's
/// child-mode marker.
const HEARTBEAT_SUBCOMMAND: &str = "heartbeat";
/// Gap between the child's heartbeats. Short enough that "the child is still
/// running" is observable within a fraction of a second.
const TICK: Duration = Duration::from_millis(50);
/// What the child appends once its full run is over.
const FINISHED: &str = "finished";

/// Poll budget for a task to reach a status.
const POLL_ATTEMPTS: usize = 200;
const POLL_INTERVAL: Duration = Duration::from_millis(25);

fn main() {
    // Child mode: brontes spawned this binary as the wrapped CLI.
    if std::env::args().nth(1).as_deref() == Some(HEARTBEAT_SUBCOMMAND) {
        run_as_child();
        return;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async {
        cancelling_a_task_kills_the_child_process().await;
        an_uncancelled_task_lets_the_child_run_to_completion().await;
    });

    println!("task_cancel_e2e: 2 checks passed");
}

/// Append one line per tick, then a final marker, so the parent can tell
/// "still running" from "ran to the end" from "stopped partway".
fn run_as_child() {
    let mut args = std::env::args().skip(1);
    let mut file = PathBuf::new();
    let mut ticks = 0_usize;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = PathBuf::from(args.next().expect("--file needs a value")),
            "--ticks" => {
                ticks = args
                    .next()
                    .expect("--ticks needs a value")
                    .parse()
                    .expect("--ticks must be a number");
            }
            _ => {}
        }
    }

    for _ in 0..ticks {
        append(&file, "tick");
        std::thread::sleep(TICK);
    }
    append(&file, FINISHED);
}

fn append(path: &Path, line: &str) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the heartbeat file");
    writeln!(f, "{line}").expect("write a heartbeat");
    f.flush().expect("flush a heartbeat");
}

fn heartbeats(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn fixture_cli() -> Command {
    Command::new("cancel-cli").version("0.0.1").subcommand(
        Command::new(HEARTBEAT_SUBCOMMAND)
            .about("Write a heartbeat line on an interval")
            .arg(Arg::new("file").long("file").help("Heartbeat file path"))
            .arg(Arg::new("ticks").long("ticks").help("How many heartbeats")),
    )
}

#[derive(Clone)]
struct TasksClient;

impl rmcp::handler::client::ClientHandler for TasksClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_tasks().build(),
            Implementation::new("task-cancel-e2e-client", "0.0.1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }
}

async fn connect() -> (
    RunningService<RoleClient, TasksClient>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let (client_io, server_io) = duplex(64 * 1024);
    let cancel = CancellationToken::new();

    let cfg = Config::default()
        .task_mode_for(
            format!("cancel-cli {HEARTBEAT_SUBCOMMAND}"),
            TaskMode::Detached,
        )
        .task_poll_interval(Duration::from_millis(10));

    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), cfg).expect("server construction");
            if let Ok(running) = server.serve_with_ct(server_io, cancel).await {
                let _ = running.waiting().await;
            }
        })
    };

    let client = TasksClient
        .serve_with_ct(client_io, cancel.clone())
        .await
        .expect("client start");

    (client, cancel, server_task)
}

/// Start the heartbeat command as a task and return its id.
async fn start_heartbeat(
    client: &RunningService<RoleClient, TasksClient>,
    file: &Path,
    ticks: usize,
) -> String {
    let arguments = serde_json::json!({
        "flags": {
            "file": file.display().to_string(),
            "ticks": ticks.to_string(),
        }
        // `args` is deliberately omitted: the schema does not require it, and a
        // call that leaves it out has to work over the real transport.
    });
    let params = CallToolRequestParams::new(format!("cancel-cli_{HEARTBEAT_SUBCOMMAND}"))
        .with_arguments(serde_json::from_value(arguments).expect("arguments object"));

    match client
        .call_tool_once(params)
        .await
        .expect("the call must be answered")
    {
        CallToolResponse::Task(create) => create.task.task_id,
        other => panic!("expected a task handle, got {other:?}"),
    }
}

async fn poll_until_terminal(
    client: &RunningService<RoleClient, TasksClient>,
    task_id: &str,
) -> TaskStatus {
    for _ in 0..POLL_ATTEMPTS {
        let info = client
            .peer()
            .get_task(GetTaskParams::new(task_id.to_owned()))
            .await
            .expect("tasks/get must be answered");
        if info.task.status().is_terminal() {
            return info.task.status();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("task never reached a terminal status");
}

/// Wait until the child has written at least `n` heartbeats.
async fn wait_for_heartbeats(path: &Path, n: usize) {
    for _ in 0..POLL_ATTEMPTS {
        if heartbeats(path).len() >= n {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("the child never wrote {n} heartbeat(s); it may not have started");
}

async fn cancelling_a_task_kills_the_child_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("heartbeats.txt");
    let (client, cancel, server_task) = connect().await;

    // 200 ticks is ten seconds of work — far longer than this check takes, so
    // a child that survives the cancel is unmistakable.
    let task_id = start_heartbeat(&client, &file, 200).await;
    wait_for_heartbeats(&file, 2).await;

    client
        .peer()
        .cancel_task(CancelTaskParams::new(task_id.clone()))
        .await
        .expect("tasks/cancel must be acknowledged");

    let status = poll_until_terminal(&client, &task_id).await;
    assert_eq!(
        status,
        TaskStatus::Cancelled,
        "a cancelled command must settle as cancelled, not as a failed call"
    );

    // The load-bearing assertion: a killed process cannot keep writing. Give
    // it several tick intervals to prove it stopped rather than stalled.
    let at_cancel = heartbeats(&file).len();
    tokio::time::sleep(TICK * 8).await;
    let after = heartbeats(&file);

    assert_eq!(
        after.len(),
        at_cancel,
        "the child kept writing after tasks/cancel — the process outlived the \
         task ({at_cancel} heartbeats at cancel, {} after)",
        after.len()
    );
    assert!(
        !after.iter().any(|line| line == FINISHED),
        "the child must not have run to completion: {after:?}"
    );

    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

async fn an_uncancelled_task_lets_the_child_run_to_completion() {
    // The disconfirming direction: without the cancel, this same child writes
    // every heartbeat and its finish marker. Without this check, the assertions
    // above would also pass against a child that never started.
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("heartbeats.txt");
    let (client, cancel, server_task) = connect().await;

    let task_id = start_heartbeat(&client, &file, 4).await;
    let status = poll_until_terminal(&client, &task_id).await;

    assert_eq!(
        status,
        TaskStatus::Completed,
        "an uncancelled command must run to completion"
    );
    let written = heartbeats(&file);
    assert_eq!(
        written.iter().filter(|line| *line == "tick").count(),
        4,
        "every heartbeat must have been written: {written:?}"
    );
    assert!(
        written.iter().any(|line| line == FINISHED),
        "the child must have reached its finish marker: {written:?}"
    );

    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}
