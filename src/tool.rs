//! Types for the MCP tool-call boundary.
//!
//! [`ToolInput`] and [`ToolOutput`] model the request and response at the
//! MCP tool invocation point, translating between MCP's JSON format and the
//! subprocess that executes the underlying CLI command.

/// Request payload for a tool invocation.
///
/// This type models the data handed from an MCP server to a tool executor
/// when a Claude model (or other MCP client) requests that a tool be called.
///
/// # Fields
///
/// - `flags`: A JSON object keyed by long flag names (e.g., the key `log-level`
///   corresponds to the CLI flag `--log-level`). Each value is the JSON
///   representation of the flag's argument (if any); absent keys mean the flag
///   was not provided.
/// - `args`: A list of positional command-line arguments, in order.
///
/// # Serialization
///
/// `ToolInput` round-trips via JSON using serde's standard mechanics.
/// An empty `ToolInput` serializes to `{"flags": {}, "args": []}`.
/// Both the outer object and the `flags` field are always present (never
/// collapsed to `{}`).
///
/// # Example
///
/// ```rust
/// use brontes::ToolInput;
/// use serde_json::json;
///
/// let mut flags = serde_json::Map::new();
/// flags.insert("log-level".into(), json!("debug"));
/// flags.insert("output".into(), json!("results.json"));
///
/// let input = ToolInput {
///     flags,
///     args: vec!["file1.txt".into(), "file2.txt".into()],
/// };
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ToolInput {
    /// Flags provided to the CLI command, keyed by long flag name (e.g., `log-level`).
    pub flags: serde_json::Map<String, serde_json::Value>,
    /// Positional arguments to the CLI command, in order.
    pub args: Vec<String>,
}

/// Response payload from a tool invocation.
///
/// This type captures the outcome of executing a CLI command on behalf of
/// an MCP tool call. It includes the process's captured stdout, stderr, and exit code.
///
/// # Fields
///
/// - `stdout`: The captured standard output from the subprocess.
/// - `stderr`: The captured standard error from the subprocess.
/// - `exit_code`: The process exit code as an `i32`. When the underlying
///   [`std::process::ExitStatus::code`] returns `None` (the process was
///   killed by a signal and the OS did not yield an exit code), brontes
///   flattens that to the sentinel value `-1`. Consumers that need to
///   distinguish "killed by signal" from "exited with -1 deliberately"
///   should inspect the stderr output instead.
///
/// # Serialization
///
/// `ToolOutput` round-trips via JSON using serde's standard mechanics.
/// All three fields are always present in the JSON representation.
///
/// # Example
///
/// ```rust
/// use brontes::ToolOutput;
///
/// let output = ToolOutput {
///     stdout: "Operation succeeded\n".to_string(),
///     stderr: String::new(),
///     exit_code: 0,
/// };
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ToolOutput {
    /// Standard output captured from the subprocess.
    pub stdout: String,
    /// Standard error captured from the subprocess.
    pub stderr: String,
    /// Process exit code. The sentinel value `-1` indicates the process was
    /// killed by signal and the OS did not yield an exit code (flattened from
    /// [`std::process::ExitStatus::code`] returning `None`).
    pub exit_code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn test_tool_input_default() {
        let input = ToolInput::default();
        assert!(input.flags.is_empty());
        assert!(input.args.is_empty());
    }

    #[test]
    fn test_tool_input_wire_shape() {
        // Empty ToolInput must serialize to exactly {"flags": {}, "args": []},
        // not to {} or with missing fields.
        let input = ToolInput::default();
        let json = to_value(&input).expect("serialize");
        let obj = json.as_object().expect("is object");
        assert_eq!(obj.len(), 2, "ToolInput must have exactly 2 fields");
        assert!(obj.contains_key("flags"), "flags field must be present");
        assert!(obj.contains_key("args"), "args field must be present");
        assert!(
            obj["flags"]
                .as_object()
                .expect("flags is object")
                .is_empty(),
            "flags must be an empty object"
        );
        assert!(
            obj["args"].as_array().expect("args is array").is_empty(),
            "args must be an empty array"
        );
    }

    #[test]
    fn test_tool_input_serializes_to_pinned_shape() {
        // Each direction asserts against a hand-curated JSON literal, so a
        // future serde rename / field removal / type change is caught — a
        // bare round-trip (`from_str(to_string(v))`) would only prove serde
        // is internally consistent, not that brontes' wire contract holds.
        let input = ToolInput {
            flags: [
                ("log-level".to_string(), json!("debug")),
                ("output".to_string(), json!("results.json")),
            ]
            .iter()
            .cloned()
            .collect(),
            args: vec!["file1.txt".to_string(), "file2.txt".to_string()],
        };
        let expected = json!({
            "flags": { "log-level": "debug", "output": "results.json" },
            "args": ["file1.txt", "file2.txt"],
        });

        let actual = to_value(&input).expect("serialize");
        assert_eq!(actual, expected, "serialized shape must match contract");

        let parsed: ToolInput = from_value(expected).expect("deserialize");
        assert_eq!(parsed.flags, input.flags, "flags must parse back");
        assert_eq!(parsed.args, input.args, "args must parse back");
    }

    #[test]
    fn test_tool_output_wire_shape() {
        // Pins the three required keys, their types, and ordering. Mirrors
        // `test_tool_input_wire_shape` (no analogous pin existed previously).
        let output = ToolOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
        let json = to_value(&output).expect("serialize");
        let obj = json.as_object().expect("is object");
        assert_eq!(obj.len(), 3, "ToolOutput must have exactly 3 fields");
        assert!(obj.contains_key("stdout"), "stdout field must be present");
        assert!(obj.contains_key("stderr"), "stderr field must be present");
        assert!(
            obj.contains_key("exit_code"),
            "exit_code field must be present"
        );
        assert!(obj["stdout"].is_string(), "stdout must serialize as string");
        assert!(obj["stderr"].is_string(), "stderr must serialize as string");
        assert!(
            obj["exit_code"].is_i64(),
            "exit_code must serialize as integer"
        );
    }

    #[test]
    fn test_tool_output_serializes_to_pinned_shape() {
        let output = ToolOutput {
            stdout: "Operation succeeded\n".to_string(),
            stderr: "Warning: deprecated flag used\n".to_string(),
            exit_code: 0,
        };
        let expected = json!({
            "stdout": "Operation succeeded\n",
            "stderr": "Warning: deprecated flag used\n",
            "exit_code": 0,
        });

        let actual = to_value(&output).expect("serialize");
        assert_eq!(actual, expected, "serialized shape must match contract");

        let parsed: ToolOutput = from_value(expected).expect("deserialize");
        assert_eq!(parsed.stdout, output.stdout);
        assert_eq!(parsed.stderr, output.stderr);
        assert_eq!(parsed.exit_code, output.exit_code);
    }

    #[test]
    fn test_tool_output_negative_exit_code_preserves_sign() {
        // The `-1` sentinel for signal-killed processes is materialised by
        // `status.code().unwrap_or(-1)` at `src/exec.rs:269`; this test pins
        // that a negative exit code survives the JSON contract (round-trip
        // alone would prove only serde's internal consistency).
        let expected = json!({
            "stdout": "",
            "stderr": "Process killed by signal\n",
            "exit_code": -1,
        });
        let parsed: ToolOutput = from_value(expected.clone()).expect("deserialize");
        assert_eq!(parsed.exit_code, -1, "-1 sentinel must deserialize");
        let actual = to_value(&parsed).expect("serialize");
        assert_eq!(actual, expected, "-1 sentinel must re-serialize");
    }
}
