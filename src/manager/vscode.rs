//! `VSCode` MCP `mcp.json` config shape (user-mode
//! `$HOME/Library/Application Support/Code/User/mcp.json` on macOS,
//! `$HOME/.config/Code/User/mcp.json` on Linux,
//! `$HOME/AppData/Roaming/Code/User/mcp.json` on Windows) and
//! workspace-mode `$CWD/.vscode/mcp.json`.
//!
//! Mirrors ophis `internal/cfgmgr/manager/vscode/{config,server}.go`
//! verbatim. The JSON top-level carries an optional `inputs` array (for
//! prompt-string inputs the editor uses when resolving `${input:<id>}`
//! references) plus the `servers` map keyed by server name. The top-level
//! key differs from Cursor — `VSCode` uses `servers`, Cursor uses
//! `mcpServers` — but the per-server struct shape is byte-identical
//! (`VSCodeServer` and `CursorServer` share the same six-field
//! declaration order).
//!
//! Field order on the Rust struct mirrors the ophis Go struct declaration
//! order so `serde_json::to_string_pretty` writes byte-identical bytes to
//! ophis for the same inputs.
//!
//! # Round-trip fidelity
//!
//! brontes never **constructs** an [`super::Input`] (`mcp vscode enable`
//! only writes to the server map), but user configs in the wild routinely
//! carry `inputs[]` entries. The full read-mutate-write cycle must preserve
//! them verbatim or the editor loses its configured prompts on the next
//! save. The integration tests in `tests/manager_vscode.rs` seed a fixture
//! with both `password: true` and `password: false` entries and assert the
//! cycle preserves them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{EditorConfig, Input};

/// Top-level shape of `VSCode`'s `mcp.json` (user-mode and workspace-mode).
///
/// `inputs` is optional and omitted from the on-disk JSON when empty —
/// matching ophis `vscode/config.go` `omitempty`. The server map is a
/// [`BTreeMap`] so on-disk key order is deterministic across runs, which
/// is what the golden round-trip parity tests against ophis require.
///
/// The JSON key is `servers` (NOT `mcpServers` — that is Cursor's
/// spelling), and the Rust field name matches the JSON key literally so
/// no `#[serde(rename = ...)]` is needed.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[allow(clippy::upper_case_acronyms)]
pub struct VSCodeConfig {
    /// `VSCode` / Cursor `inputs[]` prompt-string entries; preserved on
    /// round-trip but never constructed by brontes. Empty `Vec` collapses
    /// to no JSON key (`omitempty`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) inputs: Vec<Input>,

    /// Configured MCP servers, keyed by server name. Insertion-and-removal
    /// driven by [`super::Manager`] via the [`EditorConfig`] trait.
    ///
    /// JSON key: `servers` (NO `rename` — the Rust field name matches the
    /// on-disk key literally).
    #[serde(default)]
    pub(crate) servers: BTreeMap<String, VSCodeServer>,
}

/// One entry under `servers` in `VSCode`'s `mcp.json`.
///
/// Field order matches ophis `vscode/server.go` exactly (same shape as
/// `CursorServer`) so `serde_json::to_string_pretty`
/// produces byte-stable output for the parity golden:
///
/// 1. `type` — always `"stdio"` on write; on read, `omitempty` so non-stdio
///    entries (e.g. `"sse"`) survive a round-trip without forced rewrite.
/// 2. `command` — absolute path to the MCP server executable (`omitempty`).
/// 3. `args` — optional argv tail (e.g. `["mcp", "start"]`); omitted when empty.
/// 4. `env` — optional environment variables; omitted when empty.
/// 5. `url` — optional server URL for non-stdio transports; omitted when absent.
/// 6. `headers` — optional HTTP headers for non-stdio transports; omitted when empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(clippy::upper_case_acronyms)]
pub struct VSCodeServer {
    /// Transport type. brontes always writes `"stdio"`; `omitempty` on read
    /// so existing non-stdio entries (`"sse"`, `"http"`) survive round-trip.
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub(crate) kind: String,
    /// Absolute path to the executable `VSCode` spawns. `omitempty` so
    /// transport-only entries (URL + headers, no command) round-trip.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) command: String,
    /// Argv tail. `None` (or empty `Some(vec![])`) collapses to no JSON key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) args: Option<Vec<String>>,
    /// Per-server environment variables. `None` (or empty map) collapses
    /// to no JSON key. Sorted for byte-stable output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) env: Option<BTreeMap<String, String>>,
    /// URL for non-stdio transports (`omitempty`). brontes never writes this
    /// field; it exists for round-trip fidelity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
    /// HTTP headers for non-stdio transports (`omitempty`). brontes never
    /// writes this field; it exists for round-trip fidelity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) headers: Option<BTreeMap<String, String>>,
}

impl EditorConfig for VSCodeConfig {
    type Server = VSCodeServer;

    fn has_server(&self, name: &str) -> bool {
        self.servers.contains_key(name)
    }

    fn add_server(&mut self, name: String, server: Self::Server) {
        self.servers.insert(name, server);
    }

    fn remove_server(&mut self, name: &str) {
        self.servers.remove(name);
    }

    fn server_names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.servers.keys().map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_serializes_with_only_servers() {
        // `inputs` is `omitempty` (empty `Vec`); only `servers` survives.
        // The JSON key is `servers` (NOT `mcpServers` — that is Cursor's
        // spelling).
        let cfg = VSCodeConfig::default();
        let s = serde_json::to_string(&cfg).expect("serialize");
        assert_eq!(s, r#"{"servers":{}}"#);
    }

    #[test]
    fn stdio_server_field_order_is_type_command_args_env() {
        // Canonical stdio entry shape: type, command, args, env, [no url/headers].
        let mut env = BTreeMap::new();
        env.insert("PATH".into(), "/usr/bin".into());
        env.insert("DEBUG".into(), "1".into());
        let server = VSCodeServer {
            kind: "stdio".into(),
            command: "/bin/app".into(),
            args: Some(vec!["mcp".into(), "start".into()]),
            env: Some(env),
            url: None,
            headers: None,
        };
        let s = serde_json::to_string(&server).expect("serialize");
        assert_eq!(
            s,
            r#"{"type":"stdio","command":"/bin/app","args":["mcp","start"],"env":{"DEBUG":"1","PATH":"/usr/bin"}}"#
        );
    }

    #[test]
    fn empty_type_skipped_on_write() {
        // type defaults to empty on read; on write, an empty `kind` is
        // skipped so consumers that round-trip a non-stdio entry without
        // touching `kind` don't get a spurious `"type":""` field.
        let server = VSCodeServer {
            kind: String::new(),
            command: "/bin/app".into(),
            args: None,
            env: None,
            url: None,
            headers: None,
        };
        let s = serde_json::to_string(&server).expect("serialize");
        assert_eq!(s, r#"{"command":"/bin/app"}"#);
    }

    #[test]
    fn url_and_headers_round_trip() {
        // A non-stdio entry with url + headers must round-trip without loss.
        // brontes never writes these fields directly, but the editor may.
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer x".into());
        let server = VSCodeServer {
            kind: "sse".into(),
            command: String::new(),
            args: None,
            env: None,
            url: Some("https://example.com/mcp".into()),
            headers: Some(headers),
        };
        let s = serde_json::to_string(&server).expect("serialize");
        // Order: type, [command-skipped], [args-skipped], [env-skipped], url, headers.
        assert_eq!(
            s,
            r#"{"type":"sse","url":"https://example.com/mcp","headers":{"Authorization":"Bearer x"}}"#
        );
        let parsed: VSCodeServer = serde_json::from_str(&s).expect("parse");
        assert_eq!(parsed.kind, "sse");
        assert_eq!(parsed.command, "");
        assert_eq!(parsed.url.as_deref(), Some("https://example.com/mcp"));
        assert_eq!(
            parsed.headers.as_ref().and_then(|h| h.get("Authorization")),
            Some(&"Bearer x".to_string())
        );
    }

    #[test]
    fn inputs_round_trip_preserves_both_password_states() {
        // Round-trip fixture must include both password=true
        // and password=false entries. password=false must be omitted from
        // the on-disk JSON (`omitempty`); both must reparse correctly.
        let raw = r#"{
            "inputs": [
                {"type": "promptString", "id": "api-key", "description": "API key", "password": true},
                {"type": "promptString", "id": "username", "description": "Username", "password": false}
            ],
            "servers": {
                "existing": {"type": "stdio", "command": "/bin/x"}
            }
        }"#;
        let cfg: VSCodeConfig = serde_json::from_str(raw).expect("parse");
        assert_eq!(cfg.inputs.len(), 2);
        assert!(cfg.inputs[0].password);
        assert!(!cfg.inputs[1].password);
        assert!(cfg.has_server("existing"));

        // Write and parse back: order preserved, password=false dropped on write.
        let s = serde_json::to_string(&cfg).expect("serialize");
        // The second input must NOT include the `password` field (omitempty).
        assert!(
            !s.contains(r#""password":false"#),
            "password=false must be omitted, got {s}"
        );
        // The first input MUST include `password:true`.
        assert!(
            s.contains(r#""password":true"#),
            "password=true must be present, got {s}"
        );

        // Reparse: round-trip closes.
        let cfg2: VSCodeConfig = serde_json::from_str(&s).expect("reparse");
        assert_eq!(cfg2.inputs.len(), 2);
        assert!(cfg2.inputs[0].password);
        assert!(!cfg2.inputs[1].password);
        assert!(cfg2.has_server("existing"));
    }

    #[test]
    fn add_remove_server_round_trip() {
        let mut cfg = VSCodeConfig::default();
        assert!(!cfg.has_server("foo"));
        cfg.add_server(
            "foo".into(),
            VSCodeServer {
                kind: "stdio".into(),
                command: "/x".into(),
                args: None,
                env: None,
                url: None,
                headers: None,
            },
        );
        assert!(cfg.has_server("foo"));
        cfg.remove_server("foo");
        assert!(!cfg.has_server("foo"));
    }
}
