//! Integration tests for every `tracing::warn!` site brontes emits.
//!
//! Each `tracing::warn!` site in `src/` encodes a user-facing behavior
//! contract — see `tests/support/mod.rs` for the rationale. The tests in
//! this file assert the warns actually fire with the documented field
//! names and values, so silently deleting a warn (or changing its
//! field names) is caught by CI.
//!
//! Coverage map (warn site → test fn):
//!
//! - `src/subcommands/start.rs::parse_log_level` (unknown level — §11 #9)
//!   → [`start_unknown_log_level_warns`]
//! - `src/subcommands/stream.rs::parse_log_level` (same shape, separate
//!   surface) → [`stream_unknown_log_level_warns`]
//! - `src/exec.rs::append_flag` object-with-nested (§11 #7)
//!   → [`flag_object_with_nested_object_warns`],
//!   [`flag_object_with_nested_array_warns`]
//! - `src/exec.rs::append_scalar_flag` array-with-nested (§11 #7)
//!   → [`flag_array_with_nested_object_warns`],
//!   [`flag_array_with_nested_array_warns`]
//! - `src/command.rs` 64-char tool-name warn (PLAN line 537)
//!   → [`tool_name_over_64_chars_warns_once`]
//! - `src/command.rs` selector substring no-match warn
//!   → [`selector_substring_no_match_warns`]
//! - `src/exec.rs::read_capped` stdout/stderr `OUTPUT_CAP_BYTES` exhaustion
//!   → [`read_capped_stdout_emits_one_warn`],
//!   [`read_capped_stderr_emits_one_warn`]
//!
//! Uncovered (surfaced as SUGGESTs in the implementer report, not in CI):
//!
//! - `src/server/{stdio,http}.rs` signal-handler install failures —
//!   `tokio::signal::unix::signal(..)` does not fail under normal test
//!   conditions; testing requires either injecting a faulty signal
//!   source (production refactor) or running under a constrained
//!   process namespace. Out of scope for this task.
//! - `src/server/http.rs` `accept failed; continuing` — provoking a
//!   `TcpListener::accept` error mid-loop without breaking the listener
//!   itself is fiddly across OSes; left as a follow-up.
//! - `src/server/http.rs` `connections did not drain within ...` —
//!   requires holding a connection open past `SHUTDOWN_GRACE` (5s);
//!   left out to keep the test suite fast.

mod support;

use brontes::{Config, Selector, selectors};
use clap::{Arg, ArgAction, Command};
use serde_json::json;

use support::{assert_contains_all, capture_warns, capture_warns_async, count_occurrences};

// ---------------------------------------------------------------------------
// §11 #9 — unrecognized `--log-level`
// ---------------------------------------------------------------------------

#[test]
fn start_unknown_log_level_warns() {
    let cmd = brontes::__test_internal::start_subcommand();
    let matches = cmd
        .try_get_matches_from(["start", "--log-level", "foobar"])
        .expect("clap parses --log-level even when the value is unknown");

    let (result, captured) =
        capture_warns(|| brontes::__test_internal::parse_start_log_level(&matches));
    assert!(
        result.is_none(),
        "unknown level must fall through to default (None)"
    );
    assert_contains_all(
        &captured,
        &["WARN", "unrecognized --log-level", "value=foobar"],
    );
}

#[test]
fn stream_unknown_log_level_warns() {
    let cmd = brontes::__test_internal::stream_subcommand();
    let matches = cmd
        .try_get_matches_from(["stream", "--log-level", "verbose"])
        .expect("clap parses --log-level even when the value is unknown");

    let (result, captured) =
        capture_warns(|| brontes::__test_internal::parse_stream_log_level(&matches));
    assert!(
        result.is_none(),
        "unknown level must fall through to default (None)"
    );
    assert_contains_all(
        &captured,
        &["WARN", "unrecognized --log-level", "value=verbose"],
    );
}

#[test]
fn start_known_log_level_does_not_warn() {
    // Negative test: a recognized level must NOT trip the warn. Guards
    // against accidental over-firing if the match arms drift.
    let cmd = brontes::__test_internal::start_subcommand();
    let matches = cmd
        .try_get_matches_from(["start", "--log-level", "debug"])
        .expect("parses");
    let (result, captured) =
        capture_warns(|| brontes::__test_internal::parse_start_log_level(&matches));
    assert_eq!(result, Some(tracing::Level::DEBUG));
    assert!(
        !captured.contains("unrecognized --log-level"),
        "must not warn on a recognized level; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// §11 #7 — flag-value nested-container handling
// ---------------------------------------------------------------------------

#[test]
fn flag_object_with_nested_object_warns() {
    // `{ "label": { "k": { "nested": "object" } } }` → nested Object value
    // at key "k" triggers the "object-valued flag contained a non-scalar
    // value; skipping" warn. The remaining scalar pair (none in this case)
    // is rendered; here every pair is skipped so argv is empty.
    let value = json!({"k": {"nested": "object"}});
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("label", &value, "myapp_sub"));
    assert!(
        argv.is_empty(),
        "nested-object value must be skipped; argv = {argv:?}"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "object-valued flag contained a non-scalar value; skipping",
            "tool=myapp_sub",
            "flag=label",
            "key=k",
        ],
    );
}

#[test]
fn flag_object_with_nested_array_warns() {
    // Same code path, array-valued inner pair.
    let value = json!({"items": ["a", "b"]});
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("label", &value, "myapp_sub"));
    assert!(
        argv.is_empty(),
        "nested-array value must be skipped; argv = {argv:?}"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "object-valued flag contained a non-scalar value; skipping",
            "tool=myapp_sub",
            "flag=label",
            "key=items",
        ],
    );
}

#[test]
fn flag_array_with_nested_object_warns() {
    // `["scalar", {"x": 1}]` → first item renders, second item trips the
    // "nested non-scalar flag value; skipping" warn from
    // `append_scalar_flag`.
    let value = json!(["scalar", {"x": 1}]);
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("tag", &value, "myapp_sub"));
    assert_eq!(
        argv,
        vec!["--tag".to_string(), "scalar".to_string()],
        "only the scalar item renders; the object item is skipped"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "nested non-scalar flag value; skipping",
            "tool=myapp_sub",
            "flag=tag",
        ],
    );
}

#[test]
fn flag_array_with_nested_array_warns() {
    let value = json!([["nested"], "scalar"]);
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("tag", &value, "myapp_sub"));
    assert_eq!(
        argv,
        vec!["--tag".to_string(), "scalar".to_string()],
        "only the scalar item renders; the array item is skipped"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "nested non-scalar flag value; skipping",
            "tool=myapp_sub",
            "flag=tag",
        ],
    );
}

#[test]
fn flag_object_all_scalar_pairs_no_warn() {
    // Negative test: scalar-only object map must NOT trip the warn.
    let value = json!({"env": "prod", "version": 7});
    let (argv, captured) =
        capture_warns(|| brontes::__test_internal::render_flag_argv("label", &value, "myapp_sub"));
    // Two pairs, two `--label` flags.
    assert_eq!(count_occurrences(&format!("{argv:?}"), "--label"), 2);
    assert!(
        !captured.contains("WARN"),
        "scalar-only object must not warn; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// PLAN line 537 — 64-char tool-name warn
// ---------------------------------------------------------------------------

#[test]
fn tool_name_over_64_chars_warns_once() {
    // Build a clap tree where the resulting tool name exceeds 64 chars.
    // Prefix `myapp` + `_` + a single subcommand whose name is 70 chars:
    //   myapp_aaaaaaaaaa... (5 + 1 + 70 = 76 chars).
    let long_leaf = "a".repeat(70);
    let root =
        Command::new("myapp").subcommand(Command::new(long_leaf.clone()).about("Long-named leaf"));

    let cfg = Config::default();
    let (tools, captured) =
        capture_warns(|| brontes::generate_tools(&root, &cfg).expect("generate_tools succeeds"));

    let expected_name = format!("myapp_{long_leaf}");
    assert!(
        tools.iter().any(|t| t.name.as_ref() == expected_name),
        "expected the long-named tool to be present"
    );

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "MCP tool name exceeds 64 characters",
            // Field assertions: name and len must be present.
            &format!("name={expected_name}"),
            &format!("len={}", expected_name.len()),
        ],
    );

    // Spec says "once per offending tool" — assert exactly one fire for
    // this name, not two.
    assert_eq!(
        count_occurrences(&captured, "MCP tool name exceeds 64 characters"),
        1,
        "64-char warn must fire exactly once per offending tool; captured:\n{captured}"
    );
}

// ---------------------------------------------------------------------------
// Selector substring no-match warn
// ---------------------------------------------------------------------------

#[test]
fn selector_substring_no_match_warns() {
    // CLI has two commands: `myapp greet` and `myapp status`. Selector
    // substring `xyz-nothing-matches` matches neither path; the warn
    // must fire with `needle = "xyz-nothing-matches"`.
    let root = Command::new("myapp")
        .subcommand(Command::new("greet").about("Greet"))
        .subcommand(Command::new("status").about("Status"));

    let cfg = Config::default().selector(Selector {
        cmd: Some(selectors::allow_cmds_containing(["xyz-nothing-matches"])),
        ..Default::default()
    });

    let (_tools, captured) = capture_warns(|| {
        brontes::generate_tools(&root, &cfg).expect("generate_tools succeeds (warn is non-fatal)")
    });

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "Selector substring matches no walked command path",
            "needle=xyz-nothing-matches",
        ],
    );
}

#[test]
fn selector_substring_matching_no_warn() {
    // Negative test: a substring that does match a path must NOT warn.
    let root = Command::new("myapp")
        .subcommand(Command::new("greet").about("Greet"))
        .subcommand(Command::new("status").about("Status"));

    let cfg = Config::default().selector(Selector {
        cmd: Some(selectors::allow_cmds_containing(["status"])),
        ..Default::default()
    });

    let (_tools, captured) =
        capture_warns(|| brontes::generate_tools(&root, &cfg).expect("generate_tools succeeds"));

    assert!(
        !captured.contains("Selector substring matches no walked command path"),
        "matching substring must not warn; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// OUTPUT_CAP_BYTES exhaustion — stdout and stderr each fire one warn
// ---------------------------------------------------------------------------

#[test]
fn read_capped_stdout_emits_one_warn() {
    // Build a reader that yields cap + 1 MiB of bytes; assert the
    // truncation warn fires exactly once with `stream = "stdout"`,
    // `tool = "long-tool"`, and `limit_bytes = <cap>`.
    let total = brontes::__test_internal::OUTPUT_CAP_BYTES + (1024 * 1024);
    let source = vec![0u8; total];

    let (retained, captured) = futures::executor::block_on(async move {
        let mut output: Option<Vec<u8>> = None;
        let ((), log) = capture_warns_async(async {
            let r = brontes::__test_internal::drain_capped(
                std::io::Cursor::new(source),
                "stdout",
                "long-tool".to_string(),
            )
            .await;
            output = Some(r);
        })
        .await;
        (output.expect("drain_capped produced output"), log)
    });

    assert_eq!(
        retained.len(),
        brontes::__test_internal::OUTPUT_CAP_BYTES,
        "retained bytes must equal the cap"
    );
    assert_contains_all(
        &captured,
        &[
            "WARN",
            "tool output exceeded soft cap; further output truncated",
            "tool=long-tool",
            "stream=stdout",
            &format!("limit_bytes={}", brontes::__test_internal::OUTPUT_CAP_BYTES),
        ],
    );
    assert_eq!(
        count_occurrences(&captured, "tool output exceeded soft cap"),
        1,
        "warn must fire exactly once per stream; captured:\n{captured}"
    );
}

#[test]
fn read_capped_stderr_emits_one_warn() {
    let total = brontes::__test_internal::OUTPUT_CAP_BYTES + (512 * 1024);
    let source = vec![0u8; total];

    let captured = futures::executor::block_on(async move {
        let ((), log) = capture_warns_async(async {
            brontes::__test_internal::drain_capped(
                std::io::Cursor::new(source),
                "stderr",
                "noisy-tool".to_string(),
            )
            .await;
        })
        .await;
        log
    });

    assert_contains_all(
        &captured,
        &[
            "WARN",
            "tool output exceeded soft cap; further output truncated",
            "tool=noisy-tool",
            "stream=stderr",
        ],
    );
    assert_eq!(
        count_occurrences(&captured, "tool output exceeded soft cap"),
        1,
    );
}

#[test]
fn read_capped_under_cap_no_warn() {
    // Negative test: below-cap input must NOT warn.
    let payload = b"hello world".to_vec();
    let captured = futures::executor::block_on(async move {
        let ((), log) = capture_warns_async(async {
            brontes::__test_internal::drain_capped(
                std::io::Cursor::new(payload),
                "stdout",
                "quiet-tool".to_string(),
            )
            .await;
        })
        .await;
        log
    });
    assert!(
        !captured.contains("tool output exceeded soft cap"),
        "below-cap must not warn; captured: {captured}"
    );
}

// ---------------------------------------------------------------------------
// Compilation guard: the `ArgAction` import is here so a future test that
// needs ArgAction-driven matches can rely on it being in scope without
// pulling additional `use` lines that drift from the rest of the file.
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _compilation_guard_arg_action() -> Arg {
    Arg::new("dummy").long("dummy").action(ArgAction::SetTrue)
}
