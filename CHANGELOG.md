# Changelog

All notable changes to this project are documented here. Format adapted from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning follows [SemVer](https://semver.org/).

## [Unreleased]

### Added

- `brontes::generate_tools(root, cfg)` — turns a `clap::Command` tree
  and a `brontes::Config` into the `Vec<rmcp::model::Tool>` an MCP
  server should advertise from `list_tools`. Library-level entry point
  for callers building their own MCP server.
- `brontes::Config` — fluent builder for naming, selector filtering,
  per-tool annotations, deprecated-command set, per-flag schema
  overrides, default env, log level, and rmcp `Implementation`
  metadata. Marked `#[non_exhaustive]`; future fields will be additive.
- `brontes::Selector` plus the `brontes::selectors::*` factory
  functions (`allow_cmds`, `exclude_cmds`, `allow_cmds_containing`,
  `exclude_cmds_containing`, `allow_flags`, `exclude_flags`,
  `no_flags`) and their underlying `CmdMatcher` / `FlagMatcher` types
  for filtering commands and flags out of the tool surface.
- `brontes::ToolAnnotations` — MCP read-only / destructive /
  idempotent / open-world hints keyed by full command path. Annotation
  paths that don't match any walked command return a clear
  `Error::Config` from `generate_tools` rather than failing silently.
- `brontes::ToolInput` / `brontes::ToolOutput` — the wire shapes
  brontes uses at the MCP tool-call boundary.
- `brontes::SchemaType` — coarse type classification consumed by
  `Config::flag_type_override`.
- `brontes::MiddlewareCtx`, `Middleware`, `BoxedNext`,
  `MiddlewareResult` — types for the eventual middleware execution
  path. `MiddlewareCtx` is `#[non_exhaustive]`; downstream code
  receives a value, does not construct one directly.
- `brontes::Error` (non-exhaustive) and `brontes::Result` — library
  error plumbing. Brontes does not implement `Termination`; pair with
  `Result<(), brontes::Error>` in `main`.
- `brontes::command(cfg)` — build the `mcp` subcommand subtree
  (`mcp start`, `mcp tools`, `mcp stream`) ready to mount on a parent
  `clap::Command`. Validates the configured group name and surfaces a
  sibling-collision error from `handle` when the user's CLI already
  carries a same-named subcommand.
- `brontes::handle(matches, cli, cfg)` — dispatch an `mcp` subcommand
  match. Async; routes `start`, `tools`, and `stream` to their
  respective runtimes.
- `brontes::run(cli, cfg)` — one-call sugar that mounts the subtree,
  parses argv, and dispatches. Intended for tiny CLIs whose only
  purpose is the MCP server.
- `mcp start` — stdio MCP server runtime over `rmcp::transport::stdio`,
  with `--log-level` flag, stderr-logging tracing subscriber, and
  signal-driven graceful shutdown.
- `mcp tools` — exports the generated tool list to `./mcp-tools.json`
  as pretty-printed JSON.
- `mcp stream --host <HOST> --port <PORT>` — streamable-HTTP MCP server
  runtime over `rmcp::transport::streamable_http_server::StreamableHttpService`
  (rmcp 1.6) driven by a hyper per-connection accept loop. Empty
  `--host` binds `0.0.0.0` (Go-parity); the startup log line matches
  ophis verbatim. Signal-driven cancellation (SIGINT/SIGTERM on Unix,
  Ctrl+C on Windows) with a 5-second graceful-drain window.
- `mcp claude {enable, disable, list}` — manage Claude Desktop's
  `claude_desktop_config.json` with `--config-path`, `--server-name`,
  `--env` (repeatable `-e KEY=VAL`), and `--log-level`. Per-OS path
  resolution (macOS `~/Library/Application Support/Claude/...`, Linux
  `$XDG_CONFIG_HOME` or `~/.config/Claude/...`, Windows
  `%APPDATA%\Claude\...`). Backup-before-write semantics: the existing
  file is copied to `<base>.backup.json` before any save.
- `mcp cursor {enable, disable, list}` — manage Cursor's `mcp.json`
  with the same flags as `claude` plus `--workspace` accepted on all
  three leaves. Without `--workspace` the path is per-OS user-mode
  (`~/.cursor/mcp.json`); with `--workspace` the target is
  `$CWD/.cursor/mcp.json`. JSON shape carries the VSCode-compatible
  `type/command/args/env/url/headers` server fields and an optional
  `inputs[]` array preserved on round-trip.

### Notes

- MSRV is 1.94.

[Unreleased]: https://github.com/tj-smith47/brontes/compare/master...HEAD
