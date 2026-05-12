//! Integration tests for `mcp claude {enable, disable, list}` plus the
//! shared editor-manager infrastructure (`Manager<ClaudeConfig>`).
//!
//! Every test that writes to the filesystem isolates inside a
//! `tempfile::TempDir` — no test touches the real `$HOME`. The CLI-driven
//! tests construct the brontes `mcp` subtree, parse a synthetic argv, and
//! dispatch via [`brontes::handle`] so the real production code path is
//! exercised end-to-end.

use std::path::PathBuf;

use clap::Command;
use serde_json::Value;
use tempfile::TempDir;

/// Build a minimal CLI with `brontes::command(None)` mounted under it.
fn build_cli() -> Command {
    Command::new("my-cli")
        .version("0.1.0")
        .subcommand(brontes::command(None))
}

/// Dispatch one synthetic invocation of the CLI through `brontes::handle`.
///
/// `argv` is everything after the binary name; we always prepend `"my-cli"`
/// because clap's `get_matches_from` expects argv[0].
fn dispatch(argv: &[&str]) -> brontes::Result<()> {
    let cli = build_cli();
    let mut full: Vec<&str> = vec!["my-cli"];
    full.extend_from_slice(argv);
    let matches = cli.clone().get_matches_from(full);
    let Some(("mcp", sub)) = matches.subcommand() else {
        panic!(
            "expected `mcp` subcommand match, got {:?}",
            matches.subcommand_name()
        );
    };
    // `handle` is async, so spin up a single-thread runtime for the test.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(brontes::handle(sub, &cli, None))
}

fn read_json(path: &PathBuf) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// ── enable / disable / list, end-to-end via dispatch ───────────────────────

#[test]
fn enable_writes_config_with_expected_shape() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().expect("utf8 path"),
        "--server-name",
        "test-cli",
    ])
    .expect("enable succeeds");

    let doc = read_json(&cfg_path);
    let server = &doc["mcpServers"]["test-cli"];
    assert!(server.is_object(), "test-cli must be present");
    assert!(server["command"].is_string());
    let args: Vec<&str> = server["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    assert_eq!(args, vec!["mcp", "start"]);
    assert!(server.get("env").is_none(), "no env -> key omitted");
}

#[test]
fn enable_includes_log_level_in_args() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
        "--log-level",
        "debug",
    ])
    .expect("enable succeeds");

    let doc = read_json(&cfg_path);
    let args: Vec<&str> = doc["mcpServers"]["test-cli"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(args, vec!["mcp", "start", "--log-level", "debug"]);
}

#[test]
fn enable_with_env_writes_env_block() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
        "--env",
        "PATH=/usr/local/bin",
        "--env",
        "DEBUG=1",
    ])
    .expect("enable succeeds");

    let doc = read_json(&cfg_path);
    let env = &doc["mcpServers"]["test-cli"]["env"];
    assert_eq!(env["PATH"].as_str(), Some("/usr/local/bin"));
    assert_eq!(env["DEBUG"].as_str(), Some("1"));
}

#[test]
fn enable_without_env_omits_env_key() {
    // ophis defaultenv_test.go scenario: nil DefaultEnv + no --env -> no env key.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("enable");
    let doc = read_json(&cfg_path);
    assert!(
        doc["mcpServers"]["test-cli"].get("env").is_none(),
        "no env key when DefaultEnv is empty and --env unset"
    );
}

#[test]
fn enable_overwrites_existing_server_quietly() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    // First enable.
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("first enable");

    // Second enable: should print a warning to stdout but still succeed.
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
        "--log-level",
        "info",
    ])
    .expect("second enable");

    let doc = read_json(&cfg_path);
    let args: Vec<&str> = doc["mcpServers"]["test-cli"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        args.contains(&"--log-level"),
        "second enable must have updated the args"
    );
}

#[test]
fn disable_removes_existing_server() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("enable");

    dispatch(&[
        "mcp",
        "claude",
        "disable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("disable");

    let doc = read_json(&cfg_path);
    assert!(
        doc["mcpServers"].get("test-cli").is_none(),
        "test-cli must be removed"
    );
}

#[test]
fn disable_missing_server_is_ok() {
    // Per PLAN §11 #5: disable on a missing server name prints a warning
    // and returns Ok(()) — not an error.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    // Pre-create the file with empty mcpServers so the file exists but
    // the target name is not present (no warning about missing FILE,
    // only the missing SERVER warning).
    std::fs::write(&cfg_path, b"{\"mcpServers\":{}}").expect("seed");

    dispatch(&[
        "mcp",
        "claude",
        "disable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "not-there",
    ])
    .expect("disable on missing name returns Ok(())");
}

#[test]
fn list_on_missing_file_surfaces_path() {
    // No filesystem mutation: list against a path that does not exist.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    dispatch(&[
        "mcp",
        "claude",
        "list",
        "--config-path",
        cfg_path.to_str().unwrap(),
    ])
    .expect("list on missing file is Ok");
    // No file was created.
    assert!(!cfg_path.exists(), "list must not create the config file");
}

#[test]
fn list_shows_configured_servers() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "alpha",
    ])
    .expect("enable alpha");
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "beta",
    ])
    .expect("enable beta");

    // list itself is stdout-only; rely on the on-disk shape for assertions.
    let doc = read_json(&cfg_path);
    let names: Vec<&str> = doc["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}

// ── backup-before-write semantics ─────────────────────────────────────────

#[test]
fn save_creates_backup_when_file_exists() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");

    // First enable creates the file (no backup yet).
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "first",
    ])
    .expect("first enable");
    let backup_path = dir.path().join("claude_desktop_config.backup.json");
    assert!(
        !backup_path.exists(),
        "first write must not produce a backup"
    );

    // Capture the on-disk bytes so we can assert the backup mirrors the
    // pre-second-write state.
    let pre_second = std::fs::read(&cfg_path).expect("read pre-second");

    // Second enable triggers backup of the existing file.
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "second",
    ])
    .expect("second enable");

    assert!(backup_path.exists(), "second write must produce a backup");
    let backup_bytes = std::fs::read(&backup_path).expect("read backup");
    assert_eq!(
        backup_bytes, pre_second,
        "backup must mirror the pre-second-write state"
    );
}

#[test]
fn save_no_backup_on_missing_primary() {
    // ophis manager.go:163-167: backup is a no-op when the primary file
    // does not yet exist. Verify by enabling once into a fresh tempdir.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");
    dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "fresh",
    ])
    .expect("enable");

    assert!(cfg_path.exists());
    let backup_path = dir.path().join("claude_desktop_config.backup.json");
    assert!(
        !backup_path.exists(),
        "first write must NOT create a backup"
    );
}

// ── load semantics ────────────────────────────────────────────────────────

#[test]
fn list_on_invalid_json_returns_parse_error() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");
    std::fs::write(&cfg_path, b"not valid json").expect("seed");

    let result = dispatch(&[
        "mcp",
        "claude",
        "list",
        "--config-path",
        cfg_path.to_str().unwrap(),
    ]);
    let err = result.expect_err("must fail on invalid JSON");
    let msg = err.to_string();
    assert!(
        msg.contains("editor config: parse failed"),
        "unexpected error: {msg}"
    );
}

#[test]
fn enable_rejects_malformed_env_flag() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("claude_desktop_config.json");
    let result = dispatch(&[
        "mcp",
        "claude",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test",
        "--env",
        "NO_EQUALS_SIGN",
    ]);
    let err = result.expect_err("malformed --env must error");
    let msg = err.to_string();
    assert!(
        msg.contains("missing '='"),
        "unexpected error message: {msg}"
    );
    assert!(
        !cfg_path.exists(),
        "config must NOT be written on env-flag rejection"
    );
}
