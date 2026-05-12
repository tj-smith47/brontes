//! Claude Desktop `claude_desktop_config.json` shape.
//!
//! Mirrors ophis `internal/cfgmgr/manager/claude/{config,server}.go`
//! verbatim. The JSON top-level is a single key `mcpServers` mapped to an
//! object keyed by server name. Each value carries `command`, optional
//! `args` (array of strings), and optional `env` (string-to-string map).
//!
//! Field order on the Rust struct mirrors the ophis Go struct
//! declaration order so `serde_json::to_string_pretty` writes
//! byte-identical bytes to ophis for the same inputs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::EditorConfig;

/// Top-level shape of `claude_desktop_config.json`.
///
/// Claude Desktop reads only `mcpServers`; the absence of any `inputs`
/// field (which `VSCode` and Cursor carry) is intentional and matches ophis
/// `claude/config.go`.
///
/// The server map is a [`BTreeMap`] so on-disk key order is deterministic
/// across runs — important for the golden round-trip tests against ophis.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ClaudeConfig {
    /// Configured MCP servers, keyed by server name. Insertion-and-removal
    /// driven by [`super::Manager`] via the [`EditorConfig`] trait.
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: BTreeMap<String, ClaudeServer>,
}

/// One entry under `mcpServers` in `claude_desktop_config.json`.
///
/// Field order matches ophis `claude/server.go` exactly so
/// `serde_json::to_string_pretty` produces byte-stable output for the
/// parity golden:
///
/// 1. `command` — absolute path to the MCP server executable.
/// 2. `args` — optional argv tail (e.g. `["mcp", "start"]`); omitted when empty.
/// 3. `env` — optional environment variables; omitted when empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ClaudeServer {
    /// Absolute path to the executable Claude Desktop spawns.
    pub command: String,
    /// Argv tail. `None` (or empty `Some(vec![])`) collapses to no JSON key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Per-server environment variables. `None` (or empty map) collapses
    /// to no JSON key. Sorted for byte-stable output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
}

impl EditorConfig for ClaudeConfig {
    type Server = ClaudeServer;

    fn has_server(&self, name: &str) -> bool {
        self.mcp_servers.contains_key(name)
    }

    fn add_server(&mut self, name: String, server: Self::Server) {
        self.mcp_servers.insert(name, server);
    }

    fn remove_server(&mut self, name: &str) {
        self.mcp_servers.remove(name);
    }

    fn server_names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.mcp_servers.keys().map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_serializes_with_empty_mcpservers() {
        let cfg = ClaudeConfig::default();
        let s = serde_json::to_string(&cfg).expect("serialize");
        assert_eq!(s, r#"{"mcpServers":{}}"#);
    }

    #[test]
    fn server_with_only_command_omits_args_and_env() {
        let server = ClaudeServer {
            command: "/usr/local/bin/myapp".into(),
            args: None,
            env: None,
        };
        let s = serde_json::to_string(&server).expect("serialize");
        assert_eq!(s, r#"{"command":"/usr/local/bin/myapp"}"#);
    }

    #[test]
    fn server_with_args_and_env_serializes_in_field_order() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        env.insert("DEBUG".to_string(), "1".to_string());
        let server = ClaudeServer {
            command: "/bin/app".into(),
            args: Some(vec!["mcp".into(), "start".into()]),
            env: Some(env),
        };
        let s = serde_json::to_string(&server).expect("serialize");
        // Order: command, args, env. Env keys sorted by BTreeMap.
        assert_eq!(
            s,
            r#"{"command":"/bin/app","args":["mcp","start"],"env":{"DEBUG":"1","PATH":"/usr/bin"}}"#
        );
    }

    #[test]
    fn round_trip_preserves_servers() {
        let mut cfg = ClaudeConfig::default();
        cfg.mcp_servers.insert(
            "test".into(),
            ClaudeServer {
                command: "/bin/foo".into(),
                args: Some(vec!["start".into()]),
                env: None,
            },
        );
        let json = serde_json::to_string(&cfg).expect("serialize");
        let parsed: ClaudeConfig = serde_json::from_str(&json).expect("parse");
        assert!(parsed.has_server("test"));
        assert_eq!(parsed.mcp_servers["test"].command, "/bin/foo");
        assert_eq!(
            parsed.mcp_servers["test"].args.as_deref(),
            Some(&["start".to_string()][..])
        );
        assert!(parsed.mcp_servers["test"].env.is_none());
    }

    #[test]
    fn add_remove_server_round_trip() {
        let mut cfg = ClaudeConfig::default();
        assert!(!cfg.has_server("foo"));
        cfg.add_server(
            "foo".into(),
            ClaudeServer {
                command: "/x".into(),
                args: None,
                env: None,
            },
        );
        assert!(cfg.has_server("foo"));
        cfg.remove_server("foo");
        assert!(!cfg.has_server("foo"));
    }

    #[test]
    fn parses_real_world_fixture() {
        // A claude_desktop_config.json sample with one server.
        let raw = r#"{
            "mcpServers": {
                "mytool": {
                    "command": "/usr/local/bin/mytool",
                    "args": ["mcp", "start"],
                    "env": {"LOG_LEVEL": "debug"}
                }
            }
        }"#;
        let cfg: ClaudeConfig = serde_json::from_str(raw).expect("parse");
        assert!(cfg.has_server("mytool"));
        let server = &cfg.mcp_servers["mytool"];
        assert_eq!(server.command, "/usr/local/bin/mytool");
        assert_eq!(
            server.args.as_deref(),
            Some(&["mcp".to_string(), "start".to_string()][..])
        );
        assert_eq!(
            server.env.as_ref().and_then(|m| m.get("LOG_LEVEL")),
            Some(&"debug".to_string())
        );
    }
}
