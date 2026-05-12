//! Positional-args schema description.
//!
//! Builds a description string for the `args` property of a per-tool input
//! schema, following the pattern established by ophis's `selector.go`.

use clap::Command;

/// Build the description string for the `args` property of a per-tool
/// input schema. Always starts with the literal phrase
/// `"Positional command line arguments"`. If the command's usage
/// pattern (from `Command::render_usage()`) has any non-flag positional
/// content after the command path, append a `Usage pattern: …` line
/// describing it.
///
/// # Examples
///
/// For a command with no positionals:
/// ```text
/// Positional command line arguments
/// ```
///
/// For a command with positionals:
/// ```text
/// Positional command line arguments
/// Usage pattern: <name> <path>
/// ```
pub(crate) fn args_description(cmd: &Command) -> String {
    let mut description = String::from("Positional command line arguments");

    // clap's render_usage produces a StyledStr like:
    //   "Usage: my-cli create [OPTIONS] <NAME> <PATH>"
    // We extract everything after the command path and strip options
    // placeholders to get the positional pattern.
    let raw = {
        // `render_usage` takes &mut self in clap 4.5; we clone to keep the
        // caller's `&Command` borrow shape (so `generate_tools` can call us
        // from inside an immutable walk).
        let cmd = &mut cmd.clone();
        cmd.render_usage().to_string()
    };

    if let Some(pattern) = extract_positional_pattern(&raw) {
        description.push_str("\nUsage pattern: ");
        description.push_str(&pattern);
    }

    description
}

/// Strip ANSI escape sequences from a string.
///
/// Handles CSI sequences (ESC `[` ... terminating letter/symbol) to defend
/// against a future clap version emitting styled output through
/// `render_usage().to_string()`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            // Skip until the terminating letter or symbol (@-~).
            chars.next(); // consume '['
            for inner in chars.by_ref() {
                if inner.is_ascii_alphabetic() || matches!(inner, '~' | '@') {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract the positional pattern from a usage string.
///
/// Given a usage string like `"Usage: my-cli sub [OPTIONS] <file>"`,
/// returns `Some("<file>")`. Returns `None` if no positionals are found
/// (only command path and/or options).
fn extract_positional_pattern(raw: &str) -> Option<String> {
    // 0. Strip ANSI escapes (defensive against future clap versions).
    let clean = strip_ansi(raw);

    // 1. Strip leading "Usage: " or "Usage:"
    let after_label = clean
        .trim_start()
        .strip_prefix("Usage: ")
        .or_else(|| clean.trim_start().strip_prefix("Usage:"))
        .unwrap_or(&clean)
        .trim();

    // 2. Strip known placeholders ("[OPTIONS]", "[flags]", "[COMMAND]",
    //    "<COMMAND>", "[SUBCOMMAND]", "<SUBCOMMAND>"). Each may appear with
    //    surrounding whitespace.
    let no_options = after_label
        .replace(" [OPTIONS]", "")
        .replace(" [flags]", "")
        .replace("[OPTIONS]", "")
        .replace("[flags]", "")
        .replace(" [COMMAND]", "")
        .replace("[COMMAND]", "")
        .replace(" <COMMAND>", "")
        .replace("<COMMAND>", "")
        .replace(" [SUBCOMMAND]", "")
        .replace("[SUBCOMMAND]", "")
        .replace(" <SUBCOMMAND>", "")
        .replace("<SUBCOMMAND>", "");
    let no_options = no_options.trim();

    // 3. Find the first `<` or `[` character: that's where positionals start.
    //    Everything before that is the command path, which we discard. If
    //    there's no `<` / `[`, the whole usage was just the command path.
    no_options.find(['<', '[']).and_then(|first_arg_idx| {
        let positionals = no_options[first_arg_idx..].trim();
        if positionals.is_empty() {
            None
        } else {
            Some(positionals.to_owned())
        }
    })
}

#[cfg(test)]
mod tests {
    use clap::{Arg, ArgAction, Command};

    use super::*;

    /// Helper: build a single-command fixture and call `args_description`.
    fn description_for(cmd: &Command) -> String {
        args_description(cmd)
    }

    /// Helper: create a command with a single arg.
    fn cmd_with_arg(arg: Arg) -> Command {
        Command::new("my-cli").arg(arg)
    }

    #[test]
    fn leaf_with_no_positionals_emits_only_first_line() {
        let cmd = cmd_with_arg(
            Arg::new("verbose")
                .long("verbose")
                .action(ArgAction::SetTrue),
        );
        let desc = description_for(&cmd);
        assert_eq!(desc, "Positional command line arguments");
        // Ensure no second line
        assert!(!desc.contains('\n'));
    }

    #[test]
    fn leaf_with_one_positional_emits_pattern() {
        let cmd = cmd_with_arg(Arg::new("name").required(true));
        let desc = description_for(&cmd);
        assert!(
            desc.contains("Positional command line arguments"),
            "must start with canonical phrase"
        );
        assert!(
            desc.contains("Usage pattern:"),
            "must contain 'Usage pattern:' line"
        );
        assert!(
            desc.contains("<name>"),
            "must contain the <name> positional"
        );
    }

    #[test]
    fn leaf_with_multiple_positionals_emits_pattern() {
        let cmd = Command::new("my-cli")
            .arg(Arg::new("name").required(true))
            .arg(Arg::new("path").required(true));
        let desc = description_for(&cmd);
        assert!(
            desc.contains("Positional command line arguments"),
            "must start with canonical phrase"
        );
        assert!(
            desc.contains("Usage pattern:"),
            "must contain 'Usage pattern:' line"
        );
        assert!(
            desc.contains("<name>") && desc.contains("<path>"),
            "must contain both positionals"
        );
    }

    #[test]
    fn description_starts_with_canonical_phrase() {
        // Test with no positionals
        let cmd1 = cmd_with_arg(
            Arg::new("verbose")
                .long("verbose")
                .action(ArgAction::SetTrue),
        );
        assert!(description_for(&cmd1).starts_with("Positional command line arguments"));

        // Test with positionals
        let cmd2 = cmd_with_arg(Arg::new("file").required(true));
        assert!(description_for(&cmd2).starts_with("Positional command line arguments"));
    }

    #[test]
    fn subcommand_path_is_stripped() {
        // Build a 2-deep tree: root -> sub
        let mut parent = Command::new("my-cli")
            .subcommand(Command::new("sub").arg(Arg::new("file").required(true)));
        parent.build();

        // Get the leaf subcommand.
        let sub = parent
            .find_subcommand("sub")
            .expect("sub subcommand not found");
        let desc = description_for(sub);

        // The description's usage pattern should contain only "<file>",
        // NOT "my-cli" or "sub".
        assert!(desc.contains("<file>"), "usage pattern must contain <file>");
        // We expect the usage pattern line to NOT contain the command path.
        // Since clap renders "Usage: my-cli sub <file>", and we strip the
        // command path, we should see only "<file>".
        let usage_line = desc
            .lines()
            .find(|l| l.contains("Usage pattern:"))
            .expect("must have Usage pattern: line");
        assert!(
            !usage_line.contains("my-cli"),
            "usage pattern must not contain root command"
        );
        assert!(
            !usage_line.contains(" sub"),
            "usage pattern must not contain intermediate subcommand"
        );
    }

    #[test]
    fn command_with_only_optional_positionals_emits_pattern() {
        let cmd = cmd_with_arg(Arg::new("optional").required(false));
        let desc = description_for(&cmd);
        assert!(
            desc.contains("Positional command line arguments"),
            "must start with canonical phrase"
        );
        assert!(
            desc.contains("Usage pattern:"),
            "must contain 'Usage pattern:' line"
        );
        // clap renders optional args as [NAME]
        assert!(
            desc.contains("[optional]"),
            "must contain the [optional] positional"
        );
    }

    #[test]
    fn extract_positional_pattern_strips_options_placeholder() {
        // Direct unit test for the helper function.
        let pattern = extract_positional_pattern("Usage: my-cli [OPTIONS] <file> [output]");
        assert_eq!(
            pattern.as_deref(),
            Some("<file> [output]"),
            "must strip command path and [OPTIONS]"
        );
    }

    #[test]
    fn extract_positional_pattern_handles_flags_placeholder() {
        // Test with [flags] placeholder (older or manual override).
        let pattern = extract_positional_pattern("Usage: my-cli [flags] <name>");
        assert_eq!(
            pattern.as_deref(),
            Some("<name>"),
            "must handle [flags] placeholder"
        );
    }

    #[test]
    fn extract_positional_pattern_returns_none_for_no_positionals() {
        let pattern = extract_positional_pattern("Usage: my-cli [OPTIONS]");
        assert_eq!(pattern, None, "must return None when no positionals exist");
    }

    #[test]
    fn extract_positional_pattern_returns_none_for_empty_usage() {
        let pattern = extract_positional_pattern("");
        assert_eq!(pattern, None, "must handle empty input gracefully");
    }

    #[test]
    fn parent_with_positionals_and_subcommand_strips_command_placeholder() {
        // A parent command with its own required positional AND subcommands.
        // clap will render: "Usage: my-cli <NAME> [COMMAND]".
        // We want the Usage pattern to be `<NAME>`, NOT `<NAME> [COMMAND]`.
        let mut parent = Command::new("my-cli")
            .arg(Arg::new("name").required(true))
            .subcommand(Command::new("sub"));
        parent.build();
        let leaf = parent.clone(); // The parent itself is the command under test
        let desc = description_for(&leaf);
        assert!(
            desc.contains("Usage pattern: <name>"),
            "expected `Usage pattern: <name>`, got: {desc}"
        );
        assert!(
            !desc.contains("COMMAND"),
            "subcommand placeholder must be stripped, got: {desc}"
        );
    }

    #[test]
    fn extract_positional_pattern_strips_ansi_escapes() {
        // Defensive: if a future clap version emits ANSI escapes through
        // render_usage().to_string(), the extracted pattern must not carry
        // them through.
        let with_ansi = "\u{1b}[1mUsage:\u{1b}[0m my-cli [OPTIONS] \u{1b}[33m<NAME>\u{1b}[0m";
        let pattern = extract_positional_pattern(with_ansi);
        let p = pattern.expect("should extract pattern");
        assert!(
            !p.contains('\u{1b}'),
            "ANSI escape leaked into pattern: {p:?}"
        );
        assert!(p.contains("<NAME>"), "actual pattern content lost: {p:?}");
    }
}
