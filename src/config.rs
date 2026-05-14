//! User-facing configuration for the brontes MCP subtree.
//!
//! [`Config`] is the central configuration type consumed by
//! `brontes::generate_tools` and the `mcp` subcommand.  It is built via
//! fluent builder methods; [`Config::default()`] is a valid zero-config
//! starting point.
//!
//! # Quick start
//!
//! ```rust
//! use std::sync::Arc;
//! use brontes::{Config, Selector};
//!
//! let cfg = Config::default()
//!     .command_name("agent")
//!     .selector(Selector {
//!         cmd: Some(Arc::new(|p: &str| p.starts_with("my-cli deploy"))),
//!         ..Default::default()
//!     })
//!     .log_level(tracing::Level::DEBUG);
//! ```

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use tracing::Level;

use crate::annotations::ToolAnnotations;
use crate::schema::SchemaType;
use crate::selector::Selector;

/// Which clap field provides the primary text for an MCP tool description.
///
/// Resolution always falls back to the other field if the preferred one is
/// unset; if both are absent, brontes substitutes
/// `"Execute the {name} command"`.  An `after_help` "Examples:" block, when
/// present, is appended to whichever mode produced the primary text.
///
/// # Defaults
///
/// [`DescriptionMode::Long`] preserves brontes' historical behavior:
/// `long_about` is preferred, with `about` as the fallback.  Switch to
/// [`DescriptionMode::Short`] when MCP tool descriptions are dominated by
/// verbose `long_about` text that wastes the LLM's context budget.
///
/// # Surgical override
///
/// For one-off commands whose default-mode output is wrong, prefer
/// [`Config::description_mode_for`] or [`Config::description`] over
/// flipping the global default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DescriptionMode {
    /// Prefer `cmd.about`, fall back to `cmd.long_about`.
    ///
    /// Best for token efficiency when most commands' `long_about` text
    /// duplicates or trivially expands `about`.
    Short,

    /// Prefer `cmd.long_about`, fall back to `cmd.about`.  Default.
    ///
    /// Best when `long_about` carries information the LLM benefits from
    /// (usage caveats, prerequisites) that wouldn't fit in the short
    /// `about` line.
    #[default]
    Long,
}

/// User-facing configuration for the `mcp` subtree.
///
/// Held alongside the user's [`clap::Command`] tree; consumed by
/// `brontes::generate_tools` and the `mcp` subcommand.
///
/// [`Config::default()`] yields the same behavior as passing `None` — every
/// command that passes the safety filters becomes a tool, with no annotations
/// or overrides applied.
///
/// # Builder pattern
///
/// All fields are set via fluent builder methods.  Each method consumes `self`
/// and returns the updated `Config`, so calls can be chained:
///
/// ```rust
/// use brontes::Config;
///
/// let cfg = Config::default()
///     .command_name("agent")
///     .log_level(tracing::Level::INFO);
/// ```
///
/// # Forward compatibility
///
/// `Config` is `#[non_exhaustive]`. Construct it via [`Config::default()`] and
/// the fluent builder methods on this type — never via struct-literal syntax
/// (`Config { .. }`) from outside this crate. New fields may be added in
/// minor releases without bumping the major version; the builder methods are
/// the stable surface.
#[derive(Default, Clone)]
#[non_exhaustive]
pub struct Config {
    /// The subcommand name brontes registers on the user's CLI.
    ///
    /// `None` defaults to `"mcp"`.  Rename when your CLI already contains a
    /// command whose path includes the substring `"mcp"` (e.g.,
    /// `myapp mcp install`) and you want to avoid a collision with the brontes
    /// subtree — set this to `"agent"` or another unused name.
    pub command_name: Option<String>,

    /// Tool-name prefix substituted for the root command name when
    /// constructing each MCP tool's name.
    ///
    /// `None` means "use the root command's `get_name()`".
    pub tool_name_prefix: Option<String>,

    /// Selectors evaluated first-match-wins against each candidate command.
    ///
    /// An empty list means every command passing the safety filters becomes a
    /// tool.  When the list is non-empty, a command must be claimed by at least
    /// one selector to appear in the tool list.
    pub selectors: Vec<Selector>,

    /// Default environment variables merged into every tool call's environment.
    ///
    /// Per-call `env` overrides (set by the MCP client at invocation time) win
    /// on key conflict.  An empty merged map (no default entries AND no per-call
    /// entries) is expected to be omitted from the MCP wire payload — that
    /// omission is enforced by the tool-call builder, not by `Config` itself.
    pub default_env: HashMap<String, String>,

    /// Per-command MCP annotation hints, keyed by full command path
    /// (e.g., `"my-cli list"`).
    pub annotations: HashMap<String, ToolAnnotations>,

    /// Commands marked deprecated, keyed by full command path.
    ///
    /// Deprecated commands are filtered out at tool-list generation time,
    /// mirroring cobra's `Deprecated` field (which clap does not have a
    /// direct equivalent for).
    pub deprecated_commands: HashSet<String>,

    /// Per-flag JSON Schema overrides, keyed by `(command_path, flag_name)`.
    ///
    /// The provided value replaces the auto-derived schema for that flag
    /// wholesale; auto default/required/enum extraction is skipped for
    /// any flag that has an entry here.
    pub flag_schemas: HashMap<(String, String), Value>,

    /// Coarse per-flag type overrides for flags brontes cannot introspect.
    ///
    /// Useful when a flag uses a custom `value_parser` function whose return
    /// type is not visible to brontes's type-ID lookup.  Keyed by
    /// `(command_path, flag_name)`.
    pub flag_type_overrides: HashMap<(String, String), SchemaType>,

    /// Logging level for the MCP server's tracing subscriber.
    ///
    /// `None` falls through to `RUST_LOG`, then to `INFO`.  The `--log-level`
    /// flag on `mcp start` / `mcp stream` / `mcp tools` wins over this value.
    pub log_level: Option<Level>,

    /// MCP `Implementation` identity (server name and version) surfaced to MCP
    /// clients.
    ///
    /// `None` uses [`rmcp::model::Implementation::default()`], which derives
    /// values from `CARGO_PKG_NAME` / `CARGO_PKG_VERSION` at build time.
    pub implementation: Option<rmcp::model::Implementation>,

    /// Global default for which clap field becomes the MCP tool description.
    ///
    /// Defaults to [`DescriptionMode::Long`].  Override per-command via
    /// [`Config::description_mode_for`], or replace the entire description
    /// for a specific command via [`Config::description`].
    pub description_mode: DescriptionMode,

    /// Per-command [`DescriptionMode`] overrides, keyed by full command path.
    ///
    /// Entries here win over [`Config::description_mode`].  A
    /// [`Config::description`] entry for the same path wins over this map.
    pub description_modes: HashMap<String, DescriptionMode>,

    /// Per-command full-description overrides, keyed by full command path.
    ///
    /// When set, the stored text replaces the entire MCP tool description —
    /// the `long_about`/`about`/`after_help` cascade is bypassed for that
    /// command.  Use this to surface LLM-specific guidance that doesn't
    /// belong in the CLI's `--help` output.
    pub descriptions: HashMap<String, String>,
}

impl Config {
    /// Set the subcommand name brontes registers on the CLI.
    ///
    /// The name defaults to `"mcp"` when not set.  Use this when your CLI
    /// already contains a path that includes the substring `"mcp"` and you
    /// need to avoid a collision.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().command_name("agent");
    /// assert_eq!(cfg.command_name.as_deref(), Some("agent"));
    /// ```
    #[must_use]
    pub fn command_name(mut self, name: impl Into<String>) -> Self {
        self.command_name = Some(name.into());
        self
    }

    /// Set the tool-name prefix used when constructing MCP tool names.
    ///
    /// Defaults to the root command's `get_name()` value when not set.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().tool_name_prefix("myapp");
    /// assert_eq!(cfg.tool_name_prefix.as_deref(), Some("myapp"));
    /// ```
    #[must_use]
    pub fn tool_name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.tool_name_prefix = Some(prefix.into());
        self
    }

    /// Append a [`Selector`] to the selector list.
    ///
    /// Selectors are evaluated in the order they are added.  The first
    /// selector whose `cmd` matcher accepts a command claims it.
    ///
    /// ```rust
    /// use brontes::{Config, Selector};
    ///
    /// let cfg = Config::default()
    ///     .selector(Selector::default())
    ///     .selector(Selector::default());
    /// assert_eq!(cfg.selectors.len(), 2);
    /// ```
    #[must_use]
    pub fn selector(mut self, s: Selector) -> Self {
        self.selectors.push(s);
        self
    }

    /// Insert a default environment variable.
    ///
    /// Calling this method multiple times with different keys accumulates
    /// entries.  Per-call overrides from the MCP client win on conflict.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().default_env("LOG_FORMAT", "json");
    /// assert_eq!(cfg.default_env.get("LOG_FORMAT").map(String::as_str), Some("json"));
    /// ```
    #[must_use]
    pub fn default_env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.default_env.insert(k.into(), v.into());
        self
    }

    /// Attach [`ToolAnnotations`] to the command at `cmd_path`.
    ///
    /// `cmd_path` is the full space-joined path of the command, e.g.
    /// `"my-cli list"`.
    ///
    /// ```rust
    /// use brontes::{Config, ToolAnnotations};
    ///
    /// let cfg = Config::default().annotation(
    ///     "my-cli list",
    ///     ToolAnnotations { read_only_hint: Some(true), ..Default::default() },
    /// );
    /// assert!(cfg.annotations.contains_key("my-cli list"));
    /// ```
    #[must_use]
    pub fn annotation(mut self, cmd_path: impl Into<String>, ann: ToolAnnotations) -> Self {
        self.annotations.insert(cmd_path.into(), ann);
        self
    }

    /// Mark a command as deprecated.
    ///
    /// Deprecated commands are excluded from the generated tool list.
    /// `cmd_path` is the full space-joined command path, e.g. `"my-cli oldcmd"`.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().deprecate("my-cli oldcmd");
    /// assert!(cfg.deprecated_commands.contains("my-cli oldcmd"));
    /// ```
    #[must_use]
    pub fn deprecate(mut self, cmd_path: impl Into<String>) -> Self {
        self.deprecated_commands.insert(cmd_path.into());
        self
    }

    /// Replace the auto-derived JSON Schema for a specific flag.
    ///
    /// `cmd_path` is the full space-joined command path and `flag` is the
    /// long flag name (without the leading `--`).  The provided `schema` value
    /// is used as-is; auto default/required/enum extraction is skipped.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().flag_schema(
    ///     "my-cli list",
    ///     "limit",
    ///     serde_json::json!({"type": "integer", "minimum": 0}),
    /// );
    /// assert!(cfg.flag_schemas.contains_key(&("my-cli list".into(), "limit".into())));
    /// ```
    #[must_use]
    pub fn flag_schema(
        mut self,
        cmd_path: impl Into<String>,
        flag: impl Into<String>,
        schema: Value,
    ) -> Self {
        self.flag_schemas
            .insert((cmd_path.into(), flag.into()), schema);
        self
    }

    /// Override the coarse schema type for a flag brontes cannot introspect.
    ///
    /// Use this when a flag uses a custom `value_parser` function whose return
    /// type is opaque to brontes.  `cmd_path` is the full space-joined command
    /// path; `flag` is the long flag name without `--`.
    ///
    /// ```rust
    /// use brontes::{Config, SchemaType};
    ///
    /// let cfg = Config::default().flag_type_override("my-cli list", "filter", SchemaType::Array);
    /// assert!(cfg.flag_type_overrides.contains_key(&("my-cli list".into(), "filter".into())));
    /// ```
    #[must_use]
    pub fn flag_type_override(
        mut self,
        cmd_path: impl Into<String>,
        flag: impl Into<String>,
        ty: SchemaType,
    ) -> Self {
        self.flag_type_overrides
            .insert((cmd_path.into(), flag.into()), ty);
        self
    }

    /// Set the logging level for the MCP server's tracing subscriber.
    ///
    /// The `--log-level` CLI flag wins over this value.  When neither is set,
    /// the subscriber falls through to `RUST_LOG`, then to `INFO`.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().log_level(tracing::Level::DEBUG);
    /// assert_eq!(cfg.log_level, Some(tracing::Level::DEBUG));
    /// ```
    #[must_use]
    pub const fn log_level(mut self, lvl: Level) -> Self {
        self.log_level = Some(lvl);
        self
    }

    /// Set the MCP `Implementation` identity surfaced to MCP clients (server
    /// name, version, optional title/description/URL/icons). Leave unset to
    /// fall through to `rmcp::model::Implementation::default()`, which
    /// derives from `CARGO_PKG_NAME` and `CARGO_PKG_VERSION` of the current
    /// binary.
    ///
    /// Set explicitly when:
    /// - your CLI is rebadged under a different name to MCP clients than its
    ///   binary name (e.g., binary `myapp-cli` but MCP server identifies as
    ///   `"MyApp Agent"`);
    /// - you ship two binaries that should appear distinct to the same MCP
    ///   client (set version or title differently).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use brontes::Config;
    /// use rmcp::model::Implementation;
    ///
    /// let cfg = Config::default()
    ///     .implementation(Implementation::new("my-agent", "0.1.0"));
    /// # let _ = cfg;
    /// ```
    #[must_use]
    pub fn implementation(mut self, imp: rmcp::model::Implementation) -> Self {
        self.implementation = Some(imp);
        self
    }

    /// Set the global default [`DescriptionMode`] for MCP tool descriptions.
    ///
    /// Defaults to [`DescriptionMode::Long`], which preserves brontes'
    /// historical "prefer `long_about`, fall back to `about`" behavior.
    /// Flip to [`DescriptionMode::Short`] when verbose `long_about` text
    /// dominates your tool surface and wastes the LLM's context budget.
    ///
    /// Per-command overrides via [`Config::description_mode_for`] and
    /// full-text overrides via [`Config::description`] both win over this
    /// global setting.
    ///
    /// ```rust
    /// use brontes::{Config, DescriptionMode};
    ///
    /// let cfg = Config::default().description_mode(DescriptionMode::Short);
    /// assert_eq!(cfg.description_mode, DescriptionMode::Short);
    /// ```
    #[must_use]
    pub const fn description_mode(mut self, mode: DescriptionMode) -> Self {
        self.description_mode = mode;
        self
    }

    /// Override [`DescriptionMode`] for a specific command path.
    ///
    /// `cmd_path` is the full space-joined command path (e.g.,
    /// `"my-cli module list"`).  When set, this entry wins over
    /// [`Config::description_mode`] for that one command.  A
    /// [`Config::description`] entry for the same path wins over this.
    ///
    /// ```rust
    /// use brontes::{Config, DescriptionMode};
    ///
    /// let cfg = Config::default()
    ///     .description_mode_for("my-cli module list", DescriptionMode::Short);
    /// assert_eq!(
    ///     cfg.description_modes.get("my-cli module list"),
    ///     Some(&DescriptionMode::Short),
    /// );
    /// ```
    #[must_use]
    pub fn description_mode_for(
        mut self,
        cmd_path: impl Into<String>,
        mode: DescriptionMode,
    ) -> Self {
        self.description_modes.insert(cmd_path.into(), mode);
        self
    }

    /// Replace the entire MCP tool description for a specific command path.
    ///
    /// `cmd_path` is the full space-joined command path; `text` is the literal
    /// description string sent to MCP clients.  When set, the stored text
    /// bypasses the `long_about`/`about`/`after_help` cascade entirely for
    /// that command — useful for surfacing LLM-specific guidance (preconditions,
    /// "always pair with --dry-run", etc.) that doesn't belong in the CLI's
    /// human-facing `--help` output.
    ///
    /// Wins over both [`Config::description_mode`] and
    /// [`Config::description_mode_for`].
    ///
    /// Empty / whitespace-only `text` is rejected at
    /// [`crate::generate_tools`] time as [`crate::Error::Config`] — an empty
    /// description is useless for LLM tool selection.
    ///
    /// `text` is stored verbatim — caller is responsible for trimming. The
    /// native cascade applies `trim_end` to the `after_help` "Examples:" block,
    /// but the literal override passes whitespace through as-given so callers
    /// retain full control over the exact bytes sent to MCP clients.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().description(
    ///     "my-cli apply",
    ///     "Apply config changes. Always run with --dry-run first to preview drift.",
    /// );
    /// assert!(cfg.descriptions.contains_key("my-cli apply"));
    /// ```
    #[must_use]
    pub fn description(mut self, cmd_path: impl Into<String>, text: impl Into<String>) -> Self {
        self.descriptions.insert(cmd_path.into(), text.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn default_yields_empty_config() {
        let cfg = Config::default();
        assert!(cfg.command_name.is_none());
        assert!(cfg.tool_name_prefix.is_none());
        assert!(cfg.selectors.is_empty());
        assert!(cfg.default_env.is_empty());
        assert!(cfg.annotations.is_empty());
        assert!(cfg.deprecated_commands.is_empty());
        assert!(cfg.flag_schemas.is_empty());
        assert!(cfg.flag_type_overrides.is_empty());
        assert!(cfg.log_level.is_none());
        assert!(cfg.implementation.is_none());
    }

    #[test]
    fn command_name_sets_field() {
        let cfg = Config::default().command_name("agent");
        assert_eq!(cfg.command_name.as_deref(), Some("agent"));
    }

    #[test]
    fn tool_name_prefix_sets_field() {
        let cfg = Config::default().tool_name_prefix("myapp");
        assert_eq!(cfg.tool_name_prefix.as_deref(), Some("myapp"));
    }

    #[test]
    fn selector_pushes_in_order() {
        let cfg = Config::default()
            .selector(Selector {
                cmd: Some(Arc::new(|p: &str| p == "first")),
                ..Default::default()
            })
            .selector(Selector {
                cmd: Some(Arc::new(|p: &str| p == "second")),
                ..Default::default()
            });
        assert_eq!(cfg.selectors.len(), 2);
        assert!((cfg.selectors[0].cmd.as_ref().unwrap())("first"));
        assert!((cfg.selectors[1].cmd.as_ref().unwrap())("second"));
    }

    #[test]
    fn default_env_inserts_key_value() {
        let cfg = Config::default().default_env("LOG_FORMAT", "json");
        assert_eq!(
            cfg.default_env.get("LOG_FORMAT").map(String::as_str),
            Some("json")
        );
    }

    #[test]
    fn default_env_accumulates_multiple_entries() {
        let cfg = Config::default()
            .default_env("K1", "V1")
            .default_env("K2", "V2");
        assert_eq!(cfg.default_env.len(), 2);
        assert_eq!(cfg.default_env.get("K1").map(String::as_str), Some("V1"));
        assert_eq!(cfg.default_env.get("K2").map(String::as_str), Some("V2"));
    }

    #[test]
    fn annotation_inserts_by_path() {
        let cfg = Config::default().annotation(
            "my-cli list",
            ToolAnnotations {
                read_only_hint: Some(true),
                ..Default::default()
            },
        );
        assert!(cfg.annotations.contains_key("my-cli list"));
        assert_eq!(cfg.annotations["my-cli list"].read_only_hint, Some(true));
    }

    #[test]
    fn deprecate_inserts_path() {
        let cfg = Config::default().deprecate("my-cli oldcmd");
        assert!(cfg.deprecated_commands.contains("my-cli oldcmd"));
    }

    #[test]
    fn flag_schema_inserts_by_key() {
        let schema = serde_json::json!({"type": "integer", "minimum": 0});
        let cfg = Config::default().flag_schema("my-cli list", "limit", schema.clone());
        let key = ("my-cli list".to_string(), "limit".to_string());
        assert!(cfg.flag_schemas.contains_key(&key));
        assert_eq!(cfg.flag_schemas[&key], schema);
    }

    #[test]
    fn flag_type_override_inserts_by_key() {
        let cfg = Config::default().flag_type_override("my-cli list", "filter", SchemaType::Array);
        let key = ("my-cli list".to_string(), "filter".to_string());
        assert!(cfg.flag_type_overrides.contains_key(&key));
        assert_eq!(cfg.flag_type_overrides[&key], SchemaType::Array);
    }

    #[test]
    fn log_level_sets_field() {
        let cfg = Config::default().log_level(Level::DEBUG);
        assert_eq!(cfg.log_level, Some(Level::DEBUG));
    }

    #[test]
    fn implementation_sets_field() {
        let imp = rmcp::model::Implementation::new("test-server", "0.1.0");
        let cfg = Config::default().implementation(imp);
        assert!(cfg.implementation.is_some());
        let stored = cfg.implementation.unwrap();
        assert_eq!(stored.name, "test-server");
        assert_eq!(stored.version, "0.1.0");
    }

    #[test]
    fn default_env_last_writer_wins() {
        // Calling `.default_env()` twice on the same key should leave the
        // second value in place. This pins HashMap::insert override
        // semantics so a future refactor (e.g., switching to entry().or_insert())
        // gets caught.
        let cfg = Config::default()
            .default_env("X", "1")
            .default_env("X", "2");
        assert_eq!(cfg.default_env.get("X").map(String::as_str), Some("2"));
        assert_eq!(cfg.default_env.len(), 1);
    }

    #[test]
    fn annotation_last_writer_wins() {
        // Calling `.annotation()` twice on the same command path should
        // replace the prior annotation. Pins HashMap::insert override
        // semantics for the annotations map.
        let cfg = Config::default()
            .annotation(
                "my-cli list",
                ToolAnnotations {
                    read_only_hint: Some(true),
                    ..Default::default()
                },
            )
            .annotation(
                "my-cli list",
                ToolAnnotations {
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    ..Default::default()
                },
            );
        let ann = cfg
            .annotations
            .get("my-cli list")
            .expect("annotation present");
        assert_eq!(ann.read_only_hint, Some(false));
        assert_eq!(ann.destructive_hint, Some(true));
        assert_eq!(cfg.annotations.len(), 1);
    }

    #[test]
    fn fluent_chain_composes() {
        let cfg = Config::default()
            .command_name("agent")
            .selector(Selector::default())
            .annotation(
                "my-cli list",
                ToolAnnotations {
                    read_only_hint: Some(true),
                    ..Default::default()
                },
            )
            .deprecate("my-cli oldcmd")
            .flag_schema(
                "my-cli list",
                "limit",
                serde_json::json!({"type": "integer", "minimum": 0}),
            )
            .flag_type_override("my-cli list", "filter", SchemaType::Array)
            .log_level(Level::DEBUG);

        assert_eq!(cfg.command_name.as_deref(), Some("agent"));
        assert_eq!(cfg.selectors.len(), 1);
        assert!(cfg.annotations.contains_key("my-cli list"));
        assert!(cfg.deprecated_commands.contains("my-cli oldcmd"));
        assert!(
            cfg.flag_schemas
                .contains_key(&("my-cli list".into(), "limit".into()))
        );
        assert!(
            cfg.flag_type_overrides
                .contains_key(&("my-cli list".into(), "filter".into()))
        );
        assert_eq!(cfg.log_level, Some(Level::DEBUG));
    }
}
