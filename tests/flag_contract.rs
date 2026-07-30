//! The flag surface brontes advertises versus the argv it actually builds.
//!
//! Two contracts live here, both of which used to be claims rather than
//! guarantees:
//!
//! 1. A `clap::ArgAction::Count` flag renders as repetition. clap parses a
//!    `Count` arg with `num_args(0)`, so the `--flag N` form is rejected as an
//!    unexpected argument — the flag would be unusable through MCP.
//! 2. `flags.additionalProperties: false` is enforced. Nothing in the MCP layer
//!    validates tool-call arguments against the advertised input schema, so an
//!    unknown flag would otherwise reach the CLI as an opaque clap usage error.
//!
//! The argv assertions run through `render_tool_argv`, which derives the render
//! kinds from a real walk — the same path `mcp start` takes, minus the spawn.

use clap::{Arg, ArgAction, Command, value_parser};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RoleClient;
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::{BrontesServer, render_tool_argv};
use brontes::{Config, ToolInput};

/// CLI exposing one flag of each render kind plus a plain integer, so the
/// `Count` case is distinguishable from "every integer repeats".
fn fixture_cli() -> Command {
    Command::new("flagcli").version("0.0.1").subcommand(
        Command::new("build")
            .about("Build something")
            .arg(Arg::new("verbose").long("verbose").action(ArgAction::Count))
            .arg(Arg::new("quiet").long("quiet").action(ArgAction::SetTrue))
            .arg(
                Arg::new("jobs")
                    .long("jobs")
                    .value_parser(value_parser!(i64)),
            ),
    )
}

const fn input_with(flags: serde_json::Map<String, serde_json::Value>) -> ToolInput {
    ToolInput {
        flags,
        args: Vec::new(),
    }
}

fn flags(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[test]
fn count_flag_from_the_walk_renders_as_repetition() {
    let argv = render_tool_argv(
        &fixture_cli(),
        &Config::default(),
        "flagcli_build",
        &input_with(flags(&[("verbose", serde_json::json!(3))])),
    )
    .expect("render");

    assert_eq!(
        argv,
        vec!["build", "--verbose", "--verbose", "--verbose"],
        "an ArgAction::Count flag must repeat, never take a value"
    );
}

#[test]
fn the_rendered_count_argv_is_what_clap_actually_accepts() {
    // Closes the loop: feed brontes' own argv back through the same clap
    // command and confirm the parse yields the count the client asked for.
    // Asserting the token shape alone would not prove the CLI accepts it.
    let argv = render_tool_argv(
        &fixture_cli(),
        &Config::default(),
        "flagcli_build",
        &input_with(flags(&[("verbose", serde_json::json!(2))])),
    )
    .expect("render");

    let matches = fixture_cli()
        .no_binary_name(true)
        .try_get_matches_from(&argv)
        .expect("brontes' argv must parse against the CLI it was built from");
    let build = matches
        .subcommand_matches("build")
        .expect("build subcommand");
    assert_eq!(build.get_count("verbose"), 2);
}

#[test]
fn the_pre_fix_count_form_is_the_one_clap_rejects() {
    // Disconfirming direction: the `--verbose N` form brontes used to emit must
    // actually fail, otherwise the test above proves nothing.
    let err = fixture_cli()
        .no_binary_name(true)
        .try_get_matches_from(["build", "--verbose", "2"])
        .expect_err("--verbose N must be rejected for a Count arg");
    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
}

#[test]
fn boolean_and_value_flags_keep_their_forms() {
    let argv = render_tool_argv(
        &fixture_cli(),
        &Config::default(),
        "flagcli_build",
        &input_with(flags(&[
            ("quiet", serde_json::json!(true)),
            ("jobs", serde_json::json!(4)),
        ])),
    )
    .expect("render");

    // `flags` is a serde_json::Map, so iteration is key-sorted: jobs, quiet.
    assert_eq!(argv, vec!["build", "--jobs", "4", "--quiet"]);
}

#[test]
fn a_false_boolean_and_a_zero_count_both_vanish() {
    let argv = render_tool_argv(
        &fixture_cli(),
        &Config::default(),
        "flagcli_build",
        &input_with(flags(&[
            ("quiet", serde_json::json!(false)),
            ("verbose", serde_json::json!(0)),
        ])),
    )
    .expect("render");

    assert_eq!(argv, vec!["build"], "absent states must emit no tokens");
}

// ─────────────────────────────────────────────────────────────────────────────
// additionalProperties: false enforcement, over a real MCP transport
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct NoopClient;

impl rmcp::handler::client::ClientHandler for NoopClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        rmcp::model::ClientInfo::default()
    }
}

async fn spin_up() -> (
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
            let server = BrontesServer::new(fixture_cli(), Config::default()).expect("construct");
            if let Ok(running) = server.serve((server_read, server_write)).await {
                cancel.cancelled().await;
                let _ = running.cancel().await;
            }
        })
    };

    let client = NoopClient
        .serve((client_read, client_write))
        .await
        .expect("client handshake");

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

/// Build `tools/call` arguments carrying `flags` and an empty `args`.
fn call_args(
    flag_pairs: &[(&str, serde_json::Value)],
) -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    args.insert("flags".into(), serde_json::Value::Object(flags(flag_pairs)));
    args.insert("args".into(), serde_json::Value::Array(vec![]));
    args
}

#[tokio::test]
async fn an_undeclared_flag_is_rejected_before_the_cli_ever_runs() {
    let (client, cancel, server_task) = spin_up().await;

    let err = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("flagcli_build")
                .with_arguments(call_args(&[("nonexistent", serde_json::json!("x"))])),
        )
        .await
        .expect_err("an undeclared flag must not reach the CLI");

    let text = err.to_string();
    assert!(
        text.contains("unknown flag(s) for flagcli_build: nonexistent"),
        "error must name the offending flag: {text}"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn several_undeclared_flags_are_reported_together_and_sorted() {
    let (client, cancel, server_task) = spin_up().await;

    let err = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("flagcli_build").with_arguments(call_args(&[
                ("zeta", serde_json::json!(1)),
                ("alpha", serde_json::json!(1)),
                ("quiet", serde_json::json!(true)),
            ])),
        )
        .await
        .expect_err("undeclared flags must be rejected");

    let text = err.to_string();
    assert!(
        text.contains("unknown flag(s) for flagcli_build: alpha, zeta"),
        "every unknown flag must be listed, sorted, and the declared one omitted: {text}"
    );

    shutdown(client, cancel, server_task).await;
}
