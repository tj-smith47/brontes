//! The `mcp` clap subtree builder.
//!
//! Registered via [`crate::command`] onto the consumer's CLI; dispatched
//! via [`crate::handle`]. The subtree is structurally:
//!
//! ```text
//! mcp
//! ├── start       — serve MCP over stdio
//! ├── tools       — export the tool list to ./mcp-tools.json
//! ├── stream      — serve MCP over streamable HTTP
//! ├── claude      — manage Claude Desktop MCP servers
//! └── cursor      — manage Cursor MCP servers (user + workspace)
//! ```
//!
//! Plus a hidden internal marker subcommand (`MARKER_NAME`) that lets
//! [`crate::handle`] disambiguate "the `mcp` subcommand brontes added" from
//! "a `mcp` subcommand the user happened to register before mounting brontes".

pub(crate) mod editor;
pub(crate) mod start;
pub(crate) mod stream;
pub(crate) mod tools;

use clap::Command;

/// Hidden marker subcommand name. Its presence under the `mcp` group is the
/// signal [`crate::handle`] uses to confirm the group was minted by brontes
/// rather than by a colliding user-defined subcommand.
///
/// The double-underscore prefix avoids any plausible collision with a real
/// user-facing subcommand; the literal name is documented internally only
/// (consumers never invoke it).
//
// This name is implementation detail and may change without notice; do not
// pattern-match on it externally.
pub(crate) const MARKER_NAME: &str = "__brontes_internal_marker";

/// Build the `mcp` subtree (group command + start/tools/stream children).
///
/// `command_name` is the configured group name — defaults to `"mcp"`,
/// overridden by [`crate::Config::command_name`]. The group itself has no
/// runnable body; one of its children must be invoked.
pub(crate) fn build(command_name: &str) -> Command {
    Command::new(command_name.to_string())
        .about("MCP server management")
        .long_about("Manage MCP servers for AI assistants and code editors")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(start::build())
        .subcommand(tools::build())
        .subcommand(stream::build())
        .subcommand(editor::claude::build())
        .subcommand(editor::cursor::build())
        .subcommand(
            // Hidden marker; carries no flags and is never meant to be run.
            // Used purely so `handle()` can fingerprint the group as ours.
            Command::new(MARKER_NAME).hide(true).disable_help_flag(true),
        )
}
