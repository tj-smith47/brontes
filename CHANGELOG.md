# Changelog

All notable changes to this project are documented here. Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning follows [SemVer](https://semver.org/).

## [Unreleased]

## [0.7.1] - 2026-08-01

### Fixed

- A tool selection pinned in `Config` was a floor no launch line could get under. Every exposing entry unions, so `--group modules` on a CLI pinning `core` served `modules` *padded* with `core`, and `--hide-group core` — the obvious way to say "just modules" — was read as the pinned group failing to arrive and rejected. A launch-line hide now overrules a pinned exposure, so narrowing has a spelling. Only the side that yields changed: a hide the CLI's author pinned still cannot be undone from the launch line, and one author naming the same entry on both sides is still the contradiction it was. Narrowing has to name what it narrows *to* — hiding the whole pin and selecting nothing is refused rather than silently emptying the exposing sets, which would mean "subtract from the whole tree" and hand back every tool outside the pin, widening a server through a flag whose only job is removal. That refusal names both ways forward instead of reporting a selection the user never made.
- The launch line and the pinned config were compared before either was resolved against the root, so whether `--hide-command secrets` overruled a pinned `"demo secrets"` depended on which spelling each side happened to use. Both are normalized before they meet.

## [0.7.0] - 2026-08-01

### Added

- `--all` (`Config::expose_all`), the escape hatch from a tool selection the CLI pins in its own `Config`. Every other selection entry is additive — `ToolFilter` merges the launch line into the config by union — so a CLI shipping a trimmed default could only ever be widened one group at a time. `--all` discards the exposing side wholesale and starts from the full list. It discards only that side: a `hide_*` the author set stays set, and `mcp <editor> enable --all` writes the flag into the argv it registers, so the installed server serves what the install command validated.
- `brontes::Tool` re-exports `rmcp::model::Tool`, joining the other surface re-exports. `generate_tools` hands back a `Vec<Tool>`, and naming its element type should not cost a direct `rmcp` dependency.

### Fixed

- `Config::task_mode_for` was inert whenever its path was written without the root segment, which 0.6.0 had just made the normal way to write one. The tool walk resolves its own copy of the config, but the server kept the caller's, so a relative key was never found under the absolute path a `tools/call` carries: the command ran blocking while `tasks` stayed advertised — the capability is decided by the config's values, which were right — leaving a server that promised a handle it never handed out. The server now normalizes at construction, so every per-command setting is keyed the way requests address it.

## [0.6.0] - 2026-07-31

### Changed

- Relative command paths now work in every path-keyed builder on `Config` — `annotation`, `deprecate`, `description`, `description_mode_for`, `task_mode_for`, `promote_flag`, `promote_flag_as`, `flag_schema`, and `flag_type_override` — and not only in the selection surfaces 0.5.0 covered. `Selector` command matchers are the one surface that still sees the resolved path, because a closure has nothing to resolve.

### Removed

- The `derive` feature on the `clap` dependency, which brontes never used — it builds command trees through the builder API. Consumers who use `#[derive(Parser)]` enable it themselves and are unaffected; everyone else stops carrying `clap_derive` and its proc-macro chain.

## [0.5.0] - 2026-07-31

### Changed

- Command selection paths no longer require the CLI's own name. `--command release` and `--command "anodizer release"` name the same command, and the same holds for `--hide-command` and for group members in `Config::group`. The root is the one segment brontes can always derive — it is the binary being invoked — so requiring it was asking users to retype what they had just typed. Absolute paths keep working; a path whose first segment is the root is read as absolute, so a CLI with a subcommand named after itself addresses it by spelling the root twice.

## [0.4.0] - 2026-07-31

### Added

- MCP protocol revision `2026-07-28` support. The revision is stateless — no `initialize`/`notifications/initialized` handshake and no `Mcp-Session-Id` — and brontes serves it end to end: `server/discover` (SEP-2575) reports the supported version set, the `tools`-only capability set, and the host CLI's identity; every result carries the `resultType` discriminator (SEP-2322); and `tools/call` over streamable HTTP validates SEP-2243 standard headers (`Mcp-Method` / `Mcp-Name`). Protocol revisions back to `2024-11-05` continue to negotiate, handshake and session id included.
- SEP-2549 cache hints (`ttlMs` / `cacheScope`) on `tools/list` and `server/discover` results, with `Config::cache_ttl(Duration)` and `Config::cache_scope(CacheScope)` overrides. Defaults are five minutes and `public`: a brontes tool list is walked once at server construction from an immutable `clap` tree and depends on nothing per-client, so it is genuinely shareable. `Duration::ZERO` advertises "do not cache".
- `brontes::CacheScope` re-exports `rmcp::model::CacheScope`, so setting `Config::cache_scope` needs no direct `rmcp` dependency and cannot mismatch the `rmcp` version brontes links.
- SEP-2243 flag promotion via `Config::promote_flag(cmd_path, flag)` and `Config::promote_flag_as(cmd_path, flag, header)`. A promoted flag moves out of the tool's `flags` object to a top-level input-schema property carrying `x-mcp-header` — the only placement where the annotation is honored — so a streamable-HTTP client mirrors its value into `Mcp-Param-*` and intermediaries route on it without parsing the body. brontes folds the value back into `flags` before running the command, so the CLI is invoked identically. A promotion that could not work on the wire fails at `generate_tools` time rather than being advertised: non-primitive schema types, header names that are not RFC 9110 tokens, and two flags on one command colliding case-insensitively on the same header.
- The Tasks extension (SEP-2663, `io.modelcontextprotocol/tasks`). `Config::task_mode_for(path, TaskMode::Detached)` answers `tools/call` with a task handle instead of blocking for the length of the command, which is the difference between a wrapped `version` and a wrapped `release`: the client polls `tasks/get` for status and the final result, answers a middleware's input request with `tasks/update`, and stops the process with `tasks/cancel`. The middleware boundary is unchanged — an input request raised inside a task leaves through `tasks/get` and the chain re-enters with `input_responses` populated exactly as on a blocking retry. Detaching is never a compatibility break: a client that did not declare the extension gets the blocking result whatever the config says, and brontes advertises the `tasks` capability only when some command is actually detached. `Config::task_ttl` bounds runtime and retention (unset, the default, means no time limit), and `Config::task_poll_interval` paces clients.
- MRTR support (SEP-2322) at the middleware boundary. A middleware returns `MiddlewareOutcome::InputRequired` to ask the client for input — under the stateless revision the only channel a server has for reaching its client — and the retry arrives with `MiddlewareCtx::input_responses` and `MiddlewareCtx::request_state` populated. brontes refuses to emit an input request a peer cannot answer (protocol older than `2026-07-28`, or elicitation requested from a client that never declared the capability), degrading to a tool error rather than a protocol-level failure.
- W3C Trace Context propagation (SEP-414). A validated `traceparent` / `tracestate` / `baggage` from the request's `_meta` reaches the spawned CLI as `TRACEPARENT` / `TRACESTATE` / `BAGGAGE`, so a CLI's own spans join the caller's trace. Malformed values are dropped rather than forwarded, and `Config::propagate_trace_context(false)` opts out without disturbing `Config::default_env`. Middleware reads the parsed values through `MiddlewareCtx::trace_context` regardless of the setting.
- Command groups and launch-time tool selection, for CLIs whose command tree is larger than any one job needs. A CLI's author names bundles with `Config::group(name, paths)` and `Config::group_description(name, text)`; end users then start a server carrying only part of the tool list, with `--group` / `--command` / `--tool` and the matching `--hide-group` / `--hide-command` / `--hide-tool` on `mcp start`, `mcp stream`, and `mcp tools`. `mcp tools --groups` lists what a CLI defines. The same flags on `mcp <editor> enable` are written into the `mcp start` argv the editor registers, so a selection survives the install command — and are resolved against the CLI before the config file is written, since an editor spawns its servers where nobody reads stderr and a typo would otherwise surface as an editor showing no tools. Group members and `--command` paths cover their whole subtree and match on path segments; hiding beats selecting, and a `hide_*` pinned in `Config` cannot be undone from the launch line. Every selection has to land: a name the CLI does not have, one that resolves to a real command the server would not serve anyway (deprecated, hidden, or dropped by a `Selector`), and a filter that leaves nothing at all each fail at startup naming the entry at fault, rather than serving a tool list nobody asked for. `Config::expose_group` / `expose_command` / `expose_tool` / `hide_group` / `hide_command` / `hide_tool` pin a selection programmatically, and `brontes::Group` / `brontes::ToolFilter` are the shapes behind both surfaces.
- `MiddlewareCtx` now also carries the request's raw `_meta` (including the progress token), the negotiated `protocol_version`, and the client's declared `client_capabilities`.
- The `rmcp` types appearing on brontes' own surface are re-exported (`ClientCapabilities`, `ProtocolVersion`, `RequestMetaObject`, the MRTR and elicitation types), so consuming the middleware boundary needs no direct `rmcp` dependency and cannot mismatch the version brontes links. `RequestStateCodec` / `SealOptions` follow behind the `request-state` feature, for middleware that must verify an echoed `requestState`.

### Fixed

- `clap::ArgAction::Count` flags rendered as `--flag N`, a form clap rejects outright because a `Count` arg parses with `num_args(0)` — the flag was unusable through MCP. It now renders as repetition (`--flag --flag`). The render kind is derived from the arg's action rather than from the advertised JSON Schema type, so a `Config::flag_type_override` can no longer change how a flag is executed.
- Tool errors did not satisfy the `outputSchema` every tool advertises. A failed call now returns a conforming `ToolOutput` as `structuredContent`, with the brontes-specific detail moved to namespaced `_meta` keys.
- Under `mcp stream`, every inbound request rebuilt the server handler and re-walked the `clap` tree. The walk is now a startup cost, paid once and shared across connections.
- Every `tools/call` had to spell out both `flags` and `args`, even for a command that takes neither: the tool input schema listed them as required and omitting `args` failed with `missing field args`. Both are now optional and default to empty, so the minimal call is `{}`. Callers that still send them are unaffected.
- `flags.additionalProperties: false` was advertised but unenforced — nothing in the MCP layer validates call arguments against the input schema, so an unknown flag reached the CLI as an opaque usage error. Unknown flag names are now rejected before the command runs, reported together and sorted.
- Two commands could generate the same MCP tool name — a flat `by_cell` and a nested `by cell` both become `<cli>_by_cell`, since path separators are underscores. The list advertised the name twice and every call dispatched to whichever was walked first, leaving the other command reachable only by renaming it. The collision is now a startup error naming both command paths.
- A command whose name contained a space produced a tool name nothing could address: the path is what `Config` keys, `--command`, and group members are written against, and a name with a space in it silently absorbed its neighbours. It is now rejected at startup.
- `mcp stream` logged `MCP server listening on address …` before validating the configuration, so a config or schema error printed a running server and then exited non-zero. The `clap` walk now happens before the bind, and the log line after it.
- Published tool schemas carried brontes' own rustdoc as `title` / `description` on the `ToolInput` and `ToolOutput` wrappers, spending the model's context on prose about brontes' Rust types. Stripping it cut the generated tool-list fixture by more than half.

### Changed

- `rmcp` 2.2 → 3.0, the SDK revision implementing MCP `2026-07-28`. Consumers who name `rmcp` types directly — `Config::implementation` takes an `rmcp::model::Implementation` — must move to `rmcp` 3.x in lockstep.
- **Breaking:** `MiddlewareResult` is now `Result<MiddlewareOutcome>` rather than `Result<ToolOutput>`, so a middleware can answer with an MRTR input request instead of a finished process. `MiddlewareOutcome: From<ToolOutput>` makes the migration a trailing `.into()` on each success path; the change is a compile error at every affected site, never a silent behavior shift.
- Tools are listed in `clap` declaration order rather than reverse. The order is a tool list's only ranking signal, so it should read the way the CLI's own `--help` does. It remains stable across calls and processes, which is what the revision asks for so clients can cache a listing.

## [0.3.0] - 2026-07-19

### Changed

- `rmcp` 1.6 → 2.2. Consumers naming `rmcp` types directly — `Config::implementation` takes an `rmcp::model::Implementation` — must move in lockstep.

## [0.2.2] - 2026-07-13

### Changed

- Release pipeline publishes to crates.io through OIDC Trusted Publishing rather than a stored token.

## [0.2.1] - 2026-06-26

### Fixed

- `quinn-proto` bumped to 0.11.15 for RUSTSEC-2026-0185.

### Changed

- Release pipeline moved to anodizer's single-crate mode, with attestations and library-appropriate publishers; the tag now writes `Cargo.toml`'s version rather than requiring a manual sync commit.

## [0.2.0] - 2026-05-14

### Added

- Zed editor manager (`mcp zed {enable,disable,list}`), including `--workspace` mode. Zed's `settings.json` also carries the user's theme, font, and keymap, so brontes parses it as JSONC, writes back strict JSON, and preserves every non-`context_servers` top-level key.
- `brontes::DescriptionMode` (`Short` / `Long`, default `Long`) plus `Config::description_mode`, `Config::description_mode_for(path, mode)`, and `Config::description(path, text)` for controlling per-tool description text. The literal `description` override bypasses the `long_about`/`about`/`after_help` cascade entirely. Default behavior is unchanged from 0.1.0; consumers opt in surgically per command or globally when verbose `long_about` text wastes the LLM's context budget. Closes the per-command description override gap (the ophis-equivalent of [njayp/ophis#6](https://github.com/njayp/ophis/issues/6), which only partially shipped as PR #7's always-append `cmd.Example`).

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

[Unreleased]: https://github.com/tj-smith47/brontes/compare/v0.7.1...HEAD
[0.7.1]: https://github.com/tj-smith47/brontes/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/tj-smith47/brontes/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/tj-smith47/brontes/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/tj-smith47/brontes/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/tj-smith47/brontes/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/tj-smith47/brontes/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/tj-smith47/brontes/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/tj-smith47/brontes/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/tj-smith47/brontes/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/tj-smith47/brontes/releases/tag/v0.1.0
