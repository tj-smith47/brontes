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

### Notes

- MSRV is 1.94.
- This release ships the library surface only. The MCP server
  runtime, editor manager subcommands, and HTTP streamable transport
  are not yet shipped — track GitHub issues for the rollout.

[Unreleased]: https://github.com/tj-smith47/brontes/compare/master...HEAD
