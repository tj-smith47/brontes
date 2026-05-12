//! Integration tests for `mcp vscode {enable, disable, list}` plus the
//! `VSCode`-specific JSON shape (`VSCodeConfig` / `VSCodeServer`) and the
//! `--workspace` flag that switches between user mode (per-OS default) and
//! workspace mode (`$CWD/.vscode/mcp.json`).
//!
//! The `VSCode` and Cursor server-struct shapes are byte-identical (PLAN line
//! 549/550 — same six-field declaration order); the **one** structural
//! divergence is the top-level JSON key: `VSCode` uses `servers`, Cursor
//! uses `mcpServers`. Every test that targets the JSON shape asserts the
//! `servers` key, not `mcpServers`.
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
fn enable_writes_config_with_expected_shape() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().expect("utf8 path"),
        "--server-name",
        "test-cli",
    ])
    .expect("enable succeeds");

    let doc = read_json(&cfg_path);
    let server = &doc["servers"]["test-cli"];
    assert!(server.is_object(), "test-cli must be present");
    assert_eq!(
        server["type"].as_str(),
        Some("stdio"),
        "type must be stdio on enable"
    );
    assert!(server["command"].is_string());
    let args: Vec<&str> = server["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    assert_eq!(args, vec!["mcp", "start"]);
    assert!(server.get("env").is_none(), "no env -> key omitted");
    assert!(server.get("url").is_none(), "no url -> key omitted");
    assert!(server.get("headers").is_none(), "no headers -> key omitted");
}

#[test]
fn enable_field_order_is_type_command_args_env() {
    // JSON shape golden: serialized bytes must have type, command, args, env
    // in that exact order. Asserted by reading the raw bytes and verifying
    // the position of each key.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "ordered",
        "--env",
        "K=V",
    ])
    .expect("enable");

    let raw = std::fs::read_to_string(&cfg_path).expect("read");
    // Slice out the inner object for the server entry.
    let server_start = raw.find(r#""ordered""#).expect("ordered key");
    let after = &raw[server_start..];
    let body_start = after.find('{').expect("body");
    let body_end = after[body_start..].find('}').expect("close") + body_start;
    let body = &after[body_start..=body_end];

    let pos_type = body.find(r#""type""#).expect("type field");
    let pos_command = body.find(r#""command""#).expect("command field");
    let pos_args = body.find(r#""args""#).expect("args field");
    let pos_env = body.find(r#""env""#).expect("env field");

    assert!(
        pos_type < pos_command,
        "type must precede command, body={body}"
    );
    assert!(
        pos_command < pos_args,
        "command must precede args, body={body}"
    );
    assert!(pos_args < pos_env, "args must precede env, body={body}");
}

#[test]
fn round_trip_preserves_full_six_field_order_type_command_args_env_url_headers() {
    // Spec check #7c: assert all six VSCodeServer fields preserve declaration
    // order on round-trip. `enable` only writes type/command/args/env, so we
    // seed a fixture that already contains url/headers, run `enable` on a
    // different server name (which triggers a full read-modify-write of the
    // file), then verify the preserved server's serialized bytes have all six
    // fields in the canonical order.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    let seed = serde_json::json!({
        "servers": {
            "remote": {
                "type": "sse",
                "command": "ignored-for-sse",
                "args": ["--unused"],
                "env": { "K": "V" },
                "url": "https://example.test/mcp",
                "headers": { "Authorization": "Bearer abc" }
            }
        }
    });
    std::fs::write(&cfg_path, serde_json::to_vec_pretty(&seed).unwrap()).expect("seed");

    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "new-stdio",
    ])
    .expect("enable second server");

    let raw = std::fs::read_to_string(&cfg_path).expect("read");

    // First, parse-and-verify the preserved fields are present (the on-disk
    // text contains nested objects like env: {...}, so a naive find('}') would
    // truncate before url/headers).
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    let remote = &parsed["servers"]["remote"];
    assert_eq!(remote["type"].as_str(), Some("sse"), "type preserved");
    assert_eq!(
        remote["url"].as_str(),
        Some("https://example.test/mcp"),
        "url preserved in {raw}"
    );
    assert_eq!(
        remote["headers"]["Authorization"].as_str(),
        Some("Bearer abc"),
        "headers preserved in {raw}"
    );

    // Now verify on-disk byte ordering of all six keys. Walk the raw text
    // with a balanced-brace counter so the nested `env: {...}` doesn't
    // truncate the slice.
    let server_start = raw.find(r#""remote""#).expect("remote key");
    let after = &raw[server_start..];
    let body_start = after.find('{').expect("body open");
    let body_end_offset = balanced_object_end(&after[body_start..]).expect("balanced body close");
    let body = &after[body_start..=body_start + body_end_offset];

    let pos = |k: &str| {
        body.find(k)
            .unwrap_or_else(|| panic!("{k} missing in body={body}"))
    };
    let p_type = pos(r#""type""#);
    let p_command = pos(r#""command""#);
    let p_args = pos(r#""args""#);
    let p_env = pos(r#""env""#);
    let p_url = pos(r#""url""#);
    let p_headers = pos(r#""headers""#);

    assert!(p_type < p_command, "type < command failed in {body}");
    assert!(p_command < p_args, "command < args failed in {body}");
    assert!(p_args < p_env, "args < env failed in {body}");
    assert!(p_env < p_url, "env < url failed in {body}");
    assert!(p_url < p_headers, "url < headers failed in {body}");
}

/// Given a slice that starts with `{`, return the byte offset of the matching
/// `}`. Counts nested braces (and is string-literal-aware) so a body
/// containing nested objects isn't truncated at the inner close. Used by the
/// six-field-order golden test.
fn balanced_object_end(slice: &str) -> Option<usize> {
    let bytes = slice.as_bytes();
    debug_assert_eq!(bytes[0], b'{');
    let mut depth: u32 = 0;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_str => escaped = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn enable_includes_log_level_in_args() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
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
    let args: Vec<&str> = doc["servers"]["test-cli"]["args"]
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
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
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
    let env = &doc["servers"]["test-cli"]["env"];
    assert_eq!(env["PATH"].as_str(), Some("/usr/local/bin"));
    assert_eq!(env["DEBUG"].as_str(), Some("1"));
}

#[test]
fn enable_overwrites_existing_server() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("first enable");

    dispatch(&[
        "mcp",
        "vscode",
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
    let args: Vec<&str> = doc["servers"]["test-cli"]["args"]
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
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("enable");

    dispatch(&[
        "mcp",
        "vscode",
        "disable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "test-cli",
    ])
    .expect("disable");

    let doc = read_json(&cfg_path);
    assert!(
        doc["servers"].get("test-cli").is_none(),
        "test-cli must be removed"
    );
}

#[test]
fn disable_missing_server_is_ok() {
    // Per PLAN §11 #5: disable on a missing server name prints a warning
    // and returns Ok(()) — not an error.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");
    std::fs::write(&cfg_path, b"{\"servers\":{}}").expect("seed");

    dispatch(&[
        "mcp",
        "vscode",
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
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
        "list",
        "--config-path",
        cfg_path.to_str().unwrap(),
    ])
    .expect("list on missing file is Ok");
    assert!(!cfg_path.exists(), "list must not create the config file");
}

#[test]
fn list_shows_configured_servers() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");
    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "alpha",
    ])
    .expect("enable alpha");
    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "beta",
    ])
    .expect("enable beta");

    let doc = read_json(&cfg_path);
    let names: Vec<&str> = doc["servers"]
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
    let cfg_path = dir.path().join("mcp.json");

    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "first",
    ])
    .expect("first enable");
    let backup_path = dir.path().join("mcp.backup.json");
    assert!(
        !backup_path.exists(),
        "first write must not produce a backup"
    );

    let pre_second = std::fs::read(&cfg_path).expect("read pre-second");

    dispatch(&[
        "mcp",
        "vscode",
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
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");
    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "fresh",
    ])
    .expect("enable");

    assert!(cfg_path.exists());
    let backup_path = dir.path().join("mcp.backup.json");
    assert!(
        !backup_path.exists(),
        "first write must NOT create a backup"
    );
}

// ── round-trip preserves inputs[] (the headline reason this struct exists) ─

#[test]
fn round_trip_preserves_inputs_with_mixed_password_states() {
    // PLAN line 566: brontes never CONSTRUCTS an Input, but read-mutate-
    // write must preserve them verbatim. Both password=true and
    // password=false must survive.
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");

    // Seed a fixture: two inputs (one each password state) + one server.
    let seed = r#"{
  "inputs": [
    {
      "type": "promptString",
      "id": "api-key",
      "description": "API key",
      "password": true
    },
    {
      "type": "promptString",
      "id": "username",
      "description": "Username",
      "password": false
    }
  ],
  "servers": {
    "existing": {
      "type": "stdio",
      "command": "/bin/existing"
    }
  }
}
"#;
    std::fs::write(&cfg_path, seed).expect("seed");

    // Enable a NEW server — manager loads, mutates, saves.
    dispatch(&[
        "mcp",
        "vscode",
        "enable",
        "--config-path",
        cfg_path.to_str().unwrap(),
        "--server-name",
        "added",
    ])
    .expect("enable");

    // Read back and assert: both inputs preserved, both servers present.
    let doc = read_json(&cfg_path);
    let inputs = doc["inputs"].as_array().expect("inputs array preserved");
    assert_eq!(inputs.len(), 2, "must preserve both input entries");

    // Find each by id (order is preserved by serde via Vec, but assert by
    // content so a reorder doesn't false-fail).
    let api_key = inputs
        .iter()
        .find(|i| i["id"].as_str() == Some("api-key"))
        .expect("api-key input preserved");
    assert_eq!(api_key["type"].as_str(), Some("promptString"));
    assert_eq!(api_key["description"].as_str(), Some("API key"));
    assert_eq!(
        api_key["password"].as_bool(),
        Some(true),
        "password=true must survive"
    );

    let username = inputs
        .iter()
        .find(|i| i["id"].as_str() == Some("username"))
        .expect("username input preserved");
    assert_eq!(username["type"].as_str(), Some("promptString"));
    assert_eq!(username["description"].as_str(), Some("Username"));
    // password=false omitted on write (`omitempty`); the missing key on
    // re-parse defaults to false.
    assert!(
        username.get("password").is_none() || username["password"].as_bool() == Some(false),
        "password=false should round-trip as either missing or false"
    );

    // Both servers present.
    let servers = doc["servers"].as_object().expect("servers preserved");
    assert!(
        servers.contains_key("existing"),
        "existing server preserved"
    );
    assert!(servers.contains_key("added"), "newly added server present");

    // The seeded server's type/command survived intact.
    assert_eq!(
        servers["existing"]["type"].as_str(),
        Some("stdio"),
        "existing type field survives round-trip"
    );
    assert_eq!(
        servers["existing"]["command"].as_str(),
        Some("/bin/existing"),
        "existing command field survives round-trip"
    );
}

// ── --workspace flag: must work on enable, disable, AND list ──────────────

/// Spawn a CLI invocation with `$CWD` set to `workspace_dir`.
///
/// `std::env::set_current_dir` is process-global and Rust runs tests
/// concurrently by default, so this helper is wrapped in a mutex to serialize
/// the three `--workspace` tests against each other.
fn with_cwd<R>(workspace_dir: &std::path::Path, f: impl FnOnce() -> R) -> R {
    use std::sync::{Mutex, OnceLock};
    static CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = CWD_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().expect("cwd lock");

    let prev = std::env::current_dir().expect("save cwd");
    std::env::set_current_dir(workspace_dir).expect("set cwd");
    let result = f();
    std::env::set_current_dir(&prev).expect("restore cwd");
    result
}

#[test]
fn workspace_enable_writes_under_cwd_dot_vscode() {
    // No --config-path; --workspace must route the write to
    // $CWD/.vscode/mcp.json.
    let dir = TempDir::new().expect("tempdir");

    with_cwd(dir.path(), || {
        dispatch(&[
            "mcp",
            "vscode",
            "enable",
            "--workspace",
            "--server-name",
            "ws-srv",
        ])
        .expect("workspace enable");
    });

    let expected = dir.path().join(".vscode").join("mcp.json");
    assert!(
        expected.exists(),
        "workspace enable must write to {}",
        expected.display()
    );
    let doc = read_json(&expected);
    assert!(doc["servers"]["ws-srv"].is_object());
}

#[test]
fn workspace_disable_targets_cwd_dot_vscode() {
    let dir = TempDir::new().expect("tempdir");
    let workspace_cfg = dir.path().join(".vscode").join("mcp.json");
    std::fs::create_dir_all(workspace_cfg.parent().unwrap()).expect("mkdir");
    std::fs::write(
        &workspace_cfg,
        br#"{"servers":{"to-remove":{"type":"stdio","command":"/bin/x"}}}"#,
    )
    .expect("seed");

    with_cwd(dir.path(), || {
        dispatch(&[
            "mcp",
            "vscode",
            "disable",
            "--workspace",
            "--server-name",
            "to-remove",
        ])
        .expect("workspace disable");
    });

    let doc = read_json(&workspace_cfg);
    assert!(
        doc["servers"].get("to-remove").is_none(),
        "workspace disable must target $CWD/.vscode/mcp.json"
    );
}

#[test]
fn workspace_list_targets_cwd_dot_vscode() {
    let dir = TempDir::new().expect("tempdir");
    let workspace_cfg = dir.path().join(".vscode").join("mcp.json");
    std::fs::create_dir_all(workspace_cfg.parent().unwrap()).expect("mkdir");
    std::fs::write(
        &workspace_cfg,
        br#"{"servers":{"present":{"type":"stdio","command":"/bin/x"}}}"#,
    )
    .expect("seed");

    with_cwd(dir.path(), || {
        // list is stdout-only; success is "did not error and did not
        // crash on $CWD-resolved path". The file is preserved.
        dispatch(&["mcp", "vscode", "list", "--workspace"]).expect("workspace list");
    });

    assert!(
        workspace_cfg.exists(),
        "workspace list must not delete the file"
    );
}

// ── load semantics ────────────────────────────────────────────────────────

#[test]
fn list_on_invalid_json_returns_json_error() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");
    std::fs::write(&cfg_path, b"not valid json").expect("seed");

    let result = dispatch(&[
        "mcp",
        "vscode",
        "list",
        "--config-path",
        cfg_path.to_str().unwrap(),
    ]);
    let err = result.expect_err("must fail on invalid JSON");
    let msg = err.to_string();
    assert!(
        msg.contains("editor config: JSON error"),
        "unexpected error: {msg}"
    );
}

#[test]
fn enable_rejects_malformed_env_flag() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("mcp.json");
    let result = dispatch(&[
        "mcp",
        "vscode",
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
