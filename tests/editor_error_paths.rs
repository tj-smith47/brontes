//! Error-path coverage for the four editor subcommand `run()` dispatchers
//! (`mcp claude/cursor/vscode/zed`).
//!
//! Each editor's `run()` carries two `Err` arms the docs enumerate:
//! `unknown <editor> subcommand` and `no <editor> subcommand selected`.
//! The successful enable/disable/list paths are well-tested by the
//! `manager_{claude,cursor,vscode,zed}` integration crates; this file
//! pins the failure messages and verifies they are differentiated per-
//! editor (e.g. a vscode dispatch error must not say "claude").
//!
//! To reach those arms we build a parallel `clap::Command` that mirrors
//! the production subcommand shape but turns off `subcommand_required`,
//! letting us parse an unknown or absent leaf through to `run()`.

use clap::{Arg, ArgAction, Command};

use brontes::Error;

/// Build a stand-in `<editor>` Command whose leaves match the production
/// shape (so any flag the dispatcher reads exists at parse time), but
/// without `subcommand_required(true)` — that's what lets an unknown or
/// missing leaf parse through to the dispatcher's `Err` arms.
fn permissive_editor_root(name: &str) -> Command {
    Command::new(name.to_string())
        .subcommand_required(false)
        .arg_required_else_help(false)
        .subcommand(
            Command::new("enable")
                .arg(Arg::new("config-path").long("config-path"))
                .arg(Arg::new("server-name").long("server-name"))
                .arg(Arg::new("env").long("env").action(ArgAction::Append))
                .arg(Arg::new("log-level").long("log-level"))
                .arg(
                    Arg::new("workspace")
                        .long("workspace")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(Command::new("disable"))
        .subcommand(Command::new("list"))
        .subcommand(Command::new("madeup-leaf"))
}

fn parse_with_leaf(editor: &str, argv: &[&str]) -> clap::ArgMatches {
    let cmd = permissive_editor_root(editor);
    let mut full: Vec<&str> = vec![editor];
    full.extend_from_slice(argv);
    cmd.try_get_matches_from(full).expect("parses")
}

// ── claude ────────────────────────────────────────────────────────────────

#[test]
fn claude_run_rejects_unknown_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("claude", &["madeup-leaf"]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let result = brontes::__test_internal::editor_run("claude", &matches, &cli);
    let err = result.expect_err("unknown leaf must error");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown mcp claude subcommand") && msg.contains("madeup-leaf"),
        "got: {msg}"
    );
    assert!(matches!(err, Error::Config(_)));
}

#[test]
fn claude_run_rejects_missing_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("claude", &[]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let result = brontes::__test_internal::editor_run("claude", &matches, &cli);
    let err = result.expect_err("missing leaf must error");
    assert!(
        err.to_string()
            .contains("no mcp claude subcommand selected")
    );
}

// ── cursor ────────────────────────────────────────────────────────────────

#[test]
fn cursor_run_rejects_unknown_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("cursor", &["madeup-leaf"]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("cursor", &matches, &cli)
        .expect_err("unknown leaf must error");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown mcp cursor subcommand") && msg.contains("madeup-leaf"),
        "got: {msg}"
    );
}

#[test]
fn cursor_run_rejects_missing_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("cursor", &[]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("cursor", &matches, &cli)
        .expect_err("missing leaf must error");
    assert!(
        err.to_string()
            .contains("no mcp cursor subcommand selected")
    );
}

// ── vscode ────────────────────────────────────────────────────────────────

#[test]
fn vscode_run_rejects_unknown_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("vscode", &["madeup-leaf"]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("vscode", &matches, &cli)
        .expect_err("unknown leaf must error");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown mcp vscode subcommand") && msg.contains("madeup-leaf"),
        "got: {msg}"
    );
}

#[test]
fn vscode_run_rejects_missing_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("vscode", &[]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("vscode", &matches, &cli)
        .expect_err("missing leaf must error");
    assert!(
        err.to_string()
            .contains("no mcp vscode subcommand selected")
    );
}

// ── zed ───────────────────────────────────────────────────────────────────

#[test]
fn zed_run_rejects_unknown_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("zed", &["madeup-leaf"]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("zed", &matches, &cli)
        .expect_err("unknown leaf must error");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown mcp zed subcommand") && msg.contains("madeup-leaf"),
        "got: {msg}"
    );
}

#[test]
fn zed_run_rejects_missing_leaf_with_editor_specific_message() {
    let matches = parse_with_leaf("zed", &[]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("zed", &matches, &cli)
        .expect_err("missing leaf must error");
    assert!(err.to_string().contains("no mcp zed subcommand selected"));
}

// ── unknown-editor guard ──────────────────────────────────────────────────

#[test]
fn editor_run_helper_rejects_unrecognized_editor_name() {
    // The `editor_run` test helper accepts only the four known editor
    // names; an unknown name must surface as Error::Config with a
    // helper-specific message (the underlying production code never
    // sees a dispatch). This guards against a typo in a future test
    // file silently passing because the helper accepted it.
    let matches = parse_with_leaf("madeup", &["enable"]);
    let cli = Command::new("hostc").subcommand(brontes::command(None));
    let err = brontes::__test_internal::editor_run("madeup-editor", &matches, &cli)
        .expect_err("unknown editor must error");
    let msg = err.to_string();
    assert!(
        msg.contains("editor_run: unknown editor") && msg.contains("madeup-editor"),
        "got: {msg}"
    );
}
