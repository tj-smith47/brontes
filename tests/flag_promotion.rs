//! SEP-2243 `x-mcp-header` flag promotion.
//!
//! Promotion moves a flag out of the tool's `flags` object and up to a
//! top-level input-schema property annotated with `x-mcp-header`, so a
//! streamable-HTTP client mirrors its value into `Mcp-Param-*` and an
//! intermediary can route on it without parsing the body. Two halves must both
//! hold or the feature is worse than not having it:
//!
//! - the advertised schema puts the annotation where rmcp actually reads it,
//!   and stops advertising the flag in the place it no longer belongs;
//! - a call using the new shape reaches the CLI with exactly the argv it would
//!   have had before promotion.
//!
//! The misconfiguration cases are asserted as hard startup errors rather than
//! warnings: an annotation a peer rejects fails the whole request, so the
//! failure belongs at `generate_tools` time where the developer sees it.

use clap::{Arg, ArgAction, Command, value_parser};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RoleClient;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::BrontesServer;
use brontes::{BoxedNext, Config, Middleware, MiddlewareCtx, Selector, ToolInput, generate_tools};

fn fixture_cli() -> Command {
    Command::new("promo").version("0.0.1").subcommand(
        Command::new("deploy")
            .about("Deploy something")
            .arg(Arg::new("region").long("region").required(true))
            .arg(
                Arg::new("dry-run")
                    .long("dry-run")
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("replicas")
                    .long("replicas")
                    .value_parser(value_parser!(i64)),
            )
            .arg(
                Arg::new("ratio")
                    .long("ratio")
                    .value_parser(value_parser!(f64)),
            )
            .arg(
                Arg::new("tag")
                    .long("tag")
                    .action(ArgAction::Append)
                    .value_parser(value_parser!(String)),
            ),
    )
}

/// The `deploy` tool's input schema under `cfg`.
fn deploy_schema(cfg: &Config) -> Value {
    let tools = generate_tools(&fixture_cli(), cfg).expect("tool generation");
    let tool = tools
        .iter()
        .find(|t| t.name == "promo_deploy")
        .expect("deploy tool");
    serde_json::to_value(&*tool.input_schema).expect("schema to json")
}

fn top_level_props(schema: &Value) -> &serde_json::Map<String, Value> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("properties")
}

fn flag_props(schema: &Value) -> &serde_json::Map<String, Value> {
    schema
        .pointer("/properties/flags/properties")
        .and_then(Value::as_object)
        .expect("flags.properties")
}

// ─────────────────────────────────────────────────────────────────────────────
// The advertised schema
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_promoted_flag_is_advertised_at_the_top_level_with_the_annotation() {
    let cfg = Config::default().promote_flag("promo deploy", "region");
    let schema = deploy_schema(&cfg);

    let region = top_level_props(&schema)
        .get("region")
        .expect("region must be a top-level property — the only place rmcp reads the annotation");
    assert_eq!(
        region.get("x-mcp-header"),
        Some(&json!("region")),
        "the derived header suffix is the flag name: {region}"
    );
    assert_eq!(region.get("type"), Some(&json!("string")));
}

#[test]
fn a_promoted_flag_is_no_longer_advertised_under_flags() {
    // Leaving it in both places is the failure this design exists to avoid: a
    // model would pick one at random and the header would disagree with the body.
    let cfg = Config::default().promote_flag("promo deploy", "region");
    let schema = deploy_schema(&cfg);

    assert!(
        !flag_props(&schema).contains_key("region"),
        "promoted flag must not remain under `flags`: {schema}"
    );
    assert!(
        flag_props(&schema).contains_key("dry-run"),
        "unpromoted flags must be untouched: {schema}"
    );
}

#[test]
fn a_required_promoted_flag_moves_to_the_top_level_required_list() {
    let cfg = Config::default().promote_flag("promo deploy", "region");
    let schema = deploy_schema(&cfg);

    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .expect("required")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        required.contains(&"region"),
        "a required flag stays required after promotion: {required:?}"
    );

    let flags_required = schema
        .pointer("/properties/flags/required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        !flags_required.contains(&"region"),
        "and must not stay required in a place it can no longer be supplied: {flags_required:?}"
    );
}

#[test]
fn an_explicit_header_name_is_used_verbatim() {
    let cfg = Config::default().promote_flag_as("promo deploy", "dry-run", "Dry-Run");
    let schema = deploy_schema(&cfg);

    assert_eq!(
        top_level_props(&schema)
            .get("dry-run")
            .and_then(|p| p.get("x-mcp-header")),
        Some(&json!("Dry-Run")),
    );
}

#[test]
fn promotion_carries_the_flags_type_and_description_across() {
    let cfg = Config::default().promote_flag("promo deploy", "replicas");
    let schema = deploy_schema(&cfg);

    assert_eq!(
        top_level_props(&schema)
            .get("replicas")
            .and_then(|p| p.get("type")),
        Some(&json!("integer")),
        "an integer flag promotes as an integer, not a string"
    );
}

#[test]
fn nothing_is_promoted_without_configuration() {
    // Disconfirming direction: every assertion above must be caused by the
    // config, not by something the schema builder does unconditionally.
    let schema = deploy_schema(&Config::default());

    assert!(top_level_props(&schema).get("region").is_none());
    assert!(flag_props(&schema).contains_key("region"));
    let has_annotation = flag_props(&schema)
        .values()
        .any(|p| p.get("x-mcp-header").is_some());
    assert!(!has_annotation, "no annotation appears unbidden: {schema}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Misconfiguration fails at startup
// ─────────────────────────────────────────────────────────────────────────────

fn promotion_error(cfg: &Config) -> String {
    generate_tools(&fixture_cli(), cfg)
        .expect_err("this configuration must not produce a tool list")
        .to_string()
}

#[test]
fn promoting_an_unknown_flag_fails_at_startup() {
    let cfg = Config::default().promote_flag("promo deploy", "nonexistent");
    let msg = promotion_error(&cfg);
    assert!(
        msg.contains("promoted_flags") && msg.contains("nonexistent"),
        "{msg}"
    );
}

#[test]
fn promoting_a_flag_on_an_unknown_command_fails_at_startup() {
    let cfg = Config::default().promote_flag("promo nosuchcmd", "region");
    let msg = promotion_error(&cfg);
    assert!(msg.contains("promoted_flags"), "{msg}");
}

#[test]
fn a_header_name_that_is_not_an_http_token_fails_at_startup() {
    let cfg = Config::default().promote_flag_as("promo deploy", "region", "not a token");
    let msg = promotion_error(&cfg);
    assert!(msg.contains("valid HTTP token"), "{msg}");
}

#[test]
fn two_flags_promoting_to_one_header_fail_at_startup() {
    let cfg = Config::default()
        .promote_flag_as("promo deploy", "region", "Zone")
        .promote_flag_as("promo deploy", "replicas", "zone");
    let msg = promotion_error(&cfg);
    assert!(msg.contains("case-insensitive"), "{msg}");
}

#[test]
fn a_non_primitive_flag_fails_at_startup() {
    // An array cannot be a header value, and rmcp rejects the annotation on
    // one; refusing here beats advertising a schema that breaks every call.
    let cfg = Config::default().promote_flag("promo deploy", "tag");
    let msg = promotion_error(&cfg);
    assert!(msg.contains("string, integer, and boolean"), "{msg}");
}

#[test]
fn a_number_flag_fails_at_startup() {
    let cfg = Config::default().promote_flag("promo deploy", "ratio");
    let msg = promotion_error(&cfg);
    assert!(msg.contains("number"), "{msg}");
}

// ─────────────────────────────────────────────────────────────────────────────
// The call path, over a real MCP transport
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct NoopClient;

impl rmcp::handler::client::ClientHandler for NoopClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        rmcp::model::ClientInfo::default()
    }
}

/// Middleware that records the `ToolInput` the pipeline resolved and returns
/// without running anything, so the assertion is on what the CLI *would* be
/// handed rather than on a subprocess's output.
fn capturing_config(seen: Arc<Mutex<Option<ToolInput>>>, cfg: Config) -> Config {
    let mw: Middleware = Arc::new(move |ctx: MiddlewareCtx, _next: BoxedNext| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            *seen.lock().unwrap() = Some(ctx.input.clone());
            Ok(brontes::ToolOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            }
            .into())
        })
    });
    cfg.selector(Selector {
        middleware: Some(mw),
        ..Default::default()
    })
}

async fn spin_up(
    cfg: Config,
) -> (
    rmcp::service::RunningService<RoleClient, NoopClient>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let (client_io, server_io) = duplex(64 * 1024);
    let cancel = CancellationToken::new();
    let server_task = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let server = BrontesServer::new(fixture_cli(), cfg).expect("construct");
            if let Ok(running) = server.serve_with_ct(server_io, cancel).await {
                let _ = running.waiting().await;
            }
        })
    };

    let client = NoopClient
        .serve_with_ct(client_io, cancel.clone())
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

#[tokio::test]
async fn a_top_level_promoted_value_reaches_the_command_as_a_flag() {
    let seen = Arc::new(Mutex::new(None));
    let cfg = capturing_config(
        Arc::clone(&seen),
        Config::default().promote_flag("promo deploy", "region"),
    );
    let (client, cancel, server_task) = spin_up(cfg).await;

    let mut args = serde_json::Map::new();
    args.insert("region".into(), json!("us-east-1"));
    args.insert("flags".into(), json!({"dry-run": true}));
    args.insert("args".into(), json!([]));

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("promo_deploy").with_arguments(args))
        .await
        .expect("the promoted shape must be accepted");
    assert_eq!(result.is_error, Some(false));

    let input = seen.lock().unwrap().clone().expect("middleware ran");
    assert_eq!(
        input.flags.get("region"),
        Some(&json!("us-east-1")),
        "the hoisted value must be folded back into flags: {:?}",
        input.flags
    );
    assert_eq!(
        input.flags.get("dry-run"),
        Some(&json!(true)),
        "and the unpromoted flags must be untouched: {:?}",
        input.flags
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn the_old_nested_shape_is_rejected_for_a_promoted_flag() {
    // The cost of a single source of truth, asserted rather than assumed: once
    // a flag is promoted the schema no longer offers it under `flags`, and the
    // `additionalProperties: false` enforcement makes that refusal real.
    let seen = Arc::new(Mutex::new(None));
    let cfg = capturing_config(
        Arc::clone(&seen),
        Config::default().promote_flag("promo deploy", "region"),
    );
    let (client, cancel, server_task) = spin_up(cfg).await;

    let mut args = serde_json::Map::new();
    args.insert("flags".into(), json!({"region": "us-east-1"}));
    args.insert("args".into(), json!([]));

    let err = client
        .peer()
        .call_tool(CallToolRequestParams::new("promo_deploy").with_arguments(args))
        .await
        .expect_err("a promoted flag must not also be accepted in its old place");
    assert!(
        err.to_string()
            .contains("must be supplied as top-level arguments, not under `flags`: region"),
        "{err}"
    );
    assert!(
        seen.lock().unwrap().is_none(),
        "the rejection must happen before the pipeline runs"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn an_unpromoted_flag_still_arrives_nested() {
    // Disconfirming pair for the test above: with promotion configured for one
    // flag, every other flag keeps working exactly as before.
    let seen = Arc::new(Mutex::new(None));
    let cfg = capturing_config(
        Arc::clone(&seen),
        Config::default().promote_flag("promo deploy", "region"),
    );
    let (client, cancel, server_task) = spin_up(cfg).await;

    let mut args = serde_json::Map::new();
    args.insert("region".into(), json!("eu-west-1"));
    args.insert("flags".into(), json!({"replicas": 3}));
    args.insert("args".into(), json!([]));

    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("promo_deploy").with_arguments(args))
        .await
        .expect("call");
    assert_eq!(result.is_error, Some(false));

    let input = seen.lock().unwrap().clone().expect("middleware ran");
    assert_eq!(input.flags.get("replicas"), Some(&json!(3)));

    shutdown(client, cancel, server_task).await;
}
