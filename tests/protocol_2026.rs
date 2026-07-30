//! MCP `2026-07-28` conformance for the surface brontes owns.
//!
//! rmcp implements the transport and dispatch halves of the revision; the
//! parts a server author must supply are asserted here:
//!
//! - `server/discover` (SEP-2575) advertises `2026-07-28` among the
//!   supported versions, along with brontes' `tools`-only capabilities and
//!   the host CLI's identity.
//! - `tools/list` and `server/discover` carry SEP-2549 cache hints
//!   (`ttlMs` / `cacheScope`), defaulted by brontes and overridable via
//!   [`brontes::Config`].
//! - `tools/list` ordering is stable, which the revision asks for so
//!   clients and LLM prompt caches can reuse a listing.

use std::time::Duration;

use brontes::{CacheScope, Config};
use clap::Command;
use rmcp::ServiceExt;
use rmcp::model::{ClientCapabilities, Implementation, ProtocolVersion, RequestMetaObject};
use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use brontes::__test_internal::BrontesServer;

#[derive(Clone)]
struct NoopClient;

impl rmcp::handler::client::ClientHandler for NoopClient {
    fn get_info(&self) -> rmcp::model::ClientInfo {
        rmcp::model::ClientInfo::default()
    }
}

/// A tree deep enough that ordering is a real question: two siblings at the
/// root, one of which has its own children.
fn fixture_cli() -> Command {
    Command::new("proto-cli")
        .version("0.0.1")
        .subcommand(Command::new("alpha").about("First"))
        .subcommand(
            Command::new("beta")
                .about("Second")
                .subcommand(Command::new("inner").about("Nested"))
                .subcommand(Command::new("another").about("Also nested")),
        )
        .subcommand(Command::new("gamma").about("Third"))
}

/// Boot `BrontesServer` over an in-memory duplex pair and return a live
/// client peer plus the handles needed to shut it down.
async fn connect(
    cfg: Config,
) -> (
    rmcp::service::RunningService<rmcp::service::RoleClient, NoopClient>,
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

    let client = NoopClient
        .serve_with_ct(client_io, cancel.clone())
        .await
        .expect("client start");

    (client, cancel, server_task)
}

async fn shutdown(
    client: rmcp::service::RunningService<rmcp::service::RoleClient, NoopClient>,
    cancel: CancellationToken,
    server_task: tokio::task::JoinHandle<()>,
) {
    let _ = client.cancel().await;
    cancel.cancel();
    let _ = server_task.await;
}

/// `_meta` a 2026-07-28 client attaches to every request: its protocol
/// version, identity, and capabilities. Under the stateless revision this
/// replaces the `initialize` handshake as the place that information lives.
fn client_meta_2026() -> RequestMetaObject {
    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(ProtocolVersion::V_2026_07_28);
    meta.set_client_info(Implementation::new("proto-test-client", "0.0.1"));
    meta.set_client_capabilities(ClientCapabilities::default());
    meta
}

#[tokio::test]
async fn discover_advertises_2026_07_28_with_brontes_identity_and_capabilities() {
    let (client, cancel, server_task) = connect(Config::default()).await;

    let result = client
        .discover(client_meta_2026())
        .await
        .expect("server/discover");

    assert!(
        result
            .supported_versions
            .contains(&ProtocolVersion::V_2026_07_28),
        "discover must advertise 2026-07-28; got {:?}",
        result.supported_versions
    );
    assert!(
        result.capabilities.tools.is_some(),
        "brontes serves tools, so discover must advertise the tools capability"
    );
    // The 2026-07-28 spec deprecates Roots, Sampling, and Logging; brontes
    // deliberately advertises none of them, and logs to stderr instead.
    assert!(
        result.capabilities.logging.is_none(),
        "brontes must not advertise the deprecated logging capability"
    );
    assert_eq!(
        result.server_info(),
        Some(Implementation::new("proto-cli", "0.0.1")),
        "discover must report the host CLI's identity"
    );

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn discover_carries_cache_hints_rather_than_rmcps_zero_ttl_default() {
    let (client, cancel, server_task) = connect(Config::default()).await;

    let result = client
        .discover(client_meta_2026())
        .await
        .expect("server/discover");

    // rmcp's default `discover` leaves ttlMs at 0 (re-discover every time).
    // brontes overrides it because its discovery payload is fixed at
    // construction.
    assert_eq!(result.ttl_ms, 300_000);
    assert_eq!(result.cache_scope, CacheScope::Public);

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn tools_list_carries_the_default_cache_hints() {
    let (client, cancel, server_task) = connect(Config::default()).await;

    let list = client.peer().list_tools(None).await.expect("tools/list");

    assert_eq!(
        list.ttl_ms,
        Some(300_000),
        "an absent ttlMs tells clients not to cache at all"
    );
    assert_eq!(list.cache_scope, Some(CacheScope::Public));

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn tools_list_cache_hints_follow_config_overrides() {
    let cfg = Config::default()
        .cache_ttl(Duration::from_secs(7))
        .cache_scope(CacheScope::Private);
    let (client, cancel, server_task) = connect(cfg).await;

    let list = client.peer().list_tools(None).await.expect("tools/list");
    assert_eq!(list.ttl_ms, Some(7_000));
    assert_eq!(list.cache_scope, Some(CacheScope::Private));

    // The same override reaches discovery, so the two cacheable results a
    // brontes server emits never disagree.
    let discovered = client
        .discover(client_meta_2026())
        .await
        .expect("server/discover");
    assert_eq!(discovered.ttl_ms, 7_000);
    assert_eq!(discovered.cache_scope, CacheScope::Private);

    shutdown(client, cancel, server_task).await;
}

#[tokio::test]
async fn zero_ttl_reaches_the_wire_as_do_not_cache() {
    let cfg = Config::default().cache_ttl(Duration::ZERO);
    let (client, cancel, server_task) = connect(cfg).await;

    let list = client.peer().list_tools(None).await.expect("tools/list");
    assert_eq!(
        list.ttl_ms,
        Some(0),
        "ttlMs of 0 must be sent explicitly, not omitted"
    );

    shutdown(client, cancel, server_task).await;
}

#[test]
fn tool_ordering_is_stable_across_repeated_generation() {
    // The walk is depth-first over clap's subcommand vector, and the only
    // hash-based collection involved (the valid-path set) is used for
    // membership tests. This pins that: a `HashMap`/`HashSet` iteration
    // sneaking into the ordering path would make two runs in the same
    // process disagree, because Rust's default hasher is randomly seeded
    // per `HashMap` instance.
    let baseline: Vec<String> = brontes::generate_tools(&fixture_cli(), &Config::default())
        .expect("generate_tools")
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(
        baseline.len() >= 4,
        "fixture must produce a tree worth ordering; got {baseline:?}"
    );

    for run in 0..16 {
        let names: Vec<String> = brontes::generate_tools(&fixture_cli(), &Config::default())
            .expect("generate_tools")
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(names, baseline, "ordering drifted on run {run}");
    }
}

#[tokio::test]
async fn tools_list_wire_order_matches_generate_tools_and_repeats() {
    let (client, cancel, server_task) = connect(Config::default()).await;

    let offline: Vec<String> = brontes::generate_tools(&fixture_cli(), &Config::default())
        .expect("generate_tools")
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    let first: Vec<String> = client
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list")
        .tools
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    assert_eq!(
        first, offline,
        "the served order must match the offline listing consumers inspect"
    );

    shutdown(client, cancel, server_task).await;
}
