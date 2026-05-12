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
/// let input = ToolInput {
///     flags: [
///         ("log-level".to_string(), json!("debug")),
///         ("output".to_string(), json!("results.json")),
///     ]
///     .iter()
///     .cloned()
///     .collect(),
///     args: vec!["file1.txt".to_string(), "file2.txt".to_string()],
/// };
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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
/// - `exit_code`: The process exit code as an `i32`. On Unix systems, when a
///   process is killed by a signal and the OS does not yield an exit code,
///   the value `-1` is used as a sentinel. This matches the behavior of
///   `std::process::ExitStatus::code()`, which returns `Option<i32>`.
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolOutput {
    /// Standard output captured from the subprocess.
    pub stdout: String,
    /// Standard error captured from the subprocess.
    pub stderr: String,
    /// Process exit code, or `-1` if killed by signal and no code available.
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
    fn test_tool_input_round_trip() {
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

        let json = to_value(&input).expect("serialize");
        let deserialized: ToolInput = from_value(json).expect("deserialize");

        assert_eq!(deserialized.flags, input.flags);
        assert_eq!(deserialized.args, input.args);
    }

    #[test]
    fn test_tool_output_round_trip() {
        let output = ToolOutput {
            stdout: "Operation succeeded\n".to_string(),
            stderr: "Warning: deprecated flag used\n".to_string(),
            exit_code: 0,
        };

        let json = to_value(&output).expect("serialize");
        let deserialized: ToolOutput = from_value(json).expect("deserialize");

        assert_eq!(deserialized.stdout, output.stdout);
        assert_eq!(deserialized.stderr, output.stderr);
        assert_eq!(deserialized.exit_code, output.exit_code);
    }

    #[test]
    fn test_tool_output_signal_sentinel() {
        // Test that exit_code: -1 (signal kill) round-trips correctly.
        let output = ToolOutput {
            stdout: String::new(),
            stderr: "Process killed by signal\n".to_string(),
            exit_code: -1,
        };

        let json = to_value(&output).expect("serialize");
        let deserialized: ToolOutput = from_value(json).expect("deserialize");

        assert_eq!(deserialized.exit_code, -1, "-1 sentinel must round-trip");
    }
}
