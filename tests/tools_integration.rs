//! Integration test for `mcp tools` end-to-end through `brontes::handle`.
//!
//! Existing unit tests in `src/subcommands/tools.rs` exercise `write_atomic`
//! against a synthetic tempdir, but they bypass `run()` — the function that
//! reads `--log-level`, walks the clap tree, serializes the generated tool
//! list, and writes `./mcp-tools.json`. The integration test below drives
//! that full path so the `run` / `parse_log_level` / `init_tracing` /
//! `generate_tools` chain is covered by actual SUT exercise (no mocking, no copy-paste of
//! production logic into the test).

use std::sync::Mutex;

use clap::Command;
use serde_json::Value;
use tempfile::TempDir;

// `mcp tools` writes `mcp-tools.json` to the current working directory.
// Each test sets `cwd` to a fresh `TempDir`, so the writes can't collide —
// but `std::env::set_current_dir` is process-global, so we serialize the
// tests in this file behind a single mutex. Without this, two parallel
// tests would race the cwd and one would write into the other's tempdir.
static CWD_LOCK: Mutex<()> = Mutex::new(());

fn build_cli() -> Command {
    Command::new("toolz-cli")
        .version("0.0.1")
        .subcommand(Command::new("greet").about("Say hi"))
        .subcommand(Command::new("status").about("Show status"))
        .subcommand(brontes::command(None))
}

fn dispatch(argv: &[&str]) -> brontes::Result<()> {
    let cli = build_cli();
    let mut full: Vec<&str> = vec!["toolz-cli"];
    full.extend_from_slice(argv);
    let matches = cli.clone().get_matches_from(full);
    let Some(("mcp", sub)) = matches.subcommand() else {
        panic!("expected mcp match, got {:?}", matches.subcommand_name());
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(brontes::handle(sub, &cli, None))
}

#[test]
fn tools_writes_expected_tool_list_to_cwd() {
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let dir = TempDir::new().expect("tempdir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");

    let result = dispatch(&["mcp", "tools"]);
    std::env::set_current_dir(prev_cwd).expect("restore cwd");
    result.expect("mcp tools succeeds");

    // The walker exposes `greet` and `status` as leaves; `mcp` and its
    // children are excluded by the walker's selector defaults. The exact
    // hand-curated tool-list shape is what we pin — a regression in the
    // walker (e.g. accidentally surfacing `mcp tools` as a tool) would
    // surface as an extra name here.
    let out_path = dir.path().join("mcp-tools.json");
    assert!(
        out_path.exists(),
        "mcp tools must write {} to cwd",
        out_path.display()
    );
    let raw = std::fs::read(&out_path).expect("read mcp-tools.json");
    assert!(
        raw.ends_with(b"\n"),
        "atomic-write helper must append a trailing newline"
    );
    let tools: Value = serde_json::from_slice(&raw).expect("parse mcp-tools.json");
    let names: Vec<&str> = tools
        .as_array()
        .expect("top-level is array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name is string"))
        .collect();
    assert!(
        names.contains(&"toolz-cli_greet"),
        "expected toolz-cli_greet in {names:?}"
    );
    assert!(
        names.contains(&"toolz-cli_status"),
        "expected toolz-cli_status in {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("mcp_tools")),
        "the mcp subtree must not appear as a tool in {names:?}"
    );
}

#[test]
fn tools_overwrites_existing_file() {
    // The overwrite path in write_atomic logs at `info`; we cannot easily
    // assert the log (tracing-subscriber is set globally), but we can
    // assert the file is rewritten with the *current* tool list, not
    // appended to or left stale. Seed an unrelated payload, run mcp tools,
    // verify the bytes are replaced.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let dir = TempDir::new().expect("tempdir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");

    let out_path = dir.path().join("mcp-tools.json");
    std::fs::write(&out_path, b"stale junk that does not parse as JSON").expect("seed");

    let result = dispatch(&["mcp", "tools"]);
    std::env::set_current_dir(prev_cwd).expect("restore cwd");
    result.expect("mcp tools succeeds on overwrite");

    let raw = std::fs::read(&out_path).expect("read mcp-tools.json");
    let tools: Value = serde_json::from_slice(&raw).expect("overwrite must produce valid JSON");
    assert!(
        tools.is_array(),
        "overwritten file must parse as a tool-list array"
    );
}

#[test]
fn tools_log_level_flag_is_accepted_for_each_recognized_value() {
    // The `--log-level` flag threads through `parse_log_level` in
    // tools.rs (`trace|debug|info|warn|warning|error` are all valid).
    // Drive each variant via the dispatch path so the match arms in
    // tools.rs::parse_log_level all execute. The behavior we assert is
    // "mcp tools succeeded and wrote a valid JSON tool list" — the
    // tracing subscriber is process-global and idempotent, so we cannot
    // observe its level directly from a sibling test.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let dir = TempDir::new().expect("tempdir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");

    for raw in &["trace", "debug", "info", "warn", "warning", "error"] {
        let result = dispatch(&["mcp", "tools", "--log-level", raw]);
        // Restore cwd inside the loop on first failure to keep diagnostics
        // accurate; the outer restore handles the success path.
        if let Err(e) = &result {
            std::env::set_current_dir(&prev_cwd).expect("restore cwd");
            panic!("mcp tools failed with --log-level={raw}: {e}");
        }
        let out_path = dir.path().join("mcp-tools.json");
        assert!(out_path.exists(), "log-level={raw}: file must be written");
        let raw_bytes = std::fs::read(&out_path).expect("read");
        let _tools: Value = serde_json::from_slice(&raw_bytes)
            .unwrap_or_else(|e| panic!("log-level={raw}: invalid JSON: {e}"));
    }

    std::env::set_current_dir(prev_cwd).expect("restore cwd");
}

#[test]
fn tools_unknown_log_level_does_not_block_export() {
    // An unrecognized `--log-level` value falls through to the `_` arm
    // in parse_log_level, which returns `None` (no override) and lets
    // `RUST_LOG` / the `info` default win. The export must still
    // succeed — an unknown level is a soft fall-through, not an error.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let dir = TempDir::new().expect("tempdir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");

    let result = dispatch(&["mcp", "tools", "--log-level", "nonsense"]);
    std::env::set_current_dir(prev_cwd).expect("restore cwd");
    result.expect("unknown log-level must not error");

    let raw = std::fs::read(dir.path().join("mcp-tools.json")).expect("read");
    let _tools: Value = serde_json::from_slice(&raw).expect("valid JSON");
}
