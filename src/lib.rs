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
