//! Subprocess execution for MCP tool calls.
//!
//! When an MCP client invokes a tool, brontes spawns the user's CLI binary
//! as a subprocess with the corresponding clap subcommand path and the
//! caller-supplied flag and positional arguments. stdout and stderr are
//! captured, the exit code is recorded, and the result is returned as a
//! [`ToolOutput`].
//!
//! # Executable resolution
//!
//! The binary path is resolved exactly once via [`std::env::current_exe`]
//! and cached in a [`OnceLock`]. This mirrors ophis's eager `os.Executable()`
//! capture at module init (`execute.go:15`) while deferring the resolution
//! to the first tool call so unit tests that never spawn a subprocess do
//! not depend on the executable being resolvable. If the binary is moved
//! or replaced between brontes startup and the first call, the captured
//! path reflects the binary's location at first-call time.
//!
//! # Cancellation
//!
//! When the supplied [`CancellationToken`] fires, the in-flight subprocess
//! is killed via [`tokio::process::Child::kill`]. The child is also marked
//! `kill_on_drop(true)`, so an aborted task or panicking caller does not
//! leak a running subprocess.
//!
//! # Exit-code split
//!
//! Spawn failures (missing binary, fork failed, permissions denied) return
//! [`Error::Spawn`]. A subprocess that runs and exits non-zero returns a
//! successful [`ToolOutput`] with the captured streams and the non-zero
//! `exit_code`; the call is **not** an error from brontes's perspective —
//! the MCP layer surfaces the failure to the client via `is_error: true`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command as TokioCommand;
use tokio_util::sync::CancellationToken;

use crate::tool::{ToolInput, ToolOutput};
use crate::{Error, Result};

/// Soft cap on captured stdout/stderr per tool call (16 MiB each).
///
/// Tool subprocess output is buffered into [`Vec<u8>`] before being handed
/// back as a [`ToolOutput`]. Without a cap a runaway tool can OOM brontes
/// itself. When a stream crosses the cap, brontes emits one `warn` and
/// continues reading (discarding the overflow) so the child does not block
/// on a full pipe.
pub const OUTPUT_CAP_BYTES: usize = 16 * 1024 * 1024;

/// Cached path to the current executable.
///
/// Captured lazily on the first tool call. If the binary is moved/replaced
/// between brontes startup and the first call, the captured path reflects
/// the binary's location at first-call time. Once captured, the path is
/// reused for the lifetime of the process so each tool call avoids a
/// redundant syscall.
static EXECUTABLE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Return the path to the current executable, caching the result.
///
/// Returns [`Error::Io`] if [`std::env::current_exe`] fails. The first
/// successful resolution is cached; subsequent calls are infallible
/// clones of the cached `PathBuf`.
pub fn current_executable() -> Result<PathBuf> {
    if let Some(p) = EXECUTABLE_PATH.get() {
        return Ok(p.clone());
    }
    let path = std::env::current_exe().map_err(|e| Error::Io {
        context: "resolve current_exe for tool subprocess".into(),
        source: e,
    })?;
    // First writer wins; ignore the race result and re-read.
    let _ = EXECUTABLE_PATH.set(path);
    Ok(EXECUTABLE_PATH
        .get()
        .expect("OnceLock set above or by a concurrent writer")
        .clone())
}

/// Convert an MCP tool name plus a [`ToolInput`] into the argv vector handed
/// to the spawned subprocess.
///
/// The tool name is split on `_`; the first token (root command name or
/// configured prefix) is dropped, the remainder are the clap subcommand
/// path. Flags are appended next, then positional args.
///
/// Mirrors ophis `buildCommandArgs` / `buildFlagArgs` (`execute.go:66-123`).
pub fn build_command_args(tool_name: &str, input: &ToolInput) -> Vec<String> {
    // Drop the root token (the binary identifies itself; tool name encodes
    // root + subcommand path).
    let mut args: Vec<String> = tool_name.split('_').skip(1).map(str::to_string).collect();

    for (name, value) in &input.flags {
        append_flag(&mut args, name, value, tool_name);
    }

    for a in &input.args {
        args.push(a.clone());
    }

    args
}

/// Translate one `(flag_name, JSON value)` pair into the argv tokens it
/// produces. Mirrors ophis `parseFlagArgValue` semantics with the divergences
/// noted on the individual helpers below.
fn append_flag(out: &mut Vec<String>, name: &str, value: &Value, tool_name: &str) {
    // Parity with ophis `execute.go:84-86`: empty key or null value → skip.
    if name.is_empty() || value.is_null() {
        return;
    }

    match value {
        Value::Array(items) => {
            for item in items {
                append_scalar_flag(out, name, item, tool_name);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                if matches!(v, Value::Object(_) | Value::Array(_)) {
                    tracing::warn!(
                        target: "brontes::exec",
                        tool = %tool_name,
                        flag = %name,
                        key = %k,
                        "object-valued flag contained a non-scalar value; skipping"
                    );
                    continue;
                }
                let rendered = render_scalar(v);
                out.push(format!("--{name}"));
                out.push(format!("{k}={rendered}"));
            }
        }
        _ => append_scalar_flag(out, name, value, tool_name),
    }
}

/// Push the argv tokens for a single scalar flag value. Booleans take the
/// `--flag` / no-arg form; all other scalars take `--flag VALUE`.
fn append_scalar_flag(out: &mut Vec<String>, name: &str, value: &Value, tool_name: &str) {
    match value {
        Value::Bool(true) => out.push(format!("--{name}")),
        Value::Bool(false) | Value::Null => {}
        Value::String(s) => {
            out.push(format!("--{name}"));
            out.push(s.clone());
        }
        Value::Number(n) => {
            out.push(format!("--{name}"));
            out.push(n.to_string());
        }
        Value::Array(_) | Value::Object(_) => {
            tracing::warn!(
                target: "brontes::exec",
                tool = %tool_name,
                flag = %name,
                "nested non-scalar flag value; skipping"
            );
        }
    }
}

fn render_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => String::new(),
    }
}

/// Spawn the user's CLI binary as a subprocess for a tool call.
///
/// `argv` is the subcommand-path + flags + positional args already built by
/// [`build_command_args`]. `env` is the per-call environment merged from
/// [`crate::Config::default_env`] and any client-supplied overrides (callers
/// merge upstream; this function applies the result verbatim).
///
/// Cancellation: when `cancel` fires, the child is killed and an [`Error::Io`]
/// with context `"tool cancelled"` is returned.
///
/// # Errors
///
/// - [`Error::Io`] if [`std::env::current_exe`] fails on the first call.
/// - [`Error::Spawn`] if the subprocess cannot be started.
/// - [`Error::Io`] if the child stream capture or wait fails.
pub async fn run_tool(
    tool_name: &str,
    input: &ToolInput,
    env: &HashMap<String, String>,
    cancel: CancellationToken,
) -> Result<ToolOutput> {
    let exe = current_executable()?;
    let argv = build_command_args(tool_name, input);

    tracing::debug!(
        target: "brontes::exec",
        tool = %tool_name,
        ?argv,
        "spawning tool subprocess"
    );

    let mut cmd = TokioCommand::new(&exe);
    cmd.args(&argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(Error::Spawn)?;

    // Take the pipes; we read them concurrently into capped buffers so that
    // (a) a runaway tool cannot OOM brontes itself, and (b) the child does
    // not deadlock writing to a full pipe — we keep draining even after
    // the cap is hit, just discarding the overflow.
    let stdout_pipe = child
        .stdout
        .take()
        .expect("child stdout was set to Stdio::piped above");
    let stderr_pipe = child
        .stderr
        .take()
        .expect("child stderr was set to Stdio::piped above");

    let stdout_task = tokio::spawn(read_capped(stdout_pipe, "stdout", tool_name.to_string()));
    let stderr_task = tokio::spawn(read_capped(stderr_pipe, "stderr", tool_name.to_string()));

    tokio::select! {
        () = cancel.cancelled() => {
            // Drop order: child (kill_on_drop) → reader tasks. We do not
            // need to await the reader tasks; the kernel closes the pipes
            // when the child dies, the reads return, and the tasks finish.
            Err(Error::Io {
                context: format!("tool '{tool_name}' cancelled"),
                source: std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"),
            })
        }
        status = child.wait() => {
            let status = status.map_err(|e| Error::Io {
                context: format!("wait for tool '{tool_name}'"),
                source: e,
            })?;
            // Reader tasks finish naturally once the child closes its pipes.
            let stdout = stdout_task.await.map_err(|e| Error::Panic(format!(
                "stdout reader task for tool '{tool_name}': {e}"
            )))?;
            let stderr = stderr_task.await.map_err(|e| Error::Panic(format!(
                "stderr reader task for tool '{tool_name}': {e}"
            )))?;
            Ok(ToolOutput {
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                // ExitStatus::code() returns None when killed by signal;
                // ophis surfaces this as the underlying *exec.ExitError
                // ExitCode() value (-1 on unix signal). Match with the
                // documented -1 sentinel (tool.rs).
                exit_code: status.code().unwrap_or(-1),
            })
        }
    }
}

/// Drain an async byte stream into a `Vec<u8>` capped at
/// [`OUTPUT_CAP_BYTES`].
///
/// Reads in fixed-size chunks; once the buffer reaches the cap, the
/// remainder is read-and-discarded (so the child does not block on a full
/// pipe) and a single `warn` is emitted. Returns whatever bytes were
/// retained — at most `OUTPUT_CAP_BYTES`.
async fn read_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    stream_label: &'static str,
    tool_name: String,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8 * 1024];
    let mut warned = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let remaining = OUTPUT_CAP_BYTES.saturating_sub(buf.len());
                if remaining == 0 {
                    if !warned {
                        tracing::warn!(
                            target: "brontes::exec",
                            tool = %tool_name,
                            stream = %stream_label,
                            limit_bytes = OUTPUT_CAP_BYTES,
                            "tool output exceeded soft cap; further output truncated"
                        );
                        warned = true;
                    }
                    // Continue draining so the child does not block on a
                    // full pipe — but discard the bytes.
                    continue;
                }
                if n <= remaining {
                    buf.extend_from_slice(&chunk[..n]);
                } else {
                    buf.extend_from_slice(&chunk[..remaining]);
                    if !warned {
                        tracing::warn!(
                            target: "brontes::exec",
                            tool = %tool_name,
                            stream = %stream_label,
                            limit_bytes = OUTPUT_CAP_BYTES,
                            "tool output exceeded soft cap; further output truncated"
                        );
                        warned = true;
                    }
                }
            }
        }
    }
    buf
}

// ---------------------------------------------------------------------------
// Test-only re-exports (consumed via `lib.rs::__test_internal`).
//
// `append_flag` and `read_capped` are private implementation details; the
// warn-fire integration test crate (`tests/warn_fires.rs`) needs to drive
// them directly so the nested-container and `OUTPUT_CAP_BYTES`
// `tracing::warn!` events can be asserted without spinning up a full MCP
// server + subprocess.
// ---------------------------------------------------------------------------

/// Test-only proxy for [`append_flag`]. Exposed via
/// [`crate::__test_internal::render_flag_argv`].
pub fn append_flag_for_test(out: &mut Vec<String>, name: &str, value: &Value, tool_name: &str) {
    append_flag(out, name, value, tool_name);
}

/// Test-only proxy for [`read_capped`]. Exposed via
/// [`crate::__test_internal::drain_capped`].
pub async fn read_capped_for_test<R: AsyncRead + Unpin>(
    reader: R,
    stream_label: &'static str,
    tool_name: String,
) -> Vec<u8> {
    read_capped(reader, stream_label, tool_name).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_args_drops_root_token() {
        let input = ToolInput::default();
        let argv = build_command_args("myapp_sub_leaf", &input);
        assert_eq!(argv, vec!["sub".to_string(), "leaf".to_string()]);
    }

    #[test]
    fn build_args_with_root_only_is_empty() {
        let input = ToolInput::default();
        let argv = build_command_args("myapp", &input);
        assert!(argv.is_empty());
    }

    #[test]
    fn flag_bool_true_renders_long() {
        let mut input = ToolInput::default();
        input.flags.insert("verbose".to_string(), json!(true));
        let argv = build_command_args("app_sub", &input);
        assert_eq!(argv, vec!["sub".to_string(), "--verbose".to_string()]);
    }

    #[test]
    fn flag_bool_false_is_omitted() {
        let mut input = ToolInput::default();
        input.flags.insert("verbose".to_string(), json!(false));
        let argv = build_command_args("app_sub", &input);
        assert_eq!(argv, vec!["sub".to_string()]);
    }

    #[test]
    fn flag_string_renders_two_tokens() {
        let mut input = ToolInput::default();
        input
            .flags
            .insert("output".to_string(), json!("results.json"));
        let argv = build_command_args("app_sub", &input);
        assert_eq!(
            argv,
            vec![
                "sub".to_string(),
                "--output".to_string(),
                "results.json".to_string()
            ]
        );
    }

    #[test]
    fn flag_number_renders_decimal_string() {
        let mut input = ToolInput::default();
        input.flags.insert("limit".to_string(), json!(42));
        let argv = build_command_args("app_sub", &input);
        assert_eq!(
            argv,
            vec!["sub".to_string(), "--limit".to_string(), "42".to_string()]
        );
    }

    #[test]
    fn flag_array_recurses_per_item() {
        let mut input = ToolInput::default();
        input
            .flags
            .insert("tag".to_string(), json!(["alpha", "beta"]));
        let argv = build_command_args("app_sub", &input);
        // The two items each produce --tag VALUE.
        assert!(argv.windows(2).any(|w| w[0] == "--tag" && w[1] == "alpha"));
        assert!(argv.windows(2).any(|w| w[0] == "--tag" && w[1] == "beta"));
    }

    #[test]
    fn flag_object_renders_key_equals_value() {
        let mut input = ToolInput::default();
        input
            .flags
            .insert("label".to_string(), json!({"env": "prod"}));
        let argv = build_command_args("app_sub", &input);
        // Order: sub, --label, env=prod.
        assert!(argv.contains(&"--label".to_string()));
        assert!(argv.contains(&"env=prod".to_string()));
    }

    #[test]
    fn flag_empty_name_is_skipped() {
        let mut input = ToolInput::default();
        input.flags.insert(String::new(), json!("ignored"));
        let argv = build_command_args("app_sub", &input);
        assert_eq!(argv, vec!["sub".to_string()]);
    }

    #[test]
    fn flag_null_value_is_skipped() {
        let mut input = ToolInput::default();
        input.flags.insert("x".to_string(), Value::Null);
        let argv = build_command_args("app_sub", &input);
        assert_eq!(argv, vec!["sub".to_string()]);
    }

    #[test]
    fn positional_args_appended_after_flags() {
        let mut input = ToolInput::default();
        input.flags.insert("v".to_string(), json!(true));
        input.args = vec!["a".into(), "b".into()];
        let argv = build_command_args("app_sub", &input);
        assert_eq!(
            argv,
            vec![
                "sub".to_string(),
                "--v".to_string(),
                "a".to_string(),
                "b".to_string()
            ]
        );
    }

    /// Drive `read_capped` with > 16 MiB of input and assert the returned
    /// buffer is exactly the cap. Uses an in-memory reader so the test does
    /// not depend on spawning a subprocess that can emit ~17 MiB on every
    /// platform's CI.
    #[tokio::test]
    async fn read_capped_truncates_at_cap() {
        // Build a reader that yields 17 MiB of zeros.
        let total = OUTPUT_CAP_BYTES + (1024 * 1024);
        let source = vec![0u8; total];
        let reader = std::io::Cursor::new(source);
        let captured = read_capped(reader, "stdout", "test-tool".into()).await;
        assert_eq!(
            captured.len(),
            OUTPUT_CAP_BYTES,
            "captured output must be exactly the cap"
        );
    }

    /// Below the cap, all bytes pass through.
    #[tokio::test]
    async fn read_capped_passes_through_below_cap() {
        let payload = b"hello world".to_vec();
        let reader = std::io::Cursor::new(payload.clone());
        let captured = read_capped(reader, "stdout", "test-tool".into()).await;
        assert_eq!(captured, payload);
    }
}
