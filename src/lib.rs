//! brontes: transform clap CLIs into MCP servers.
//!
//! brontes walks a [`clap::Command`] tree, exposes every reachable command as an
//! [MCP](https://modelcontextprotocol.io) tool, and ships a complete MCP server
//! runtime over stdio so the resulting agent surface plugs straight into Claude
//! Desktop, Cursor, and `VSCode`.
//!
//! # Two-line quick start
//!
//! ```no_run
//! use clap::Command;
//!
//! #[tokio::main]
//! async fn main() -> brontes::Result<()> {
//!     let cli = Command::new("my-cli")
//!         .version("0.1.0")
//!         .subcommand(Command::new("greet").about("Say hi"))
//!         .subcommand(brontes::command(None));            // [1] mount
//!
//!     let matches = cli.clone().get_matches();
//!     match matches.subcommand() {
//!         Some(("mcp", sub)) => brontes::handle(sub, &cli, None).await,  // [2] dispatch
//!         Some(("greet", _)) => { println!("hi"); Ok(()) }
//!         _ => Ok(()),
//!     }
//! }
//! ```
//!
//! For tiny CLIs whose only purpose is the MCP server, collapse the
//! ceremony into one line with [`run`]:
//!
//! ```no_run
//! use clap::Command;
//!
//! #[tokio::main]
//! async fn main() -> brontes::Result<()> {
//!     brontes::run(Command::new("my-cli").version("0.1.0"), None).await
//! }
//! ```
//!
//! # Capabilities
//!
//! - [`generate_tools`] — walk a [`clap::Command`] tree into a
//!   [`Vec<rmcp::model::Tool>`](rmcp::model::Tool) for offline inspection,
//!   editor-config generation, or hand-rolled server wiring.
//! - [`command`], [`handle`], [`run`] — mount the `mcp` subtree and serve
//!   the generated tool list over stdio (`mcp start`) or streamable HTTP
//!   (`mcp stream --host <addr> --port <num>`).
//! - [`Config`] — selectors, annotations, per-flag schema overrides,
//!   default environment variables, server identity overrides.
//! - [`Selector`], [`Middleware`] — first-match-wins routing rules and
//!   an async middleware boundary for wrapping tool execution.
//!
//! Bug reports and feature requests:
//! <https://github.com/tj-smith47/brontes/issues>.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod annotations;
mod command;
mod config;
mod error;
mod exec;
mod manager;
mod schema;
mod selector;
pub mod selectors;
mod server;
mod subcommands;
mod tool;
mod walk;

pub use annotations::ToolAnnotations;
pub use command::{command, generate_tools, handle, run};
pub use config::Config;
pub use error::{Error, Result};
pub use schema::SchemaType;
pub use selector::{
    BoxedNext, CmdMatcher, FlagMatcher, Middleware, MiddlewareCtx, MiddlewareResult, Selector,
};
pub use tool::{ToolInput, ToolOutput};

/// Internal-test access point: not a stable surface, do not use from
/// downstream crates. Re-exported only so the integration-test crate can
/// drive [`server::BrontesServer`] over an in-memory duplex transport,
/// [`server::http::serve_http`] against an ephemeral local port, or the
/// private helpers that emit `tracing::warn!` events the warn-fire test
/// suite asserts on.
// Not a semver-stable surface. Downstream crates relying on this break without notice.
#[doc(hidden)]
pub mod __test_internal {
    pub use crate::server::BrontesServer;
    pub use crate::server::http::serve_http;
    /// Re-exported HTTP-server internals so the warn-fire test crate can
    /// drive `serve_http_with` against a faulty acceptor and a compressed
    /// shutdown grace — see `tests/warn_fires.rs` for the two assertions.
    pub use crate::server::http::{
        Acceptor, SHUTDOWN_GRACE, TokioTcpAcceptor, bind_default_acceptor, serve_http_with,
    };
    /// Re-exported [`hyper_util::rt::TokioIo`] so the warn-fire test crate
    /// can satisfy the [`Acceptor::accept`] return type without taking
    /// `hyper-util` as a dev-dependency (it is already a main dep).
    pub use hyper_util::rt::TokioIo;

    /// Drive the same flag-rendering logic that `mcp start` / `mcp stream`
    /// use when translating a tool call's JSON `flags` map into argv. The
    /// integration test crate uses this to assert that the §11 #7
    /// nested-non-scalar `tracing::warn!` events fire as documented.
    #[must_use]
    pub fn render_flag_argv(
        flag_name: &str,
        value: &serde_json::Value,
        tool_name: &str,
    ) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        crate::exec::append_flag_for_test(&mut out, flag_name, value, tool_name);
        out
    }

    /// Drive the `OUTPUT_CAP_BYTES` capture path on an in-memory reader so
    /// the warn-fire test crate can assert the soft-cap `tracing::warn!`
    /// fires exactly once per stream when output exceeds the cap. The
    /// returned `Vec<u8>` is the retained bytes — the test does not need
    /// it but receives it for symmetry with the production reader.
    pub async fn drain_capped<R>(
        reader: R,
        stream_label: &'static str,
        tool_name: String,
    ) -> Vec<u8>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        crate::exec::read_capped_for_test(reader, stream_label, tool_name).await
    }

    /// Exposed cap (16 MiB) so the warn-fire test crate can build a
    /// reader that overshoots without re-deriving the constant.
    pub const OUTPUT_CAP_BYTES: usize = crate::exec::OUTPUT_CAP_BYTES;

    /// Drive the `mcp start` `--log-level` parser on a prebuilt
    /// `ArgMatches`. Returns `Some(level)` on a recognized value, `None`
    /// on an unrecognized value (which also emits the §11 #9
    /// `tracing::warn!` the warn-fire test crate asserts on).
    #[must_use]
    pub fn parse_start_log_level(matches: &clap::ArgMatches) -> Option<tracing::Level> {
        crate::subcommands::start::parse_log_level_for_test(matches)
    }

    /// Build the `mcp start` subcommand. Lets the test crate build an
    /// `ArgMatches` with `--log-level <raw>` via the same parser shape
    /// the production code uses.
    #[must_use]
    pub fn start_subcommand() -> clap::Command {
        crate::subcommands::start::build_for_test()
    }

    /// Drive the `mcp stream` `--log-level` parser. Same shape as
    /// [`parse_start_log_level`]; both surfaces carry the same warn so
    /// the test suite exercises each independently.
    #[must_use]
    pub fn parse_stream_log_level(matches: &clap::ArgMatches) -> Option<tracing::Level> {
        crate::subcommands::stream::parse_log_level_for_test(matches)
    }

    /// Build the `mcp stream` subcommand for `--log-level` test driving.
    #[must_use]
    pub fn stream_subcommand() -> clap::Command {
        crate::subcommands::stream::build_for_test()
    }
}
