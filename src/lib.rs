//! brontes: transform clap CLIs into MCP servers.
//!
//! brontes walks a [`clap::Command`] tree, exposes every reachable command as an
//! [MCP](https://modelcontextprotocol.io) tool, and ships editor-config helpers
//! for Claude Desktop, `VSCode`, and Cursor.
//!
//! # Quick start
//!
//! ```rust
//! use clap::Command;
//! use brontes::Config;
//!
//! let root = Command::new("myapp")
//!     .subcommand(Command::new("deploy").about("Deploy the app"));
//!
//! let tools = brontes::generate_tools(&root, &Config::default())
//!     .expect("valid config");
//! // `tools` is a Vec<rmcp::model::Tool> ready to register with a server.
//! ```
//!
//! # Status
//!
//! This release ships brontes' **library surface**: [`generate_tools`]
//! and its supporting types ([`Config`], [`Selector`], the
//! [`selectors`] factory functions, [`ToolAnnotations`], [`ToolInput`],
//! [`ToolOutput`], [`SchemaType`], [`Error`], [`Result`], and the
//! middleware plumbing — [`MiddlewareCtx`], [`Middleware`],
//! [`BoxedNext`], [`MiddlewareResult`]). A consumer can build the MCP
//! tool list for their `clap::Command` tree today without a running
//! MCP server.
//!
//! Not yet shipped, planned for a later minor release:
//!
//! - The MCP server runtime — `brontes::command()`, `brontes::handle()`,
//!   and `brontes::run()` — that turns a generated tool list into a
//!   live MCP server.
//! - Editor manager subcommands for Claude Desktop, Cursor, and `VSCode`
//!   config integration.
//! - HTTP streamable transport (stdio support will land alongside the
//!   server runtime).
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
mod schema;
mod selector;
pub mod selectors;
mod tool;
mod walk;

pub use annotations::ToolAnnotations;
pub use command::generate_tools;
pub use config::Config;
pub use error::{Error, Result};
pub use schema::SchemaType;
pub use selector::{
    BoxedNext, CmdMatcher, FlagMatcher, Middleware, MiddlewareCtx, MiddlewareResult, Selector,
};
pub use tool::{ToolInput, ToolOutput};
