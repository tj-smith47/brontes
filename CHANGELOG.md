# Changelog

All notable changes to this project are documented here. Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning follows [SemVer](https://semver.org/).

## [Unreleased]

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
