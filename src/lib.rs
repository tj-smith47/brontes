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
/// drive [`server::BrontesServer`] over an in-memory duplex transport
/// or [`server::http::serve_http`] against an ephemeral local port.
// Not a semver-stable surface. Downstream crates relying on this break without notice.
#[doc(hidden)]
pub mod __test_internal {
    pub use crate::server::BrontesServer;
    pub use crate::server::http::serve_http;
}
