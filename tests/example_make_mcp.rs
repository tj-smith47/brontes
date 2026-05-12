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

/// Build the example with `cargo build --example make-mcp`, then return the
/// path to the produced binary.
///
/// `cargo test --example` doesn't exist; we shell out to `cargo build` once
/// from inside this test and locate the binary by convention
/// (`<target>/debug/examples/make-mcp`). The `CARGO_TARGET_DIR` env var is
/// respected when set (so test runs under a custom target dir still work).
fn build_and_locate_example() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(&cargo)
        .args(["build", "--example", "make-mcp"])
        .status()
        .expect("invoke cargo build --example make-mcp");
    assert!(status.success(), "cargo build --example make-mcp failed");

    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            p.push("target");
            p
        },
        PathBuf::from,
    );
    let bin = if cfg!(windows) {
        "make-mcp.exe"
    } else {
        "make-mcp"
    };
    target_dir.join("debug").join("examples").join(bin)
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

    // The example surfaces both the root (`make`) and the `make_build` leaf;
    // only the leaf carries the required `directory` flag.
    let build_tool = tools
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("make_build"))
        .unwrap_or_else(|| {
            let names: Vec<&str> = tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .collect();
            panic!("expected a tool named `make_build`, got names: {names:?}");
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
        "`directory` must appear in the required flag list for make_build; got {names:?}"
    );
}
