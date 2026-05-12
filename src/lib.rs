//! brontes: transform clap CLIs into MCP servers.
//!
//! brontes walks a [`clap::Command`] tree, exposes every reachable command as an
//! [MCP](https://modelcontextprotocol.io) tool, and ships editor-config helpers
//! for Claude Desktop, `VSCode`, and Cursor.
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

// Phase 0: `Error` and `Result` are defined here for Phase 1+ wiring and are
// covered by unit tests, but no public API is yet exported.
#[allow(dead_code)]
mod error;
