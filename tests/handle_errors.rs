//! Error-path coverage for `brontes::handle`.
//!
//! Every successful dispatch is well-tested in the other integration
//! crates (`manager_zed.rs`, `manager_cursor.rs`, etc.). This file pins
//! the failure modes the documented `# Errors` block in `handle`'s
//! rustdoc enumerates, so a refactor that drops one of those guards
//! breaks the test loudly:
//!
//! - Explicit-empty `Config::command_name` — caught BEFORE the lookup.
//! - The configured group is missing from the CLI (forgot to mount).
//! - The configured group exists but was not minted by brontes (sibling
//!   collision with a same-named user subcommand).
//! - The dispatched leaf is the hidden internal marker.
//! - The dispatched leaf is some unknown name.
//! - No subcommand was selected (`subcommand_required(false)` bypass).

use clap::{ArgAction, Command};

use brontes::{Config, Error};

fn build_cli_with_mcp() -> Command {
    Command::new("hostc")
        .version("0.0.1")
        .subcommand(Command::new("noop"))
        .subcommand(brontes::command(None))
}

fn run_async<F: std::future::Future>(fut: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(fut)
}

#[test]
fn handle_rejects_explicit_empty_command_name_with_config_error() {
    // Setting `Config::command_name("")` would have silently fallen back
    // to "mcp" inside `command()` — so without an explicit guard the
    // user's typo would never surface. handle() catches it as Error::Config
    // BEFORE attempting the find_subcommand lookup.
    let cli = build_cli_with_mcp();
    let matches = cli
        .clone()
        .try_get_matches_from(["hostc", "mcp", "tools"])
        .expect("parses");
    let sub = matches.subcommand_matches("mcp").expect("mcp matches");

    let cfg = Config::default().command_name("");
    let result = run_async(brontes::handle(sub, &cli, Some(&cfg)));
    let err = result.expect_err("must reject empty command_name");
    let msg = err.to_string();
    assert!(msg.contains("command_name must not be empty"), "got: {msg}");
    assert!(matches!(err, Error::Config(_)));
}

#[test]
fn handle_rejects_when_mcp_subtree_not_mounted() {
    // Caller asks handle to dispatch the "mcp" group but built a CLI
    // that never mounted brontes::command(). Pin the friendly error.
    let cli_with_mcp = build_cli_with_mcp();
    let matches = cli_with_mcp
        .try_get_matches_from(["hostc", "mcp", "tools"])
        .expect("parses");
    let sub = matches.subcommand_matches("mcp").expect("mcp matches");

    // Now hand handle() a DIFFERENT CLI tree without the mcp mount.
    let cli_missing_mount = Command::new("hostc")
        .version("0.0.1")
        .subcommand(Command::new("noop"));
    let result = run_async(brontes::handle(sub, &cli_missing_mount, None));
    let err = result.expect_err("must reject missing mount");
    let msg = err.to_string();
    assert!(
        msg.contains("did you forget to mount brontes::command"),
        "got: {msg}"
    );
}

#[test]
fn handle_rejects_sibling_collision_with_user_owned_mcp_subcommand() {
    // The user pre-registered their own "mcp" subcommand (no brontes
    // marker child). handle() must refuse to dispatch into it.
    let cli = Command::new("hostc")
        .version("0.0.1")
        .subcommand(Command::new("mcp").subcommand(Command::new("install")));

    // Synthesize an ArgMatches for the user-owned `mcp install`.
    let matches = cli
        .clone()
        .try_get_matches_from(["hostc", "mcp", "install"])
        .expect("parses");
    let sub = matches.subcommand_matches("mcp").expect("mcp matches");

    let result = run_async(brontes::handle(sub, &cli, None));
    let err = result.expect_err("must reject sibling collision");
    let msg = err.to_string();
    assert!(
        msg.contains("not minted by brontes") && msg.contains("sibling collision"),
        "got: {msg}"
    );
}

#[test]
fn handle_rejects_internal_marker_leaf_without_leaking_marker_name() {
    // The hidden `__brontes_internal_marker` subcommand parses cleanly
    // through the clap surface but must error at dispatch — and the
    // error message must NOT leak the literal marker name (it's a
    // private implementation detail).
    let cli = build_cli_with_mcp();
    let matches = cli
        .clone()
        .try_get_matches_from(["hostc", "mcp", "__brontes_internal_marker"])
        .expect("hidden subcommand still parses");
    let sub = matches.subcommand_matches("mcp").expect("mcp matches");

    let result = run_async(brontes::handle(sub, &cli, None));
    let err = result.expect_err("internal marker must not be runnable");
    let msg = err.to_string();
    assert!(
        msg.contains("internal marker") && !msg.contains("__brontes_internal_marker"),
        "marker name must not leak; got: {msg}"
    );
}

#[test]
fn handle_rejects_no_leaf_selected_with_friendly_error() {
    // The brontes-minted `mcp` group sets `subcommand_required(true)`,
    // so a normal argv that names just `mcp` is rejected by clap before
    // handle() runs. To exercise the `None` arm in handle, we synthesize
    // a stand-in match by parsing through a CLI whose `mcp` child does
    // NOT require a subcommand.
    let cli = Command::new("hostc")
        .version("0.0.1")
        // Replicate brontes::command()'s outer shape (the marker subcommand)
        // so the marker check passes, but flip `subcommand_required` off so
        // a bare `mcp` parses without a leaf.
        .subcommand(
            Command::new("mcp")
                .subcommand(Command::new("__brontes_internal_marker").hide(true))
                .subcommand_required(false)
                .arg_required_else_help(false)
                .arg(
                    clap::Arg::new("noise")
                        .long("noise")
                        .action(ArgAction::SetTrue),
                ),
        );
    let matches = cli
        .clone()
        .try_get_matches_from(["hostc", "mcp"])
        .expect("parses without leaf");
    let sub = matches.subcommand_matches("mcp").expect("mcp matches");

    let result = run_async(brontes::handle(sub, &cli, None));
    let err = result.expect_err("no leaf must error");
    let msg = err.to_string();
    assert!(msg.contains("no mcp subcommand selected"), "got: {msg}");
}

#[test]
fn handle_rejects_unknown_leaf_with_friendly_error() {
    // Same shape as above, but with an actual unknown leaf rather than
    // none. clap's `subcommand_required(false)` lets `mcp totallymade`
    // through to handle(), which then errors.
    let cli = Command::new("hostc").version("0.0.1").subcommand(
        Command::new("mcp")
            .subcommand(Command::new("__brontes_internal_marker").hide(true))
            .subcommand(Command::new("totallymade"))
            .subcommand_required(false)
            .arg_required_else_help(false),
    );
    let matches = cli
        .clone()
        .try_get_matches_from(["hostc", "mcp", "totallymade"])
        .expect("parses");
    let sub = matches.subcommand_matches("mcp").expect("mcp matches");

    let result = run_async(brontes::handle(sub, &cli, None));
    let err = result.expect_err("unknown leaf must error");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown mcp subcommand") && msg.contains("totallymade"),
        "got: {msg}"
    );
}
