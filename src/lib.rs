//! brontes: transform clap CLIs into MCP servers.
//!
//! brontes walks a [`clap::Command`] tree, exposes every reachable command as an
//! [MCP](https://modelcontextprotocol.io) tool, and ships editor-config helpers
//! for Claude Desktop, `VSCode`, and Cursor.
//!
//! # Status
//!
//! Early development. The public surface currently exports the crate's
//! [`Error`] and [`Result`] types. The MCP tool-generation surface and
//! the `mcp` subcommand tree are not yet exposed.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod error;

pub use error::{Error, Result};
