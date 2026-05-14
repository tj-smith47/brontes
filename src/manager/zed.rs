//! Zed `settings.json` shape for the `context_servers` MCP block.
//!
//! Zed reuses its general editor `settings.json` for MCP configuration —
//! themes, keybindings, font preferences, and a `context_servers` map for
//! local- and remote-transport MCP servers all live in the same file. That
//! makes Zed structurally different from Claude / Cursor / `VSCode`:
//!
//! - **Unknown top-level keys must round-trip verbatim.** A `mcp zed enable`
//!   that wipes the user's `theme`, `font_family`, or `keymap` because they
//!   were not in our typed struct would be catastrophic. The
//!   [`ZedConfig::other`] field captures every non-`context_servers` key as
//!   a [`serde_json::Value`] and writes them back unchanged on save.
//!
//! - **The file is JSONC.** Zed's default `settings.json` ships with
//!   extensive `// ...` line comments and `/* ... */` block comments, plus
//!   trailing commas. `serde_json` rejects all three. The
//!   [`EditorConfig::preprocess`](super::EditorConfig::preprocess) hook
//!   strips them before deserialization; after our first write the file is
//!   strict JSON (comments are lost) — same trade-off ophis accepts. The
//!   user's content keys still round-trip; only the comments on top of them
//!   are sacrificed.
//!
//! - **`context_servers` is a top-level key**, not a nested `mcpServers`
//!   (Claude / Cursor) or `servers` (`VSCode`).
//!
//! The per-server field shape (Zed docs):
//!
//! ```jsonc
//! {
//!   "context_servers": {
//!     "local-mcp-server":  { "command": "...", "args": [...], "env": {} },
//!     "remote-mcp-server": { "url": "...", "headers": {...} }
//!   }
//! }
//! ```
//!
//! Local servers carry `command` + optional `args` + optional `env`. Remote
//! servers carry `url` + optional `headers`. brontes only ever writes the
//! local-server shape (`mcp zed enable` mints a stdio child), but the
//! remote-server fields exist on [`ZedServer`] so a load-mutate-save round
//! trip preserves them when a user has hand-edited a remote entry.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::EditorConfig;

/// Top-level shape of Zed's `settings.json` from brontes's point of view.
///
/// The typed `context_servers` map is the only field brontes reads or
/// writes; every other key in the file is captured by the flattened
/// [`other`](Self::other) [`BTreeMap`] of [`serde_json::Value`] and written
/// back unchanged on save. Empty `context_servers` is `omitempty` so a
/// `disable` of the last MCP server leaves a configfile that does not carry
/// a stray empty object key Zed might otherwise complain about.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct ZedConfig {
    /// Configured MCP context servers, keyed by server name. Empty maps
    /// collapse to no JSON key (`omitempty`) so disabling the last server
    /// removes the `context_servers` entry entirely.
    #[serde(
        default,
        rename = "context_servers",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub(crate) context_servers: BTreeMap<String, ZedServer>,

    /// Pass-through capture of every other top-level key in the file.
    ///
    /// Zed `settings.json` carries the user's editor configuration —
    /// `theme`, `font_family`, `tab_size`, `keymap`, `language`, etc. —
    /// alongside `context_servers`. We have no business interpreting any of
    /// them; we just deserialize them as opaque [`serde_json::Value`]s and
    /// serialize them back unchanged so a round trip is byte-stable
    /// (modulo JSONC comments, which the preprocess hook strips on load —
    /// see the module docs).
    #[serde(flatten)]
    pub(crate) other: BTreeMap<String, serde_json::Value>,
}

/// One entry under `context_servers` in Zed's `settings.json`.
///
/// Field order on the struct mirrors the order in the published Zed docs
/// (`command, args, env, url, headers`) so a serialized entry reads the
/// same as the docs example.
///
/// brontes always writes the local-stdio shape (`command` + optional
/// `args` + optional `env`); the `url` / `headers` fields exist purely
/// so a load-mutate-save round trip preserves a hand-edited remote entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ZedServer {
    /// Absolute path to the executable Zed spawns. `omitempty` so a
    /// remote-only entry (`url` + `headers`, no `command`) round-trips
    /// without a spurious `"command":""` field appearing on write.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) command: String,
    /// Argv tail (e.g. `["mcp", "start"]`). `None` (or empty `Some(vec![])`)
    /// collapses to no JSON key on write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) args: Option<Vec<String>>,
    /// Per-server environment variables. `None` (or empty map) collapses to
    /// no JSON key. Backed by [`BTreeMap`] so the on-disk key order is
    /// stable across runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) env: Option<BTreeMap<String, String>>,
    /// URL for remote-transport servers (`omitempty`). brontes never writes
    /// this field; it is captured purely so a user-authored remote entry
    /// round-trips without loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
    /// HTTP headers for remote-transport servers (`omitempty`). brontes
    /// never writes this field; it exists for round-trip fidelity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) headers: Option<BTreeMap<String, String>>,
}

impl EditorConfig for ZedConfig {
    type Server = ZedServer;

    fn has_server(&self, name: &str) -> bool {
        self.context_servers.contains_key(name)
    }

    fn add_server(&mut self, name: String, server: Self::Server) {
        self.context_servers.insert(name, server);
    }

    fn remove_server(&mut self, name: &str) {
        self.context_servers.remove(name);
    }

    fn server_names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.context_servers.keys().map(String::as_str))
    }

    fn preprocess(bytes: Vec<u8>) -> Vec<u8> {
        strip_jsonc(&bytes)
    }
}

/// Strip JSONC syntax (line comments, block comments, trailing commas)
/// from a byte slice and return the equivalent strict-JSON bytes.
///
/// Implemented as a small state machine that tracks whether the cursor is
/// inside a `"..."` string so JSON-looking sequences inside strings — like
/// `"http://example.com"` (the `//` does NOT start a comment) — pass
/// through verbatim. Escape sequences inside strings are honored only
/// enough to keep the string-vs-non-string distinction correct;
/// `serde_json` validates the actual escape semantics downstream.
///
/// Trailing-comma handling: a `,` is dropped when the next non-whitespace
/// byte is `]` or `}`. Comments inside the lookahead window are not
/// supported (rare in practice); we conservatively keep the comma rather
/// than risk skipping past a comment and misjudging the close.
///
/// Idempotent on strict JSON: a strict-JSON input produces the same bytes
/// out. The function does NOT validate JSON — invalid input returns
/// invalid (but stripped) output; `serde_json` surfaces the parse error.
fn strip_jsonc(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i: usize = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            out.push(b);
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            in_string = true;
            out.push(b);
            i += 1;
            continue;
        }
        // `// ...` line comment — skip to end of line (keep the newline so
        // line-number diagnostics from serde_json stay roughly aligned).
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // `/* ... */` block comment.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            // Step past the closing `*/`, clamped to the end of input.
            i = i.saturating_add(2).min(bytes.len());
            continue;
        }
        // Trailing comma: drop a `,` whose next non-whitespace byte is
        // `]` or `}`. We do not look through comments here — that case is
        // rare and the worst it produces is a comma left in place, which
        // serde_json will then reject (the user can re-save to clean it).
        if b == b',' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b']' || bytes[j] == b'}') {
                i += 1;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_serializes_to_empty_object() {
        // Empty `context_servers` is `omitempty`; no `other` keys. Result
        // is an empty JSON object — Zed will treat it the same as an
        // absent file.
        let cfg = ZedConfig::default();
        let s = serde_json::to_string(&cfg).expect("serialize");
        assert_eq!(s, "{}");
    }

    #[test]
    fn local_server_field_order_is_command_args_env() {
        let mut env = BTreeMap::new();
        env.insert("DEBUG".into(), "1".into());
        env.insert("PATH".into(), "/usr/bin".into());
        let server = ZedServer {
            command: "/bin/app".into(),
            args: Some(vec!["mcp".into(), "start".into()]),
            env: Some(env),
            url: None,
            headers: None,
        };
        let s = serde_json::to_string(&server).expect("serialize");
        // Order: command, args, env, [url-skipped, headers-skipped].
        // env keys are sorted by BTreeMap.
        assert_eq!(
            s,
            r#"{"command":"/bin/app","args":["mcp","start"],"env":{"DEBUG":"1","PATH":"/usr/bin"}}"#
        );
    }

    #[test]
    fn remote_server_only_carries_url_and_headers() {
        // A remote entry without `command` must NOT emit `"command":""` on
        // write — empty `command` is `omitempty` for round-trip fidelity.
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer x".into());
        let server = ZedServer {
            command: String::new(),
            args: None,
            env: None,
            url: Some("https://example.com/mcp".into()),
            headers: Some(headers),
        };
        let s = serde_json::to_string(&server).expect("serialize");
        assert_eq!(
            s,
            r#"{"url":"https://example.com/mcp","headers":{"Authorization":"Bearer x"}}"#
        );
    }

    #[test]
    fn round_trip_preserves_unknown_top_level_keys() {
        // The on-disk file has theme + font_family + context_servers; after
        // a read-mutate-write cycle the theme/font keys MUST be preserved
        // verbatim (this is the whole reason ZedConfig has an `other` flat
        // map; a typed struct that did not capture unknown keys would wipe
        // them on save).
        let raw = r#"{
            "theme": "One Dark",
            "font_family": "JetBrains Mono",
            "context_servers": {
                "existing": {"command": "/bin/x"}
            }
        }"#;
        let cfg: ZedConfig = serde_json::from_str(raw).expect("parse");
        assert!(cfg.has_server("existing"));
        assert_eq!(
            cfg.other.get("theme").and_then(|v| v.as_str()),
            Some("One Dark")
        );
        assert_eq!(
            cfg.other.get("font_family").and_then(|v| v.as_str()),
            Some("JetBrains Mono")
        );

        let s = serde_json::to_string(&cfg).expect("serialize");
        // Both passthrough keys MUST appear in the serialized output.
        assert!(s.contains(r#""theme":"One Dark""#), "got {s}");
        assert!(s.contains(r#""font_family":"JetBrains Mono""#), "got {s}");
        assert!(s.contains(r#""context_servers""#), "got {s}");
    }

    #[test]
    fn add_remove_server_does_not_touch_other_keys() {
        // Driving `add_server` / `remove_server` through the trait must
        // leave the pass-through `other` keys untouched. This is the
        // exact mutation `Manager::enable_server` performs.
        let mut cfg = ZedConfig::default();
        cfg.other.insert(
            "theme".into(),
            serde_json::Value::String("Solarized".into()),
        );

        cfg.add_server(
            "foo".into(),
            ZedServer {
                command: "/bin/foo".into(),
                args: None,
                env: None,
                url: None,
                headers: None,
            },
        );
        assert!(cfg.has_server("foo"));
        assert_eq!(
            cfg.other.get("theme").and_then(|v| v.as_str()),
            Some("Solarized"),
            "add_server must not touch `other`"
        );

        cfg.remove_server("foo");
        assert!(!cfg.has_server("foo"));
        assert_eq!(
            cfg.other.get("theme").and_then(|v| v.as_str()),
            Some("Solarized"),
            "remove_server must not touch `other`"
        );
    }

    #[test]
    fn server_names_iterates_keys_in_sorted_order() {
        // `BTreeMap` sorts keys; `server_names` is what `mcp zed list`
        // prints, so the output order must be stable and alphabetical.
        let mut cfg = ZedConfig::default();
        cfg.add_server(
            "zebra".into(),
            ZedServer {
                command: "/z".into(),
                ..Default::default()
            },
        );
        cfg.add_server(
            "alpha".into(),
            ZedServer {
                command: "/a".into(),
                ..Default::default()
            },
        );
        let names: Vec<&str> = cfg.server_names().collect();
        assert_eq!(names, vec!["alpha", "zebra"]);
    }

    // ── strip_jsonc: comments and trailing commas ─────────────────────

    #[test]
    fn strip_jsonc_removes_line_comments() {
        let raw = "// header\n{\"a\":1} // trailing\n";
        let out = strip_jsonc(raw.as_bytes());
        let s = std::str::from_utf8(&out).expect("utf8");
        // Comments gone; the object survives intact.
        assert!(!s.contains("header"), "got {s:?}");
        assert!(!s.contains("trailing"), "got {s:?}");
        let v: serde_json::Value = serde_json::from_slice(&out).expect("parse");
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn strip_jsonc_removes_block_comments() {
        let raw = "/* block */ {\"a\":1, /* inline */ \"b\":2}";
        let out = strip_jsonc(raw.as_bytes());
        let v: serde_json::Value = serde_json::from_slice(&out).expect("parse");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_jsonc_preserves_comment_syntax_inside_strings() {
        // The classic JSONC bug: a URL like "http://x" contains `//` but
        // it is INSIDE a string and must not be treated as a line comment.
        let raw = r#"{"url":"http://example.com/mcp"}"#;
        let out = strip_jsonc(raw.as_bytes());
        let v: serde_json::Value = serde_json::from_slice(&out).expect("parse");
        assert_eq!(v["url"], "http://example.com/mcp");
    }

    #[test]
    fn strip_jsonc_drops_trailing_comma_in_array_and_object() {
        let raw = "{\"a\":[1,2,3,], \"b\":{\"x\":1,}}";
        let out = strip_jsonc(raw.as_bytes());
        let v: serde_json::Value = serde_json::from_slice(&out).expect("parse");
        assert_eq!(v["a"], serde_json::json!([1, 2, 3]));
        assert_eq!(v["b"]["x"], 1);
    }

    #[test]
    fn strip_jsonc_keeps_non_trailing_commas() {
        // A comma between two values is NOT trailing and must survive.
        let raw = r"[1, 2, 3]";
        let out = strip_jsonc(raw.as_bytes());
        assert_eq!(out, raw.as_bytes());
    }

    #[test]
    fn strip_jsonc_preserves_escaped_quote_inside_strings() {
        // An escaped quote (\\\") must not close the string and trigger
        // comment scanning on the bytes that follow inside the string.
        let raw = r#"{"a":"with \"quote\" and // not-comment"}"#;
        let out = strip_jsonc(raw.as_bytes());
        let v: serde_json::Value = serde_json::from_slice(&out).expect("parse");
        assert_eq!(v["a"], "with \"quote\" and // not-comment");
    }

    #[test]
    fn strip_jsonc_idempotent_on_strict_json() {
        // A strict-JSON input must round-trip through strip_jsonc unchanged.
        let raw = r#"{"a":1,"b":[2,3],"c":"x"}"#;
        let out = strip_jsonc(raw.as_bytes());
        assert_eq!(out, raw.as_bytes());
    }

    #[test]
    fn preprocess_enables_jsonc_parsing_through_editor_config_trait() {
        // `EditorConfig::preprocess` is the trait hook `Manager::load` calls
        // before deserialization. A JSONC-laden Zed settings.json must
        // parse cleanly through ZedConfig once the hook strips comments
        // and trailing commas.
        let raw = br#"// User comment
        {
            "theme": "One Dark", // theme choice
            "context_servers": {
                "existing": {"command": "/bin/x",},
            },
        }"#;
        let preprocessed = ZedConfig::preprocess(raw.to_vec());
        let cfg: ZedConfig = serde_json::from_slice(&preprocessed).expect("parse");
        assert!(cfg.has_server("existing"));
        assert_eq!(
            cfg.other.get("theme").and_then(|v| v.as_str()),
            Some("One Dark")
        );
    }
}
