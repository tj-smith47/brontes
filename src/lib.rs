//! brontes: transform clap CLIs into MCP servers.
//!
//! brontes walks a [`clap::Command`] tree, exposes every reachable command as an
//! [MCP](https://modelcontextprotocol.io) tool, and ships the editor-config helpers
//! (`enable`/`disable`/`list`) for Claude Desktop, `VSCode`, and Cursor.
//!
//! See the [README](https://github.com/tj-smith47/brontes) for a full quick-start.
//!
//! # Status
//!
//! Phase 0 scaffold. The public surface is intentionally empty at this revision;
//! see the project's PLAN for the phased rollout.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod error;

pub use error::{Error, Result};
