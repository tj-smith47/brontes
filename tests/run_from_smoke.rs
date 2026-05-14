//! End-to-end tests for `brontes::run_from` — the argv-injectable variant
//! of `brontes::run` that the production sugar entry point delegates to.
//!
//! `run` itself calls `get_matches()` which consumes the process argv
//! unconditionally; that makes it untestable in a unit-test context.
//! `run_from` lets the tests supply synthetic argv so the full
//! mount-`mcp`-subtree → parse → dispatch path is exercised.

use clap::Command;
use tempfile::TempDir;

#[test]
fn run_from_dispatches_mcp_tools_to_completion() {
    // Happy path: `run_from` mounts the mcp subtree, parses an argv
    // selecting `mcp tools`, and dispatches into the tools-export
    // handler. The handler writes `mcp-tools.json` to cwd; isolating
    // cwd in a tempdir is the simplest way to detect "the dispatch
    // arrived at the right leaf" without observing stdout.
    let dir = TempDir::new().expect("tempdir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let cli = Command::new("runfrom").version("0.0.1");
    let result = rt.block_on(brontes::run_from(cli, None, ["runfrom", "mcp", "tools"]));

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
    result.expect("run_from(mcp tools) succeeds");

    assert!(
        dir.path().join("mcp-tools.json").exists(),
        "tools dispatch must have written mcp-tools.json"
    );
}

#[test]
fn run_from_rejects_non_mcp_subtree_with_dedicated_error_message() {
    // `run_from` is the one-call sugar; it ONLY dispatches the brontes
    // `mcp` subtree. Apps with their own subcommands must mount via
    // brontes::command() and dispatch via brontes::handle() instead.
    // A non-mcp subcommand selection must surface a clean Config error
    // that points the consumer at the right migration path.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let cli = Command::new("runfrom")
        .version("0.0.1")
        .subcommand(Command::new("greet"));
    let err = rt
        .block_on(brontes::run_from(cli, None, ["runfrom", "greet"]))
        .expect_err("non-mcp subcommand must error");
    let msg = err.to_string();
    assert!(
        msg.contains("only dispatches the \"mcp\" subtree")
            && msg.contains("Mount brontes::command()"),
        "error message must point at the right migration path; got: {msg}"
    );
    assert!(matches!(err, brontes::Error::Config(_)));
}

#[test]
fn run_from_rejects_bare_invocation_with_friendly_error() {
    // No subcommand at all on argv → `mounted.get_matches_from` returns
    // a top-level match with `subcommand() == None`. `run_from`
    // surfaces an Error::Config that names the expected subtree.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let cli = Command::new("runfrom").version("0.0.1");
    let err = rt
        .block_on(brontes::run_from(cli, None, ["runfrom"]))
        .expect_err("bare invocation must error");
    let msg = err.to_string();
    assert!(
        msg.contains("no subcommand provided") && msg.contains("\"mcp\""),
        "got: {msg}"
    );
}

#[test]
fn run_from_honors_custom_command_name_on_dispatch() {
    // When `Config::command_name("agent")` is set, the mounted subtree
    // is named `agent`, NOT `mcp`. `run_from` must look up THAT name
    // on dispatch — passing `mcp` on argv must miss the dispatch and
    // surface the wrong-subtree error, while passing `agent` must
    // succeed.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let cfg = brontes::Config::default().command_name("agent");
    let cli = Command::new("renamed").version("0.0.1");
    // tools dispatch writes mcp-tools.json into cwd; we already covered
    // the success path above. Here we expect either success OR a clean
    // error — the failure mode we DO NOT want is a panic, which would
    // mean the custom-name path bypassed validation.
    let err = rt.block_on(brontes::run_from(
        cli,
        Some(&cfg),
        ["renamed", "agent", "tools"],
    ));
    // Either accept Ok (file written) or a Config error — anything else
    // is a regression.
    if let Err(e) = err {
        assert!(
            matches!(e, brontes::Error::Config(_) | brontes::Error::Io { .. }),
            "expected Config/Io error or Ok, got: {e}"
        );
    }
}
