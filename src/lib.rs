//! brontes: transform clap CLIs into MCP servers.
//!
//! brontes walks a [`clap::Command`] tree, exposes every reachable command as an
//! [MCP](https://modelcontextprotocol.io) tool, and ships editor-config helpers
//! for Claude Desktop, `VSCode`, and Cursor.
//!
//! # Status
//!
//! Phase 1 in progress. The public surface currently exports the crate's
//! [`Error`] and [`Result`] types. The MCP-tool generation surface
//! (`generate_tools`) and the `mcp` subcommand tree land in subsequent
//! Phase 1 / Phase 2 commits.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod error;

pub use error::{Error, Result};
