//! Integration test for the `make-mcp` example.
//!
//! Builds the example binary, runs `mcp tools` from a scratch tmpdir, then
//! parses the emitted `mcp-tools.json` and asserts the wrapped `build`
//! subcommand surfaces with `directory` in its `inputSchema.properties.flags.required`
//! array — proving the example exercises the required-flag schema path end
//! to end and that the canonical brontes consumer pattern works without the
//! library being aware of who is consuming it.
//!
//! Mirrors the shape of ophis's `examples/make/main_test.go` end-to-end
//! coverage of the same flow.
//!
//! `mcp tools` writes to `./mcp-tools.json` (cwd-relative) rather than
//! stdout, so the test runs the subprocess with its working directory set
//! to a tmpdir and reads the JSON back from there.

use std::path::PathBuf;
use std::process::Command;

/// Build the example with `cargo build --example make-mcp --message-format=json`
/// and return the path Cargo reports for the produced binary.
///
/// Parsing the JSON output rather than guessing the path makes the test
/// robust to cross-compile setups (`CARGO_BUILD_TARGET=...` inserts a triple
/// segment), workspace target-dir overrides, and the windows `.exe` suffix
/// — Cargo's own report is the source of truth. This is the same approach
/// `assert_cmd` uses internally.
fn build_and_locate_example() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(&cargo)
        .args(["build", "--example", "make-mcp", "--message-format=json"])
        .output()
        .expect("invoke cargo build --example make-mcp");
    assert!(
        output.status.success(),
        "cargo build --example make-mcp failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Cargo emits one JSON object per line on stdout. The line we want is a
    // `compiler-artifact` whose `target.kind` includes `"example"` and whose
    // `target.name` is `"make-mcp"`; its `executable` field is the absolute
    // path to the produced binary. Other lines (build scripts, dependency
    // artifacts, the final `build-finished` summary) are ignored.
    for line in output.stdout.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if msg.get("reason").and_then(|v| v.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let target = msg.get("target");
        let kinds = target
            .and_then(|t| t.get("kind"))
            .and_then(|v| v.as_array());
        let name = target.and_then(|t| t.get("name")).and_then(|v| v.as_str());
        let is_make_mcp_example = kinds
            .is_some_and(|ks| ks.iter().any(|k| k.as_str() == Some("example")))
            && name == Some("make-mcp");
        if !is_make_mcp_example {
            continue;
        }
        if let Some(exe) = msg.get("executable").and_then(|v| v.as_str()) {
            return PathBuf::from(exe);
        }
    }
    panic!(
        "cargo build did not report an executable for the make-mcp example; \
         stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn mcp_tools_emits_build_tool_with_required_directory_flag() {
    let exe = build_and_locate_example();
    let workdir = tempfile::tempdir().expect("create tmpdir");

    let output = Command::new(&exe)
        .args(["mcp", "tools"])
        .current_dir(workdir.path())
        .output()
        .expect("invoke make-mcp mcp tools");
    assert!(
        output.status.success(),
        "`mcp tools` exited non-zero: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let tools_path = workdir.path().join("mcp-tools.json");
    let raw = std::fs::read(&tools_path)
        .unwrap_or_else(|e| panic!("expected mcp-tools.json at {}: {e}", tools_path.display()));
    let tools: serde_json::Value =
        serde_json::from_slice(&raw).expect("mcp-tools.json must be valid JSON");

    let tools = tools
        .as_array()
        .expect("mcp-tools.json must be a JSON array of tool descriptors");

    // The example surfaces both the root (`make-mcp`) and the
    // `make-mcp_build` leaf; only the leaf carries the required `directory`
    // flag. `build_tool_name` only collapses spaces to underscores; hyphens
    // inside path segments are preserved verbatim, so the consumer-visible
    // root name `make-mcp` survives intact.
    let build_tool = tools
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("make-mcp_build"))
        .unwrap_or_else(|| {
            let names: Vec<&str> = tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .collect();
            panic!("expected a tool named `make-mcp_build`, got names: {names:?}");
        });

    // The required-flag schema path puts required flag names under
    // `inputSchema.properties.flags.required` (the nested `flags` object
    // is what carries per-flag metadata; the outer `required` lists the
    // top-level ToolInput slots `flags` and `args`).
    let required = build_tool
        .pointer("/inputSchema/properties/flags/required")
        .and_then(|v| v.as_array())
        .expect("inputSchema.properties.flags.required must be an array");

    let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"directory"),
        "`directory` must appear in the required flag list for make-mcp_build; got {names:?}"
    );
}
