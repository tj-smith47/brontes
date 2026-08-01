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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use rmcp::model::CacheScope;
use serde_json::Value;
use tracing::Level;

use crate::annotations::ToolAnnotations;
use crate::schema::SchemaType;
use crate::selector::Selector;
use crate::toolset::{Group, ToolFilter};

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

/// Whether a `tools/call` runs to completion inline or is handed back as a
/// task handle (SEP-2663, `io.modelcontextprotocol/tasks`).
///
/// The mode only ever applies to a client that declared the tasks extension.
/// A client without it always receives the blocking result, whatever the mode
/// says, because a task handle is a shape it cannot parse.
///
/// # Defaults
///
/// [`TaskMode::Blocking`] is the default and matches every brontes release
/// before the extension existed: `tools/call` returns when the process exits.
/// That is fine for commands measured in seconds and wrong for the ones this
/// library exists to wrap — a release, a build, a deploy — where the call
/// occupies the client for minutes with no way to check on it or stop it.
///
/// # Surgical override
///
/// Prefer [`Config::task_mode_for`] on the handful of long-running commands
/// over flipping the global default: a task costs the client an extra poll
/// round trip, which is pure overhead on a command that returns immediately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TaskMode {
    /// Run the command inline; `tools/call` returns the finished result.
    /// Default.
    #[default]
    Blocking,

    /// Return a task handle immediately and run the command in the
    /// background.  The client polls `tasks/get` for progress and the final
    /// result, answers any input request with `tasks/update`, and can stop
    /// the command with `tasks/cancel`.
    Detached,
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
/// # Command paths
///
/// Every builder that takes a `cmd_path` reads it relative to the CLI's own
/// name, which brontes supplies from the command tree: `"release notes"` and
/// `"my-cli release notes"` name the same command. Paths are space-joined and
/// matched on whole segments.
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

    /// Flags promoted out of `flags` to a top-level input-schema property
    /// carrying SEP-2243's `x-mcp-header`, keyed by `(command_path,
    /// flag_name)` with the header suffix as the value.
    ///
    /// Ordered rather than hashed so a configuration error names the same
    /// offending flag on every run. Set through [`Config::promote_flag`] or
    /// [`Config::promote_flag_as`].
    pub promoted_flags: BTreeMap<(String, String), String>,

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

    /// Freshness hint on brontes' cacheable results (`ttlMs`, SEP-2549) —
    /// `tools/list` and `server/discover`.
    ///
    /// `None` uses [`Config::DEFAULT_CACHE_TTL`].  Set [`Duration::ZERO`] to
    /// tell clients not to cache those results at all.
    pub cache_ttl: Option<Duration>,

    /// Who may cache brontes' cacheable results (`cacheScope`, SEP-2549).
    ///
    /// `None` uses [`Config::DEFAULT_CACHE_SCOPE`].
    pub cache_scope: Option<CacheScope>,

    /// Whether to lower a request's W3C Trace Context onto the spawned CLI's
    /// environment (SEP-414).
    ///
    /// `None` uses [`Config::DEFAULT_PROPAGATE_TRACE_CONTEXT`].  See
    /// [`Config::propagate_trace_context`].
    pub propagate_trace_context: Option<bool>,

    /// Whether `tools/call` blocks or hands back a task handle (SEP-2663).
    ///
    /// Defaults to [`TaskMode::Blocking`].  Override per-command via
    /// [`Config::task_mode_for`].
    pub task_mode: TaskMode,

    /// Per-command [`TaskMode`] overrides, keyed by full command path.
    ///
    /// Takes precedence over [`Config::task_mode`].  Paths that match no
    /// walked command are rejected by `generate_tools`.
    pub task_modes: HashMap<String, TaskMode>,

    /// How long a detached task may run, and how long its record survives
    /// afterwards (`ttlMs`, SEP-2663).
    ///
    /// `None` — the default — means unlimited.  See [`Config::task_ttl`] for
    /// what a finite value implies.
    pub task_ttl: Option<Duration>,

    /// The polling interval brontes suggests to clients for its tasks
    /// (`pollIntervalMs`, SEP-2663).
    ///
    /// `None` uses [`Config::DEFAULT_TASK_POLL_INTERVAL`].
    pub task_poll_interval: Option<Duration>,

    /// Named bundles of commands an end user can ask for by name, keyed by
    /// group name.
    ///
    /// Ordered so `mcp tools --groups` lists them the same way on every run.
    /// Set through [`Config::group`] / [`Config::group_description`].
    pub groups: BTreeMap<String, Group>,

    /// Which of the walked commands this server exposes.
    ///
    /// Empty by default (every command becomes a tool). The `--group` /
    /// `--command` / `--tool` flags on `mcp start`, `mcp stream`, and
    /// `mcp tools` merge onto whatever is set here, so a developer default and
    /// an end-user choice compose.
    pub tool_filter: ToolFilter,
}

impl Config {
    /// Default freshness hint for cacheable results: five minutes.
    ///
    /// A brontes tool list is walked once at server construction from an
    /// immutable clap tree, so it cannot change while the server runs — a
    /// non-zero TTL is always honest.  Five minutes is short enough that a
    /// consumer who rebuilds and restarts their CLI sees the new tool list
    /// promptly, and long enough to stop clients re-listing on every turn.
    pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300);

    /// Default cache scope for cacheable results: [`CacheScope::Public`].
    ///
    /// `Public` is accurate rather than optimistic: [`Config`] is frozen when
    /// the server is constructed and the walk depends on nothing per-client,
    /// so every client of a given brontes server receives a byte-identical
    /// tool list.  A shared intermediary therefore cannot serve one client's
    /// list to another client and be wrong.  Any future feature that varies
    /// the tool list per request breaks that invariant and must revisit this
    /// default; consumers who front brontes with a caching proxy across trust
    /// boundaries can narrow it via [`Config::cache_scope`].
    pub const DEFAULT_CACHE_SCOPE: CacheScope = CacheScope::Public;

    /// Resolve the effective freshness hint in milliseconds.
    ///
    /// Saturates at [`u64::MAX`] so an absurd [`Duration`] cannot wrap.
    #[must_use]
    pub fn resolved_cache_ttl_ms(&self) -> u64 {
        let ttl = self.cache_ttl.unwrap_or(Self::DEFAULT_CACHE_TTL);
        u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX)
    }

    /// Resolve the effective cache scope.
    #[must_use]
    pub fn resolved_cache_scope(&self) -> CacheScope {
        self.cache_scope.unwrap_or(Self::DEFAULT_CACHE_SCOPE)
    }

    /// Trace-context propagation is on by default.
    ///
    /// The propagated values only exist when the client actually sends them,
    /// so a client that carries no trace changes nothing about the spawned
    /// process.  On for a traced client is what makes the feature zero-config;
    /// off by default would mean every consumer discovers a broken trace and
    /// then discovers the flag.
    pub const DEFAULT_PROPAGATE_TRACE_CONTEXT: bool = true;

    /// Resolve whether trace context propagates into the spawned CLI.
    #[must_use]
    pub fn resolved_propagate_trace_context(&self) -> bool {
        self.propagate_trace_context
            .unwrap_or(Self::DEFAULT_PROPAGATE_TRACE_CONTEXT)
    }

    /// Default polling interval suggested for detached tasks: one second.
    ///
    /// Matches the SDK's own default.  A wrapped CLI command that is worth
    /// detaching runs for long enough that a second of poll granularity is
    /// noise; [`Config::task_poll_interval`] tightens it.
    pub const DEFAULT_TASK_POLL_INTERVAL: Duration = Duration::from_secs(1);

    /// Resolve the effective [`TaskMode`] for a command path.
    ///
    /// A per-command entry wins over the global default. The lookup is exact
    /// against the keys as written, so query it the way you wrote them;
    /// brontes resolves relative and absolute spellings onto each other when
    /// it builds the tool list, not here.
    #[must_use]
    pub fn resolved_task_mode(&self, cmd_path: &str) -> TaskMode {
        self.task_modes
            .get(cmd_path)
            .copied()
            .unwrap_or(self.task_mode)
    }

    /// Resolve the task TTL in milliseconds, or `None` for unlimited.
    ///
    /// Saturates at [`u64::MAX`] so an absurd [`Duration`] cannot wrap.
    #[must_use]
    pub fn resolved_task_ttl_ms(&self) -> Option<u64> {
        self.task_ttl
            .map(|ttl| u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX))
    }

    /// Resolve the suggested poll interval in milliseconds.
    ///
    /// Saturates at [`u64::MAX`] so an absurd [`Duration`] cannot wrap.
    #[must_use]
    pub fn resolved_task_poll_interval_ms(&self) -> u64 {
        let interval = self
            .task_poll_interval
            .unwrap_or(Self::DEFAULT_TASK_POLL_INTERVAL);
        u64::try_from(interval.as_millis()).unwrap_or(u64::MAX)
    }

    /// Whether any command on this server can produce a task.
    ///
    /// Drives the `tasks` capability: brontes advertises the extension only
    /// when some command is actually detached, so a client never negotiates a
    /// capability that would answer `tasks/get` with "no such task" forever.
    #[must_use]
    pub fn tasks_enabled(&self) -> bool {
        self.task_mode == TaskMode::Detached
            || self.task_modes.values().any(|m| *m == TaskMode::Detached)
    }

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
    /// `cmd_path` is the space-joined command path, with or without the
    /// CLI's own name, e.g.
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
    /// `cmd_path` is the space-joined command path (the CLI's own name is
    /// optional), e.g. `"oldcmd"` or `"my-cli oldcmd"`.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().deprecate("oldcmd");
    /// assert!(cfg.deprecated_commands.contains("oldcmd"));
    /// ```
    #[must_use]
    pub fn deprecate(mut self, cmd_path: impl Into<String>) -> Self {
        self.deprecated_commands.insert(cmd_path.into());
        self
    }

    /// Replace the auto-derived JSON Schema for a specific flag.
    ///
    /// `cmd_path` is the space-joined command path (the CLI's own name is
    /// optional) and `flag` is the
    /// long flag name (without the leading `--`).  The provided `schema` value
    /// is used as-is; auto default/required/enum extraction is skipped.
    ///
    /// One key does not belong here: SEP-2243's `x-mcp-header` is honored only
    /// on top-level properties of a tool's input schema, and a `flag_schemas`
    /// entry always lands nested under `flags`.  Use [`Config::promote_flag`],
    /// which hoists the flag to the top level where the annotation works; a
    /// schema carrying the key directly logs a warning from `generate_tools`.
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
    /// type is opaque to brontes.  `cmd_path` is the space-joined command
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

    /// Promote a flag to a SEP-2243 `Mcp-Param-*` HTTP header, deriving the
    /// header suffix from the flag name.
    ///
    /// The flag moves out of the tool's `flags` object and becomes a top-level
    /// property of the input schema — the only place the `x-mcp-header`
    /// annotation is honored — so a streamable-HTTP client mirrors its value
    /// into `Mcp-Param-<flag>` and intermediaries can route on it without
    /// parsing the body. brontes folds the value back into `flags` before
    /// running the command, so the CLI is invoked identically either way.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// // `--region` moves to the top level and rides in `Mcp-Param-region`.
    /// let cfg = Config::default().promote_flag("my-cli deploy", "region");
    /// assert_eq!(
    ///     cfg.promoted_flags.get(&("my-cli deploy".into(), "region".into())),
    ///     Some(&"region".to_string()),
    /// );
    /// ```
    ///
    /// # This changes the tool's wire shape
    ///
    /// The promoted flag is no longer accepted under `flags`, because a value
    /// present in two places is a value two clients can disagree about. Callers
    /// follow the advertised schema, so conforming clients adapt on their own —
    /// but anything hand-writing the old shape for this one flag must be
    /// updated.
    ///
    /// # Requirements
    ///
    /// Enforced by `generate_tools`, which fails rather than advertising an
    /// annotation the peer would reject:
    ///
    /// - the command path and flag must exist;
    /// - the derived header must be a valid RFC 9110 token (alphanumerics and
    ///   ``!#$%&'*+-.^_`|~``) — use [`Config::promote_flag_as`] when the flag
    ///   name is not one;
    /// - header names must be unique per command, case-insensitively;
    /// - the flag's schema type must be `string`, `integer`, or `boolean`;
    /// - the flag may not be named `flags` or `args`, which are already
    ///   top-level properties.
    #[must_use]
    pub fn promote_flag(self, cmd_path: impl Into<String>, flag: impl Into<String>) -> Self {
        let flag = flag.into();
        let header = flag.clone();
        self.promote_flag_as(cmd_path, flag, header)
    }

    /// Promote a flag to a SEP-2243 `Mcp-Param-*` header under an explicit
    /// header name.
    ///
    /// Identical to [`Config::promote_flag`] except that the header suffix is
    /// given rather than derived — for a flag name that is not a valid HTTP
    /// token, or when the header has to match a name an intermediary already
    /// routes on.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// // The flag is `--api.key`; the header must be `Mcp-Param-Api-Key`.
    /// let cfg = Config::default().promote_flag_as("my-cli deploy", "api.key", "Api-Key");
    /// assert_eq!(
    ///     cfg.promoted_flags.get(&("my-cli deploy".into(), "api.key".into())),
    ///     Some(&"Api-Key".to_string()),
    /// );
    /// ```
    #[must_use]
    pub fn promote_flag_as(
        mut self,
        cmd_path: impl Into<String>,
        flag: impl Into<String>,
        header: impl Into<String>,
    ) -> Self {
        self.promoted_flags
            .insert((cmd_path.into(), flag.into()), header.into());
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
    /// `cmd_path` is the space-joined command path, the CLI's own name
    /// optional (e.g.,
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
    /// `cmd_path` is the space-joined command path (the CLI's own name is
    /// optional); `text` is the literal
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

    /// Set the freshness hint on `tools/list` and `server/discover` results.
    ///
    /// Defaults to [`Config::DEFAULT_CACHE_TTL`].  [`Duration::ZERO`]
    /// tells clients the tool list must not be cached.
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().cache_ttl(Duration::from_secs(30));
    /// assert_eq!(cfg.resolved_cache_ttl_ms(), 30_000);
    /// ```
    #[must_use]
    pub const fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Set who may cache `tools/list` and `server/discover` results.
    ///
    /// Defaults to [`Config::DEFAULT_CACHE_SCOPE`].  Narrow this to
    /// [`CacheScope::Private`] when a shared caching intermediary sits between
    /// brontes and clients in different trust domains.
    ///
    /// ```rust
    /// use brontes::{CacheScope, Config};
    ///
    /// let cfg = Config::default().cache_scope(CacheScope::Private);
    /// assert_eq!(cfg.resolved_cache_scope(), CacheScope::Private);
    /// ```
    #[must_use]
    pub const fn cache_scope(mut self, scope: CacheScope) -> Self {
        self.cache_scope = Some(scope);
        self
    }

    /// Turn W3C Trace Context propagation on or off.
    ///
    /// When on (the default, [`Config::DEFAULT_PROPAGATE_TRACE_CONTEXT`]), a
    /// request's `_meta` `traceparent` / `tracestate` / `baggage` (SEP-414) are
    /// validated and lowered onto the spawned CLI's `TRACEPARENT` /
    /// `TRACESTATE` / `BAGGAGE` environment variables, so an instrumented CLI
    /// joins the calling agent's trace.  The values are also readable from
    /// [`crate::MiddlewareCtx::trace_context`] whether or not propagation is on.
    ///
    /// Turn it off when the wrapped CLI must not observe the caller's trace —
    /// for example when its own tracing setup would misread those variables.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().propagate_trace_context(false);
    /// assert!(!cfg.resolved_propagate_trace_context());
    /// ```
    #[must_use]
    pub const fn propagate_trace_context(mut self, propagate: bool) -> Self {
        self.propagate_trace_context = Some(propagate);
        self
    }

    /// Hand `tools/call` back as a task handle for every command that a
    /// tasks-capable client invokes (SEP-2663).
    ///
    /// Reach for [`Config::task_mode_for`] first: a task costs an extra poll
    /// round trip, which buys nothing on a command that returns immediately.
    ///
    /// ```rust
    /// use brontes::{Config, TaskMode};
    ///
    /// let cfg = Config::default().task_mode(TaskMode::Detached);
    /// assert_eq!(cfg.resolved_task_mode("cli build"), TaskMode::Detached);
    /// ```
    #[must_use]
    pub const fn task_mode(mut self, mode: TaskMode) -> Self {
        self.task_mode = mode;
        self
    }

    /// Override [`TaskMode`] for a specific command path.
    ///
    /// The path is the space-separated command path (the CLI's own name is
    /// optional), not the MCP tool name.  A path matching no walked command is
    /// an [`crate::Error::Config`] from `generate_tools` rather than a
    /// silently ignored entry.
    ///
    /// ```rust
    /// use brontes::{Config, TaskMode};
    ///
    /// let cfg = Config::default()
    ///     .task_mode_for("release", TaskMode::Detached)
    ///     .task_mode_for("publish", TaskMode::Detached);
    ///
    /// assert_eq!(cfg.resolved_task_mode("release"), TaskMode::Detached);
    /// assert_eq!(cfg.resolved_task_mode("version"), TaskMode::Blocking);
    /// ```
    #[must_use]
    pub fn task_mode_for(mut self, cmd_path: impl Into<String>, mode: TaskMode) -> Self {
        self.task_modes.insert(cmd_path.into(), mode);
        self
    }

    /// Bound how long a detached task may run and how long its record lives.
    ///
    /// Unset — the default — means unlimited, which preserves brontes'
    /// execution contract: a wrapped command runs until it exits or the client
    /// cancels it, never until a clock brontes invented runs out.
    ///
    /// A finite TTL is therefore two things at once, and the first is easy to
    /// miss: a command still running when the TTL elapses is **aborted** and
    /// its task settles as `failed`.  Set it to a value above the slowest
    /// command the server exposes, or leave it unset.
    ///
    /// The tradeoff for leaving it unset is retention: finished task records
    /// are held for the lifetime of the server process.  That is bounded and
    /// harmless for `mcp start`, where the process belongs to one client and
    /// exits with it; a long-lived `mcp stream` server serving many clients
    /// should set a TTL so completed tasks are eventually swept.
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().task_ttl(Duration::from_secs(3600));
    /// assert_eq!(cfg.resolved_task_ttl_ms(), Some(3_600_000));
    /// ```
    #[must_use]
    pub const fn task_ttl(mut self, ttl: Duration) -> Self {
        self.task_ttl = Some(ttl);
        self
    }

    /// Suggest how often clients should poll `tasks/get` for a detached
    /// command.
    ///
    /// `None` uses [`Config::DEFAULT_TASK_POLL_INTERVAL`].  Shorten it for a
    /// CLI whose commands finish in a second or two, so the poll interval does
    /// not dominate the command's own runtime.
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().task_poll_interval(Duration::from_millis(250));
    /// assert_eq!(cfg.resolved_task_poll_interval_ms(), 250);
    /// ```
    #[must_use]
    pub const fn task_poll_interval(mut self, interval: Duration) -> Self {
        self.task_poll_interval = Some(interval);
        self
    }

    /// Name a bundle of commands an end user can select with
    /// `--group <NAME>`.
    ///
    /// Each entry covers its own subtree, so naming `"release"` also takes
    /// `"release notes"` — the group tracks the command tree
    /// rather than drifting from it as subcommands are added.  Calling this
    /// twice with the same name appends to the existing group.
    ///
    /// Paths that match no walked command are rejected by
    /// [`crate::generate_tools`], so a renamed subcommand breaks the build
    /// instead of silently shrinking the group.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().group("release", ["release", "publish"]);
    /// assert_eq!(cfg.groups["release"].commands.len(), 2);
    /// ```
    #[must_use]
    pub fn group<I, S>(mut self, name: impl Into<String>, commands: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.groups
            .entry(name.into())
            .or_default()
            .commands
            .extend(commands.into_iter().map(Into::into));
        self
    }

    /// Attach a one-line summary to a group, shown by `mcp tools --groups`.
    ///
    /// Naming a group that has no members yet creates it, so the description
    /// may be set before or after [`Config::group`].
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default()
    ///     .group("release", ["release"])
    ///     .group_description("release", "Cut, sign, and publish a release");
    /// assert!(cfg.groups["release"].description.is_some());
    /// ```
    #[must_use]
    pub fn group_description(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.groups.entry(name.into()).or_default().description = Some(description.into());
        self
    }

    /// Expose only the named group, as `--group <NAME>` does.
    ///
    /// Combines with the other `expose_*` methods (union) and loses to every
    /// `hide_*` method.  Pin one here to ship a CLI whose MCP server is
    /// trimmed by default; end users widen or narrow it from the launch flags.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default()
    ///     .group("release", ["release"])
    ///     .expose_group("release");
    /// assert!(cfg.tool_filter.groups.contains("release"));
    /// ```
    #[must_use]
    pub fn expose_group(mut self, name: impl Into<String>) -> Self {
        self.tool_filter.groups.insert(name.into());
        self
    }

    /// Discard every exposure pinned so far and serve the whole tool list, as
    /// `--all` does.
    ///
    /// Every other `expose_*` method is additive, so a CLI that ships a
    /// trimmed default would otherwise be trimmed forever — an end user could
    /// widen it one group at a time but never get back to the full list. This
    /// is the way out. Hiding is untouched: a command the CLI's author removed
    /// stays removed.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().expose_group("release").expose_all();
    /// assert!(!cfg.tool_filter.selects());
    /// ```
    #[must_use]
    pub const fn expose_all(mut self) -> Self {
        self.tool_filter.expose_all = true;
        self
    }

    /// Expose a command and everything under it, as `--command <PATH>` does.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().expose_command("release");
    /// assert!(cfg.tool_filter.commands.contains("release"));
    /// ```
    #[must_use]
    pub fn expose_command(mut self, cmd_path: impl Into<String>) -> Self {
        self.tool_filter.commands.insert(cmd_path.into());
        self
    }

    /// Expose one MCP tool by its generated name, as `--tool <NAME>` does.
    ///
    /// Unlike [`Config::expose_command`] this does not take the subtree — it
    /// is the surgical form, for when a command's children should stay hidden.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().expose_tool("anodizer_release");
    /// assert!(cfg.tool_filter.tools.contains("anodizer_release"));
    /// ```
    #[must_use]
    pub fn expose_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_filter.tools.insert(tool_name.into());
        self
    }

    /// Remove a group from the tool list, as `--hide-group <NAME>` does.
    ///
    /// Hiding beats exposing, so this holds even against an `expose_*` entry
    /// naming the same command.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default()
    ///     .group("dangerous", ["secrets"])
    ///     .hide_group("dangerous");
    /// assert!(cfg.tool_filter.hidden_groups.contains("dangerous"));
    /// ```
    #[must_use]
    pub fn hide_group(mut self, name: impl Into<String>) -> Self {
        self.tool_filter.hidden_groups.insert(name.into());
        self
    }

    /// Remove a command and everything under it, as `--hide-command <PATH>`
    /// does.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().hide_command("secrets");
    /// assert!(cfg.tool_filter.hidden_commands.contains("secrets"));
    /// ```
    #[must_use]
    pub fn hide_command(mut self, cmd_path: impl Into<String>) -> Self {
        self.tool_filter.hidden_commands.insert(cmd_path.into());
        self
    }

    /// Remove one MCP tool by its generated name, as `--hide-tool <NAME>`
    /// does.
    ///
    /// ```rust
    /// use brontes::Config;
    ///
    /// let cfg = Config::default().hide_tool("anodizer_secrets_get");
    /// assert!(cfg.tool_filter.hidden_tools.contains("anodizer_secrets_get"));
    /// ```
    #[must_use]
    pub fn hide_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_filter.hidden_tools.insert(tool_name.into());
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
        assert!(cfg.cache_ttl.is_none());
        assert!(cfg.cache_scope.is_none());
    }

    #[test]
    fn unset_cache_hints_resolve_to_the_documented_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.resolved_cache_ttl_ms(), 300_000);
        assert_eq!(cfg.resolved_cache_scope(), CacheScope::Public);
    }

    #[test]
    fn cache_hint_overrides_win_over_defaults() {
        let cfg = Config::default()
            .cache_ttl(Duration::from_millis(1_500))
            .cache_scope(CacheScope::Private);
        assert_eq!(cfg.resolved_cache_ttl_ms(), 1_500);
        assert_eq!(cfg.resolved_cache_scope(), CacheScope::Private);
    }

    #[test]
    fn zero_ttl_is_preserved_rather_than_treated_as_unset() {
        // `Duration::ZERO` is a meaningful hint ("do not cache"), so it must
        // not collapse back to the five-minute default.
        let cfg = Config::default().cache_ttl(Duration::ZERO);
        assert_eq!(cfg.resolved_cache_ttl_ms(), 0);
    }

    #[test]
    fn absurd_ttl_saturates_instead_of_wrapping() {
        let cfg = Config::default().cache_ttl(Duration::MAX);
        assert_eq!(cfg.resolved_cache_ttl_ms(), u64::MAX);
    }

    #[test]
    fn selector_pushes_in_order() {
        // Pins selector ordering (first-match-wins is downstream of insertion
        // order). NOT a tautology: the public `selectors` slice ordering is
        // a load-bearing surface — the walker iterates in this order and
        // the first matching selector wins, so a refactor that swaps Vec
        // for a HashSet would silently break first-match semantics.
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
    fn fluent_chain_covers_every_setter() {
        // Single comprehensive chain that hits every builder method on
        // `Config`. Replaces ten individual `setter_sets_field` tests that
        // were tautological identity assertions ("`.x(v)` makes `cfg.x ==
        // v`" — true by construction).
        //
        // The two real invariants this test pins:
        //   (a) Every setter actually stores its argument into the right
        //       field (a rename / wrong-field bug surfaces as a missing
        //       assertion below).
        //   (b) The builder is fluent — every method returns `self` — so
        //       a single chained expression composes without intermediate
        //       binding ceremony. A future refactor that accidentally
        //       returns a non-self value will fail to compile this test.
        let imp = rmcp::model::Implementation::new("test-server", "0.1.0");
        let cfg = Config::default()
            .command_name("agent")
            .tool_name_prefix("myapp")
            .selector(Selector::default())
            .default_env("LOG_FORMAT", "json")
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
            .log_level(Level::DEBUG)
            .implementation(imp);

        assert_eq!(cfg.command_name.as_deref(), Some("agent"));
        assert_eq!(cfg.tool_name_prefix.as_deref(), Some("myapp"));
        assert_eq!(cfg.selectors.len(), 1);
        assert_eq!(
            cfg.default_env.get("LOG_FORMAT").map(String::as_str),
            Some("json")
        );
        assert!(cfg.annotations.contains_key("my-cli list"));
        assert_eq!(cfg.annotations["my-cli list"].read_only_hint, Some(true));
        assert!(cfg.deprecated_commands.contains("my-cli oldcmd"));
        let schema_key = ("my-cli list".to_string(), "limit".to_string());
        assert_eq!(
            cfg.flag_schemas[&schema_key],
            serde_json::json!({"type": "integer", "minimum": 0})
        );
        let type_key = ("my-cli list".to_string(), "filter".to_string());
        assert_eq!(cfg.flag_type_overrides[&type_key], SchemaType::Array);
        assert_eq!(cfg.log_level, Some(Level::DEBUG));
        let stored_imp = cfg.implementation.expect("implementation stored");
        assert_eq!(stored_imp.name, "test-server");
        assert_eq!(stored_imp.version, "0.1.0");
    }
}
