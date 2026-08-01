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
//!   default environment variables, server identity overrides, per-command
//!   description mode and full-text override, and the SEP-2549 cache hints
//!   advertised on `tools/list` / `server/discover`.
//! - [`Selector`], [`Middleware`] — first-match-wins routing rules and
//!   an async middleware boundary for wrapping tool execution.
//!
//! # Protocol support
//!
//! brontes negotiates every MCP revision rmcp knows, through `2026-07-28`.
//! That revision is stateless — no `initialize` handshake and no session
//! id — and adds `server/discover`, a `resultType` discriminator on every
//! result, and cache hints on list results; brontes serves all three.
//! Earlier revisions back to `2024-11-05` still negotiate, handshake and
//! all.
//!
//! A brontes server advertises exactly one capability, `tools`. Roots,
//! Sampling, and Logging are deprecated as of `2026-07-28` and brontes
//! implements none of them: it logs to stderr via `tracing`, and
//! [`Middleware`] is the extension point for anything a server would
//! otherwise delegate to the client.
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
mod toolset;
mod trace;
mod walk;

pub use annotations::ToolAnnotations;
pub use command::{command, generate_tools, handle, run, run_from};
pub use config::{Config, DescriptionMode, TaskMode};
pub use error::{Error, Result};
// Re-exported so setting `Config::cache_scope` does not force a direct `rmcp`
// dependency on consumers — and, more importantly, so the value they pass is
// guaranteed to be the same `CacheScope` the linked rmcp expects rather than
// one from a different rmcp major version in their own tree.
pub use rmcp::model::CacheScope;
// Re-exported for the same reason as `CacheScope`: [`generate_tools`] hands
// back a `Vec<Tool>`, and a consumer who names that type — in a test helper,
// or a function that post-processes the list — should not have to add an
// `rmcp` dependency, nor risk naming a `Tool` from a different rmcp major.
pub use rmcp::model::Tool;
pub use schema::SchemaType;
pub use selector::{
    BoxedNext, CmdMatcher, FlagMatcher, Middleware, MiddlewareCtx, MiddlewareOutcome,
    MiddlewareResult, Selector,
};
pub use tool::{ToolInput, ToolOutput};
pub use toolset::{Group, ToolFilter};
pub use trace::TraceContext;

/// The `rmcp` types that appear on brontes' own public surface.
///
/// [`MiddlewareCtx`] hands a middleware the negotiated protocol version, the
/// client's declared capabilities, the raw request `_meta`, and — on an MRTR
/// retry — the client's answers; [`MiddlewareOutcome::InputRequired`] takes an
/// [`InputRequiredResult`]. None of that is usable if the consumer cannot name
/// the types, and naming them through a direct `rmcp` dependency risks a
/// different major version than the one brontes is linked against, which fails
/// as a confusing type mismatch. Re-exporting is the same reasoning as
/// [`CacheScope`] above, applied to the whole boundary.
///
/// Only the elicitation half of [`InputRequest`] is re-exported. Sampling and
/// roots are deprecated as of `2026-07-28`, and re-exporting their envelopes
/// would raise a deprecation warning in every consumer that so much as links
/// brontes. brontes still forwards a sampling or roots request a middleware
/// builds — it just will not put the warning in everyone else's build to do it;
/// a middleware that wants one depends on `rmcp` directly and owns the choice.
pub use rmcp::model::{
    ClientCapabilities, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
    ElicitationSchema, InputRequest, InputRequests, InputRequiredResult, InputResponses,
    ProtocolVersion, RequestMetaObject,
};

/// Seal and verify an MRTR `requestState` so an echoed value cannot be forged.
///
/// Available with the `request-state` feature, which forwards `rmcp`'s. A
/// middleware that lets `requestState` influence authorization or resource
/// access is required by SEP-2322 to verify its integrity first; this is the
/// tool for that.
#[cfg(feature = "request-state")]
pub use rmcp::model::{RequestStateCodec, SealOptions};

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
    /// Re-exported signal-listener internals so the warn-fire test crate
    /// can drive `spawn_signal_listener_with` against a faulty
    /// [`SignalSource`] and assert the two `tracing::warn!` install
    /// failure paths (`could not install SIGINT handler` /
    /// `could not install SIGTERM handler`) fire as documented. Mirrors
    /// the `SignalSource` re-export pattern used for `Acceptor`.
    ///
    /// [`SignalSource`]: crate::subcommands::signal::SignalSource
    #[cfg(unix)]
    pub use crate::subcommands::signal::{
        SignalSource, TokioUnixSignalSource, spawn_signal_listener_with,
    };
    /// Re-exported [`hyper_util::rt::TokioIo`] so the warn-fire test crate
    /// can satisfy the [`Acceptor::accept`] return type without taking
    /// `hyper-util` as a dev-dependency (it is already a main dep).
    pub use hyper_util::rt::TokioIo;

    /// How brontes lowers a flag's JSON value into argv. Re-exported so the
    /// integration test crate can drive [`render_flag_argv`] across every
    /// rendering mode, including the `ArgAction::Count` repetition form.
    pub use crate::schema::FlagRender;

    /// Render the argv for a tool call the way `mcp start` / `mcp stream` do,
    /// deriving the per-flag render kinds from a real walk of `root`.
    ///
    /// [`render_flag_argv`] proves the renderer behaves once told how to render;
    /// this proves the walk tells it correctly — the `clap::ArgAction` →
    /// [`FlagRender`] → argv chain end to end, short only of the spawn.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Config`] when the walk rejects `cfg`, or when
    /// `tool_name` is not in the generated tool list.
    pub fn render_tool_argv(
        root: &clap::Command,
        cfg: &crate::Config,
        tool_name: &str,
        input: &crate::ToolInput,
    ) -> crate::Result<Vec<String>> {
        let resolved = crate::command::generate_tools_with_middleware(root, cfg)?;
        let tool = resolved
            .iter()
            .find(|r| r.tool.name == tool_name)
            .ok_or_else(|| {
                crate::Error::Config(format!("render_tool_argv: no such tool {tool_name:?}"))
            })?;
        Ok(crate::exec::build_command_args(
            tool_name,
            input,
            &tool.flag_specs,
        ))
    }

    /// Drive the same flag-rendering logic that `mcp start` / `mcp stream`
    /// use when translating a tool call's JSON `flags` map into argv.
    ///
    /// The integration test crate uses this to assert that the
    /// nested-non-scalar and count-flag `tracing::warn!` events fire as
    /// documented.
    #[must_use]
    pub fn render_flag_argv(
        flag_name: &str,
        value: &serde_json::Value,
        render: FlagRender,
        tool_name: &str,
    ) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        crate::exec::append_flag_for_test(&mut out, flag_name, value, render, tool_name);
        out
    }

    /// Drive the `OUTPUT_CAP_BYTES` capture path on an in-memory reader so
    /// the warn-fire test crate can assert the soft-cap `tracing::warn!`
    /// fires exactly once per stream when output exceeds the cap.
    ///
    /// The returned `Vec<u8>` is the retained bytes — the test does not
    /// need it but receives it for symmetry with the production reader.
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
    /// `ArgMatches`.
    ///
    /// Returns `Some(level)` on a recognized value, `None` on an
    /// unrecognized value (which also emits the unrecognized-value
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

    /// Build the `mcp tools` subcommand, so the test crate can compare the
    /// three leaves' flag surfaces against each other.
    #[must_use]
    pub fn tools_subcommand() -> clap::Command {
        crate::subcommands::tools::build()
    }

    /// Fold the tool-selection flags in `matches` onto `cfg`, as each `mcp`
    /// leaf does at startup.
    #[must_use]
    pub fn apply_selection_flags(
        cfg: crate::Config,
        root: &str,
        matches: &clap::ArgMatches,
    ) -> crate::Config {
        crate::subcommands::common::apply_selection_flags(cfg, root, matches)
    }

    /// Append the tool-selection flags in `matches` to a generated
    /// `mcp start` argv, as `mcp <editor> enable` does.
    pub fn push_selection_flags(args: &mut Vec<String>, matches: &clap::ArgMatches) {
        crate::subcommands::editor::push_selection_flags(args, matches);
    }

    /// Drive `crate::subcommands::stream::run_with_cancel` with a
    /// caller-supplied cancellation token.
    ///
    /// Production code uses `stream::run`, which spawns its own signal
    /// listener and mints a fresh token; the integration test crate
    /// passes a pre-cancelled token so `serve_http` returns immediately
    /// after bind, without firing real SIGINT at the test process.
    /// Covers the argv-parsing + `SocketAddr` build + serve-http call
    /// path that `run` shares with production.
    pub async fn stream_run_with_cancel(
        matches: &clap::ArgMatches,
        cli: clap::Command,
        cfg: Option<crate::Config>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> crate::Result<()> {
        crate::subcommands::stream::run_with_cancel(matches, cli, cfg, cancel).await
    }

    /// Drive `crate::server::stdio::serve_stdio_with` with a caller-
    /// supplied transport pair and cancellation token.
    ///
    /// Production code uses `serve_stdio`, which always uses the real
    /// process stdio + signal-driven cancel; the integration test crate
    /// passes an in-memory [`tokio::io::duplex`] pair so the smoke test
    /// can drive a real MCP client peer against the server without
    /// touching the process's stdin/stdout.
    pub async fn serve_stdio_with<R, W>(
        cli: clap::Command,
        cfg: crate::Config,
        transport: (R, W),
        cancel: tokio_util::sync::CancellationToken,
    ) -> crate::Result<()>
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        crate::server::stdio::serve_stdio_with(cli, cfg, transport, cancel).await
    }

    /// Dispatch a synthesized `<editor>` subcommand match through the
    /// production editor `run()` entry.
    ///
    /// Used by the editor-error-paths integration tests to exercise the
    /// `unknown leaf` / `missing leaf` Err arms in each editor's `run()`
    /// (claude / cursor / vscode / zed) without needing each test to
    /// import the editor's private `run`.
    ///
    /// `editor` is one of `"claude"`, `"cursor"`, `"vscode"`, `"zed"`.
    /// An unrecognized editor name returns [`crate::Error::Config`].
    pub fn editor_run(
        editor: &str,
        matches: &clap::ArgMatches,
        cli: &clap::Command,
    ) -> crate::Result<()> {
        match editor {
            "claude" => crate::subcommands::editor::claude::run(matches, cli, None),
            "cursor" => crate::subcommands::editor::cursor::run(matches, cli, None),
            "vscode" => crate::subcommands::editor::vscode::run(matches, cli, None),
            "zed" => crate::subcommands::editor::zed::run(matches, cli, None),
            other => Err(crate::Error::Config(format!(
                "editor_run: unknown editor {other:?}"
            ))),
        }
    }
}
