//! Shared `mcp <editor>` subcommand machinery.
//!
//! The Claude / Cursor / `VSCode` editor subtrees share five concerns:
//!
//! - Flag surface — `--config-path`, `--server-name`, `--env`,
//!   `--log-level` (enable only), and a per-editor `--workspace`
//!   (Cursor / `VSCode` only). Helpers here build the clap [`Arg`]s for the
//!   subset every editor uses.
//! - `--env KEY=VAL` parsing — append-mode, short `-e`, parser rejects
//!   missing `=` or empty keys.
//! - `default_env` merge — start with [`crate::Config::default_env`],
//!   overlay `--env`, drop empty merged maps so the JSON `env` key is
//!   omitted entirely (matches ophis `defaultenv_test.go` four-scenario
//!   table).
//! - Server-name derivation — `--server-name` overrides; otherwise
//!   [`crate::manager::paths::derive_server_name`] strips one extension
//!   from `std::env::current_exe()`.
//! - Existing-server warning and disable-missing warning — printed to
//!   stdout with no emoji prefix.
//!
//! Claude, Cursor, and `VSCode` layer their own subcommand modules beside
//! each other using the shared helpers below.

pub mod claude;
pub mod cursor;
pub mod vscode;

use std::collections::BTreeMap;

use clap::{Arg, ArgAction};

use crate::Result;

/// Build the `--config-path <PATH>` clap argument.
///
/// Shared across all three editors and across `enable` / `disable` /
/// `list`. When unset, the editor resolves a per-OS default path.
pub fn arg_config_path() -> Arg {
    Arg::new("config-path")
        .long("config-path")
        .value_name("PATH")
        .help("Path to the editor's MCP config file (overrides per-OS default)")
}

/// Build the `--server-name <NAME>` clap argument.
///
/// Shared across `enable` and `disable`. Default is
/// [`crate::manager::paths::derive_server_name`] of
/// [`std::env::current_exe`].
pub fn arg_server_name() -> Arg {
    Arg::new("server-name")
        .long("server-name")
        .value_name("NAME")
        .help("Name for the MCP server (default: derived from executable name)")
}

/// Build the `--env KEY=VAL` clap argument used by `enable`.
///
/// Repeatable (`ArgAction::Append`). Short form `-e` matches ophis
/// (`enable.go:39`). Parsing rejects values that do not contain `=` or
/// start with `=` (empty key) — caught at [`merge_env`].
pub fn arg_env() -> Arg {
    Arg::new("env")
        .long("env")
        .short('e')
        .value_name("KEY=VAL")
        .help("Environment variable (repeatable; e.g. -e KEY1=val1 -e KEY2=val2)")
        .action(ArgAction::Append)
}

/// Build the `--log-level <LEVEL>` clap argument used by `enable`.
///
/// Persists into the generated `mcp start` argv so the spawned server
/// inherits the level. Validation is intentionally permissive at parse
/// time; unknown values warn at server start (matches `mcp start` /
/// `mcp tools` behavior).
pub fn arg_log_level() -> Arg {
    Arg::new("log-level")
        .long("log-level")
        .value_name("LEVEL")
        .help("Log level for the spawned MCP server (trace, debug, info, warn, error)")
}

/// Merge `default_env` and the parsed `--env` flags into a single map.
///
/// Returns `None` when the merged map is empty so callers can write
/// `Option<BTreeMap<...>>` directly into the server struct's
/// `#[serde(skip_serializing_if = "Option::is_none")] env` field —
/// matching the four ophis `defaultenv_test.go` scenarios:
///
/// - `default_env` populated, no `--env` -> default values written.
/// - `default_env` + `--env` overlap -> user `--env` wins on conflict.
/// - both empty -> `None` returned, JSON `env` key omitted entirely.
/// - one empty, one missing -> same `None` collapse.
///
/// # Errors
///
/// Returns [`crate::Error::Config`] when any `--env` value is missing the
/// `=` separator or has an empty key.
pub fn merge_env(
    default_env: &std::collections::HashMap<String, String>,
    user_pairs: &[String],
) -> Result<Option<BTreeMap<String, String>>> {
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in default_env {
        merged.insert(k.clone(), v.clone());
    }
    for raw in user_pairs {
        let (k, v) = parse_env_pair(raw)?;
        merged.insert(k, v);
    }
    Ok(if merged.is_empty() {
        None
    } else {
        Some(merged)
    })
}

/// Parse a single `--env KEY=VAL` argument.
///
/// The split is on the FIRST `=` so values containing `=` survive intact
/// (matches `KEY=val=with=eq` -> `("KEY", "val=with=eq")`). Empty keys
/// reject with [`crate::Error::Config`].
fn parse_env_pair(raw: &str) -> Result<(String, String)> {
    let Some(idx) = raw.find('=') else {
        return Err(crate::Error::Config(format!(
            "--env value {raw:?} missing '=' separator (expected KEY=VAL)"
        )));
    };
    let (k, v_with_eq) = raw.split_at(idx);
    if k.is_empty() {
        return Err(crate::Error::Config(format!(
            "--env value {raw:?} has empty key (expected KEY=VAL)"
        )));
    }
    // v_with_eq starts with '='; strip exactly one character.
    let v = &v_with_eq[1..];
    Ok((k.to_string(), v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn merge_env_empty_returns_none() {
        let default = HashMap::new();
        let user: Vec<String> = Vec::new();
        let result = merge_env(&default, &user).expect("merges");
        assert!(result.is_none(), "empty merged map must collapse to None");
    }

    #[test]
    fn merge_env_default_only() {
        let mut default = HashMap::new();
        default.insert("PATH".into(), "/usr/bin".into());
        let user: Vec<String> = Vec::new();
        let merged = merge_env(&default, &user)
            .expect("merges")
            .expect("non-empty");
        assert_eq!(merged.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn merge_env_user_wins_on_conflict() {
        let mut default = HashMap::new();
        default.insert("PATH".into(), "/default".into());
        default.insert("HOME".into(), "/home/default".into());
        let user = vec!["PATH=/user".to_string()];
        let merged = merge_env(&default, &user)
            .expect("merges")
            .expect("non-empty");
        assert_eq!(merged.get("PATH").map(String::as_str), Some("/user"));
        assert_eq!(
            merged.get("HOME").map(String::as_str),
            Some("/home/default")
        );
    }

    #[test]
    fn merge_env_user_only() {
        let default = HashMap::new();
        let user = vec!["FOO=bar".to_string()];
        let merged = merge_env(&default, &user)
            .expect("merges")
            .expect("non-empty");
        assert_eq!(merged.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn merge_env_rejects_missing_separator() {
        let default = HashMap::new();
        let user = vec!["BAD".to_string()];
        let err = merge_env(&default, &user).expect_err("must reject missing '='");
        let msg = err.to_string();
        assert!(msg.contains("missing '='"), "got: {msg}");
    }

    #[test]
    fn merge_env_rejects_empty_key() {
        let default = HashMap::new();
        let user = vec!["=val".to_string()];
        let err = merge_env(&default, &user).expect_err("must reject empty key");
        let msg = err.to_string();
        assert!(msg.contains("empty key"), "got: {msg}");
    }

    #[test]
    fn merge_env_value_with_equals_survives() {
        let default = HashMap::new();
        let user = vec!["KEY=foo=bar=baz".to_string()];
        let merged = merge_env(&default, &user)
            .expect("merges")
            .expect("non-empty");
        assert_eq!(merged.get("KEY").map(String::as_str), Some("foo=bar=baz"));
    }
}
