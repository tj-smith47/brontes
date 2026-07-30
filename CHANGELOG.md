# Changelog

All notable changes to this project are documented here. Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning follows [SemVer](https://semver.org/).

## [Unreleased]

### Added

- MCP protocol revision `2026-07-28` support. The revision is stateless — no `initialize`/`notifications/initialized` handshake and no `Mcp-Session-Id` — and brontes serves it end to end: `server/discover` (SEP-2575) reports the supported version set, the `tools`-only capability set, and the host CLI's identity; every result carries the `resultType` discriminator (SEP-2322); and `tools/call` over streamable HTTP validates SEP-2243 standard headers (`Mcp-Method` / `Mcp-Name`). Protocol revisions back to `2024-11-05` continue to negotiate, handshake and session id included.
- SEP-2549 cache hints (`ttlMs` / `cacheScope`) on `tools/list` and `server/discover` results, with `Config::cache_ttl(Duration)` and `Config::cache_scope(CacheScope)` overrides. Defaults are five minutes and `public`: a brontes tool list is walked once at server construction from an immutable `clap` tree and depends on nothing per-client, so it is genuinely shareable. `Duration::ZERO` advertises "do not cache".
- `brontes::CacheScope` re-exports `rmcp::model::CacheScope`, so setting `Config::cache_scope` needs no direct `rmcp` dependency and cannot mismatch the `rmcp` version brontes links.
- SEP-2243 flag promotion via `Config::promote_flag(cmd_path, flag)` and `Config::promote_flag_as(cmd_path, flag, header)`. A promoted flag moves out of the tool's `flags` object to a top-level input-schema property carrying `x-mcp-header` — the only placement where the annotation is honored — so a streamable-HTTP client mirrors its value into `Mcp-Param-*` and intermediaries route on it without parsing the body. brontes folds the value back into `flags` before running the command, so the CLI is invoked identically. A promotion that could not work on the wire fails at `generate_tools` time rather than being advertised: non-primitive schema types, header names that are not RFC 9110 tokens, and two flags on one command colliding case-insensitively on the same header.
- MRTR support (SEP-2322) at the middleware boundary. A middleware returns `MiddlewareOutcome::InputRequired` to ask the client for input — under the stateless revision the only channel a server has for reaching its client — and the retry arrives with `MiddlewareCtx::input_responses` and `MiddlewareCtx::request_state` populated. brontes refuses to emit an input request a peer cannot answer (protocol older than `2026-07-28`, or elicitation requested from a client that never declared the capability), degrading to a tool error rather than a protocol-level failure.
- W3C Trace Context propagation (SEP-414). A validated `traceparent` / `tracestate` / `baggage` from the request's `_meta` reaches the spawned CLI as `TRACEPARENT` / `TRACESTATE` / `BAGGAGE`, so a CLI's own spans join the caller's trace. Malformed values are dropped rather than forwarded, and `Config::propagate_trace_context(false)` opts out without disturbing `Config::default_env`. Middleware reads the parsed values through `MiddlewareCtx::trace_context` regardless of the setting.
- `MiddlewareCtx` now also carries the request's raw `_meta` (including the progress token), the negotiated `protocol_version`, and the client's declared `client_capabilities`.
- The `rmcp` types appearing on brontes' own surface are re-exported (`ClientCapabilities`, `ProtocolVersion`, `RequestMetaObject`, the MRTR and elicitation types), so consuming the middleware boundary needs no direct `rmcp` dependency and cannot mismatch the version brontes links. `RequestStateCodec` / `SealOptions` follow behind the `request-state` feature, for middleware that must verify an echoed `requestState`.

### Fixed

- `clap::ArgAction::Count` flags rendered as `--flag N`, a form clap rejects outright because a `Count` arg parses with `num_args(0)` — the flag was unusable through MCP. It now renders as repetition (`--flag --flag`). The render kind is derived from the arg's action rather than from the advertised JSON Schema type, so a `Config::flag_type_override` can no longer change how a flag is executed.
- Tool errors did not satisfy the `outputSchema` every tool advertises. A failed call now returns a conforming `ToolOutput` as `structuredContent`, with the brontes-specific detail moved to namespaced `_meta` keys.
- `flags.additionalProperties: false` was advertised but unenforced — nothing in the MCP layer validates call arguments against the input schema, so an unknown flag reached the CLI as an opaque usage error. Unknown flag names are now rejected before the command runs, reported together and sorted.
- Published tool schemas carried brontes' own rustdoc as `title` / `description` on the `ToolInput` and `ToolOutput` wrappers, spending the model's context on prose about brontes' Rust types. Stripping it cut the generated tool-list fixture by more than half.

- `brontes::DescriptionMode` (`Short` / `Long`, default `Long`) plus `Config::description_mode`, `Config::description_mode_for(path, mode)`, and `Config::description(path, text)` for controlling per-tool description text. The literal `description` override bypasses the `long_about`/`about`/`after_help` cascade entirely. Default behavior is unchanged from 0.1.0; consumers opt in surgically per command or globally when verbose `long_about` text wastes the LLM's context budget. Closes the per-command description override gap (the ophis-equivalent of [njayp/ophis#6](https://github.com/njayp/ophis/issues/6), which only partially shipped as PR #7's always-append `cmd.Example`).

### Changed

- `rmcp` 2.2 → 3.0, the SDK revision implementing MCP `2026-07-28`. Consumers who name `rmcp` types directly — `Config::implementation` takes an `rmcp::model::Implementation` — must move to `rmcp` 3.x in lockstep.
- **Breaking:** `MiddlewareResult` is now `Result<MiddlewareOutcome>` rather than `Result<ToolOutput>`, so a middleware can answer with an MRTR input request instead of a finished process. `MiddlewareOutcome: From<ToolOutput>` makes the migration a trailing `.into()` on each success path; the change is a compile error at every affected site, never a silent behavior shift.

## [0.1.0] - 2026-05-13

Initial release. brontes transforms `clap` CLIs into [MCP](https://modelcontextprotocol.io) servers, inspired by [njayp/ophis](https://github.com/njayp/ophis).

### Added

#### Library surface

- `brontes::generate_tools(root, cfg) -> Result<Vec<rmcp::model::Tool>>` turns a `clap::Command` tree plus a `brontes::Config` into the `Vec<rmcp::model::Tool>` an MCP server advertises from `list_tools`. Library entry point for callers wiring their own server.
- `brontes::Config` is a fluent builder for tool-name prefix, selector filtering, per-tool annotations, deprecated-command set, per-flag schema and type overrides, default environment, log level, and rmcp `Implementation` identity metadata. Marked `#[non_exhaustive]`; future fields land additively.
- `brontes::Selector` plus the `brontes::selectors` factory functions (`allow_cmds`, `exclude_cmds`, `allow_cmds_containing`, `exclude_cmds_containing`, `allow_flags`, `exclude_flags`, `no_flags`) and their underlying `CmdMatcher` / `FlagMatcher` types filter commands and flags out of the tool surface.
- `brontes::ToolAnnotations` carries MCP read-only / destructive / idempotent / open-world hints keyed by full command path. Annotation paths that miss every walked command return a clear `Error::Config` from `generate_tools`.
- `brontes::ToolInput` and `brontes::ToolOutput` model the wire shapes brontes uses at the MCP tool-call boundary.
- `brontes::SchemaType` exposes the coarse type classification consumed by `Config::flag_type_override`.
- `brontes::MiddlewareCtx`, `Middleware`, `BoxedNext`, and `MiddlewareResult` form the async middleware boundary that wraps tool execution. `MiddlewareCtx` is `#[non_exhaustive]`; downstream middleware receives a value rather than constructing one.
- `brontes::Error` (non-exhaustive) and `brontes::Result` provide the library error surface. Pair with `Result<(), brontes::Error>` in `main` rather than relying on a `Termination` impl.

#### CLI mounting

- `brontes::command(cfg)` builds the `mcp` subcommand subtree (`mcp start`, `mcp stream`, `mcp tools`, plus the editor groups) ready to mount on a parent `clap::Command`. Validates the configured group name and surfaces a sibling-collision error from `handle` when the user's CLI already carries a same-named subcommand.
- `brontes::handle(matches, cli, cfg)` dispatches an `mcp` subcommand match. Async; routes `start`, `tools`, `stream`, and every editor leaf to the right runtime.
- `brontes::run(cli, cfg)` is one-call sugar that mounts the subtree, parses argv, and dispatches. Targets tiny CLIs whose only purpose is the MCP server.

#### MCP server runtimes

- `mcp start` runs the stdio MCP server over `rmcp::transport::stdio` with a stderr-logging tracing subscriber, a `--log-level` flag, and signal-driven graceful shutdown.
- `mcp stream --host <HOST> --port <PORT>` runs the streamable-HTTP MCP server over `rmcp::transport::streamable_http_server::StreamableHttpService` (rmcp 1.6) behind a hyper per-connection accept loop. Empty `--host` binds `0.0.0.0`; the startup log line matches ophis verbatim. Signal-driven cancellation (SIGINT/SIGTERM on Unix, Ctrl+C on Windows) with a 5-second graceful-drain window. `--allow-host <HOST>` (repeatable) appends to rmcp's DNS-rebind allow-list so LAN/public hosts reach the server.
- `mcp tools` exports the generated tool list to `./mcp-tools.json` as pretty-printed JSON for offline inspection.

#### Editor managers

- `mcp claude {enable, disable, list}` manages Claude Desktop's `claude_desktop_config.json` with `--config-path`, `--server-name`, `--env` (repeatable `-e KEY=VAL`), and `--log-level`. Resolves per-OS paths (macOS `~/Library/Application Support/Claude/...`, Linux `$XDG_CONFIG_HOME` or `~/.config/Claude/...`, Windows `%APPDATA%\Claude\...`). Backup-before-write semantics: the existing file is copied to `<base>.backup.json` before any save.
- `mcp cursor {enable, disable, list}` manages Cursor's `mcp.json` with the same flags as `claude` plus `--workspace` on all three leaves. Without `--workspace` the target is per-OS user-mode (`~/.cursor/mcp.json`); with `--workspace` the target is `$CWD/.cursor/mcp.json`. The on-disk shape carries the VSCode-compatible `type`/`command`/`args`/`env`/`url`/`headers` server fields plus an optional `inputs[]` array preserved on round-trip.
- `mcp vscode {enable, disable, list}` manages VSCode's MCP server registration with the same flag set and `--workspace` selector. User-mode resolves to the per-OS VSCode user-settings location; workspace-mode resolves to `$CWD/.vscode/mcp.json`.

#### Example crate

- `examples/make-mcp` ships a complete consumer that wraps GNU `make` as a single-leaf CLI (`build` with `--directory`, `--target`, `--jobs`, `--dry-run`). Exercises required-flag schema generation end-to-end and serves as the canonical "what does a brontes consumer look like" reference.

### Notes

- MSRV is 1.94.

[Unreleased]: https://github.com/tj-smith47/brontes/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/tj-smith47/brontes/releases/tag/v0.1.0
