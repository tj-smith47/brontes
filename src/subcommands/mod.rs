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
//! ├── vscode      — manage VSCode MCP servers (user + workspace)
//! ├── cursor      — manage Cursor MCP servers (user + workspace)
//! └── zed         — manage Zed MCP context_servers (user + workspace)
//! ```
//!
//! Plus a hidden internal marker subcommand (`MARKER_NAME`) that lets
//! [`crate::handle`] disambiguate "the `mcp` subcommand brontes added" from
//! "a `mcp` subcommand the user happened to register before mounting brontes".

pub mod common;
pub mod editor;
pub mod signal;
pub mod start;
pub mod stream;
pub mod tools;

use clap::Command;

/// Hidden marker subcommand name. Its presence under the `mcp` group is the
/// signal [`crate::handle`] uses to confirm the group was created by brontes.
/// A colliding user-defined subcommand does not carry the marker.
///
/// The double-underscore prefix avoids any plausible collision with a real
/// user-facing subcommand; the literal name is documented internally only
/// (consumers never invoke it).
//
// This name is implementation detail and may change without notice; do not
// pattern-match on it externally.
pub const MARKER_NAME: &str = "__brontes_internal_marker";

/// Build the `mcp` subtree (group command + start/tools/stream children).
///
/// `command_name` is the configured group name — defaults to `"mcp"`,
/// overridden by [`crate::Config::command_name`]. The group itself has no
/// runnable body; one of its children must be invoked.
pub fn build(command_name: &str) -> Command {
    Command::new(command_name.to_string())
        .about("MCP server management")
        .long_about("Manage MCP servers for AI assistants and code editors")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(start::build())
        .subcommand(tools::build())
        .subcommand(stream::build())
        .subcommand(editor::claude::build())
        .subcommand(editor::vscode::build())
        .subcommand(editor::cursor::build())
        .subcommand(editor::zed::build())
        .subcommand(
            // Hidden marker; carries no flags and is never meant to be run.
            // Used purely so `handle()` can fingerprint the group as ours.
            Command::new(MARKER_NAME).hide(true).disable_help_flag(true),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Help text reaches every end user of a CLI that embeds brontes, so it
    /// stays in plain English: no team jargon and no contrast frames.
    #[test]
    fn help_text_is_plain_english() {
        const BANNED_WORDS: &[&str] = &[
            "pin", "pins", "pinned", "pinning", "surface", "surfaces", "surfaced", "walk", "walks",
            "walked", "mint", "minted", "land", "lands", "landed", "green", "drain", "drained",
            "seam", "seams",
        ];
        const BANNED_FRAMES: &[&str] = &["instead of", "rather than", ", not "];

        fn check(cmd: &Command, path: &str, bad: &mut Vec<String>) {
            let mut texts: Vec<String> = [cmd.get_about(), cmd.get_long_about()]
                .into_iter()
                .flatten()
                .map(ToString::to_string)
                .collect();
            for arg in cmd.get_arguments() {
                texts.extend(
                    [arg.get_help(), arg.get_long_help()]
                        .into_iter()
                        .flatten()
                        .map(ToString::to_string),
                );
            }
            for text in texts {
                let lower = text.to_lowercase();
                let word = lower
                    .split(|c: char| !c.is_alphanumeric())
                    .find(|w| BANNED_WORDS.contains(w));
                let frame = BANNED_FRAMES.iter().find(|f| lower.contains(*f));
                if let Some(hit) = word.or_else(|| frame.copied()) {
                    bad.push(format!("{path}: {hit:?} in {text:?}"));
                }
            }
            for sub in cmd.get_subcommands() {
                check(sub, &format!("{path} {}", sub.get_name()), bad);
            }
        }

        let mut bad = Vec::new();
        check(&build("mcp"), "mcp", &mut bad);
        assert!(
            bad.is_empty(),
            "help text breaks the plain-English rule:\n{}",
            bad.join("\n")
        );
    }
}
