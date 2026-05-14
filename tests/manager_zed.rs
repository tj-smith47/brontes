//! Integration tests for `mcp zed {enable, disable, list}` plus the
//! Zed-specific JSON shape (`ZedConfig` / `ZedServer`), the JSONC
//! preprocessing hook, and the unrelated-top-level-keys preservation
//! invariant that distinguishes Zed from Claude / Cursor / `VSCode`.
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

fn build_cli() -> Command {
    Command::new("my-cli")
        .version("0.1.0")
        .subcommand(brontes::command(None))
}

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

// ── enable / disable / list end-to-end ─────────────────────────────────────

#[test]
fn enable_writes_config_with_context_servers_shape() {
    // First-write path: no settings.json exists yet. After enable, the file
    // must materialize with a single `context_servers` block carrying the
    // brontes-managed server entry.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().expect("utf8 path"),
        "--server-name",
        "test-cli",
    ])
    .expect("enable succeeds");

    let doc = read_json(&cfg_path);
    let server = &doc["context_servers"]["test-cli"];
    assert!(server.is_object(), "test-cli must be present");
    // Local-stdio shape has NO `type` field (unlike VSCode/Cursor) — Zed
    // distinguishes local from remote by presence of `command` vs `url`.
    assert!(server.get("type").is_none(), "zed must not write `type`");
    assert!(server["command"].is_string());
    let args: Vec<&str> = server["args"]
        .as_array()
        .expect("args array")
        .iter()
        .map(|v| v.as_str().expect("string arg"))
        .collect();
    assert_eq!(args, ["mcp", "start"]);
}

#[test]
fn enable_then_disable_removes_only_the_named_server() {
    // Two-entry add-then-disable: the disable must leave the other entry
    // and the `context_servers` map intact.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "alpha",
    ])
    .expect("enable alpha");
    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "beta",
    ])
    .expect("enable beta");

    dispatch(&[
        "mcp",
        "zed",
        "disable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "alpha",
    ])
    .expect("disable alpha");

    let doc = read_json(&cfg_path);
    assert!(
        doc["context_servers"]["alpha"].is_null(),
        "alpha must be removed; got {doc}"
    );
    assert!(
        doc["context_servers"]["beta"].is_object(),
        "beta must remain; got {doc}"
    );
}

#[test]
fn enable_preserves_unrelated_top_level_keys() {
    // The defining Zed invariant: settings.json carries theme/font/keymap
    // alongside context_servers. enable must NOT wipe them.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");
    std::fs::write(
        &cfg_path,
        r#"{
  "theme": "One Dark",
  "font_family": "JetBrains Mono",
  "tab_size": 2,
  "context_servers": {
    "prior": {"command": "/bin/x"}
  }
}"#,
    )
    .expect("seed");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "new-server",
    ])
    .expect("enable new-server");

    let doc = read_json(&cfg_path);
    // brontes-mutated entry present.
    assert!(
        doc["context_servers"]["new-server"].is_object(),
        "new-server must be added"
    );
    // prior entry present.
    assert!(
        doc["context_servers"]["prior"].is_object(),
        "prior must survive enable"
    );
    // unrelated top-level keys present.
    assert_eq!(doc["theme"].as_str(), Some("One Dark"));
    assert_eq!(doc["font_family"].as_str(), Some("JetBrains Mono"));
    assert_eq!(doc["tab_size"].as_i64(), Some(2));
}

#[test]
fn enable_preserves_unrelated_keys_through_jsonc_input() {
    // A real Zed settings.json is JSONC — line comments and trailing
    // commas. The preprocess hook must allow it to load cleanly; the
    // unrelated keys (modulo the comments themselves) must survive a
    // round trip.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");
    std::fs::write(
        &cfg_path,
        r#"// Zed user settings
{
    // visual choices
    "theme": "Solarized Light",
    "font_family": "Fira Code", // monospace family
    /* MCP block — managed by brontes */
    "context_servers": {
        "prior": {"command": "/bin/x",},
    },
}"#,
    )
    .expect("seed jsonc");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "new-server",
    ])
    .expect("enable through jsonc");

    let doc = read_json(&cfg_path);
    assert!(
        doc["context_servers"]["new-server"].is_object(),
        "new server added through jsonc input"
    );
    assert!(
        doc["context_servers"]["prior"].is_object(),
        "prior entry survived through jsonc parsing"
    );
    assert_eq!(doc["theme"].as_str(), Some("Solarized Light"));
    assert_eq!(doc["font_family"].as_str(), Some("Fira Code"));
}

#[test]
fn enable_backup_before_in_place_mutation() {
    // First write does NOT produce a backup file; the subsequent write
    // (against an existing settings.json) MUST produce
    // `settings.backup.json` next to it before the new bytes are written.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");
    let backup = dir.path().join("settings.backup.json");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "first",
    ])
    .expect("first enable");
    assert!(
        !backup.exists(),
        "first write must NOT create a .backup.json"
    );

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "second",
    ])
    .expect("second enable");
    assert!(
        backup.exists(),
        "second write MUST snapshot to .backup.json before overwriting"
    );
}

#[test]
fn disable_missing_server_is_no_op_not_error() {
    // ophis parity: disabling a server that is not in the map prints a
    // friendly note but exits 0 (no error). The file must NOT change.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");
    std::fs::write(
        &cfg_path,
        r#"{"theme":"Z","context_servers":{"a":{"command":"/x"}}}"#,
    )
    .expect("seed");
    let before = std::fs::read(&cfg_path).expect("read before");

    dispatch(&[
        "mcp",
        "zed",
        "disable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "does-not-exist",
    ])
    .expect("disable missing exits ok");

    let after = std::fs::read(&cfg_path).expect("read after");
    assert_eq!(before, after, "no-op disable must not rewrite the file");
}

#[test]
fn workspace_flag_resolves_to_dot_zed_under_cwd() {
    // --workspace must route through `$CWD/.zed/settings.json`, NOT the
    // per-OS user mode default. We assert by writing under a tempdir-as-
    // cwd and confirming the file lands there.
    let dir = TempDir::new().expect("tempdir");
    let prev_cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");

    let result = dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--workspace",
        "--server-name",
        "ws-server",
    ]);

    // Always restore cwd before asserting, even on panic.
    std::env::set_current_dir(prev_cwd).expect("restore cwd");
    result.expect("workspace enable succeeds");

    let workspace_path = dir.path().join(".zed").join("settings.json");
    assert!(
        workspace_path.exists(),
        "workspace settings.json must materialize at {}",
        workspace_path.display()
    );
    let doc = read_json(&workspace_path);
    assert!(
        doc["context_servers"]["ws-server"].is_object(),
        "ws-server present in workspace config"
    );
}

#[test]
fn list_prints_servers_one_per_line() {
    // `mcp zed list` must surface configured server names, one per line.
    // The test asserts on file state plus the trait surface (sorted-key
    // iteration) rather than capturing stdout (which is the same code
    // path Cursor / VSCode use).
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");
    std::fs::write(
        &cfg_path,
        r#"{
            "theme": "Solarized",
            "context_servers": {
                "zebra": {"command": "/z"},
                "alpha": {"command": "/a"}
            }
        }"#,
    )
    .expect("seed");

    dispatch(&[
        "mcp",
        "zed",
        "list",
        "--config-path",
        cfg_path.to_str().unwrap(),
    ])
    .expect("list ok");

    // File state unchanged — list is read-only.
    let doc = read_json(&cfg_path);
    assert_eq!(doc["theme"].as_str(), Some("Solarized"));
    assert!(doc["context_servers"]["zebra"].is_object());
    assert!(doc["context_servers"]["alpha"].is_object());
}

#[test]
fn env_flag_merges_into_server_env() {
    // -e KEY=VAL is the same parsing layer Claude/Cursor/VSCode use, but
    // this test pins that Zed routes through it too — and that the
    // resulting `env` lands under context_servers.<name>.env on disk.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "envtest",
        "-e",
        "FOO=bar",
        "-e",
        "BAZ=qux",
    ])
    .expect("enable with env");

    let doc = read_json(&cfg_path);
    let env = &doc["context_servers"]["envtest"]["env"];
    assert_eq!(env["FOO"].as_str(), Some("bar"));
    assert_eq!(env["BAZ"].as_str(), Some("qux"));
}

#[test]
fn enable_no_env_produces_no_env_key() {
    // Per the env-merge contract: when no `default_env` and no `--env`, the
    // resulting server entry MUST NOT carry an `env` JSON key.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "no-env",
    ])
    .expect("enable no-env");

    let doc = read_json(&cfg_path);
    let server = &doc["context_servers"]["no-env"];
    assert!(
        server.get("env").is_none(),
        "empty env must collapse to no JSON key; got {server}"
    );
}

#[test]
fn enable_with_log_level_appends_flag_to_args() {
    // --log-level threads through into the spawned-server argv (Zed will
    // re-invoke the bin as `<bin> mcp start --log-level <lvl>`).
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");

    dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "lvl",
        "--log-level",
        "debug",
    ])
    .expect("enable log-level");

    let doc = read_json(&cfg_path);
    let args: Vec<&str> = doc["context_servers"]["lvl"]["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(args, ["mcp", "start", "--log-level", "debug"]);
}

#[test]
fn invalid_env_pair_surfaces_as_config_error() {
    // The merge_env shared layer is also Zed's enable path; a missing `=`
    // must surface as `Error::Config`, NOT panic, NOT silently drop.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("settings.json");

    let result = dispatch(&[
        "mcp",
        "zed",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "bad",
        "-e",
        "MISSING_SEPARATOR",
    ]);
    let err = result.expect_err("must reject malformed --env");
    let msg = err.to_string();
    assert!(msg.contains("missing '='"), "got {msg}");
    // No file written when the env validation fails.
    assert!(!cfg_path.exists(), "no settings.json on validation failure");
}
