# brontes

[![CI](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![Release](https://github.com/tj-smith47/brontes/actions/workflows/release.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/release.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/tj-smith47/brontes/badges/coverage.json)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> *brontes* (Greek: thunder). In myth, the Cyclops smith who forged Zeus's thunderbolts. This crate will forge clap CLIs into MCP servers.

A Rust library for transforming `clap` CLIs into [MCP](https://modelcontextprotocol.io) servers, inspired by [njayp/ophis](https://github.com/njayp/ophis).

Written by [Claude](https://claude.ai) (Opus 4.6 / 4.7); maintained by us.

> **Status:** Beta — used in production by anodizer + cfgd; APIs stabilizing toward 1.0.

## Why brontes

- **Ship existing CLIs to AI agents in two lines.** Mount `brontes::command` and dispatch with `brontes::handle`; every clap subcommand becomes an MCP tool, instantly usable from Claude Desktop, Cursor, VSCode, and Zed.
- **Token-efficient by design.** Per-command description overrides, `Short`/`Long` mode toggle, deprecation filter, and `after_help` "Examples:" promotion let you trim the description surface the LLM has to read.
- **Production-ready security defaults.** Streamable HTTP transport is loopback-only by default via rmcp's DNS-rebind allow-list; widen it explicitly with `--allow-host`. Auth is not built in — wire it through `Middleware`.
- **Async middleware boundary.** Wrap tool execution with auth, audit logging, rate limiting, or distributed tracing without forking the runtime.

## How it works

```
  clap::Command tree
        │
        ▼
  ┌───────────────────┐         ┌──────────────────────┐
  │  brontes walker   │ ──────▶ │  Vec<rmcp::Tool>     │
  │  (selectors,      │         │  (one tool per       │
  │   annotations,    │         │   reachable command) │
  │   descriptions)   │         └──────────────────────┘
  └───────────────────┘                   │
                                          ▼
                              ┌──────────────────────────┐
                              │  MCP server runtime      │
                              │   • stdio  (mcp start)   │
                              │   • HTTP   (mcp stream)  │
                              └──────────────────────────┘
                                          │
                                          ▼
                              ┌──────────────────────────┐
                              │  Editor configs          │
                              │   • Claude Desktop       │
                              │   • Cursor (user + ws)   │
                              │   • VSCode (user + ws)   │
                              │   • Zed    (user + ws)   │
                              └──────────────────────────┘
```

The **walker** recursively visits every `clap::Command`, applies safety filters (deprecated commands, selector predicates), and turns each leaf into an `rmcp::model::Tool` with a JSON-Schema-typed input map derived from the command's flags. Layered on at this stage:

- Annotations
- Per-command description overrides
- Per-flag schema overrides

The **runtime** wraps that tool list in an `rmcp` server and serves it over either stdin/stdout (for editor-launched processes) or streamable HTTP (for sidecar deployments). Tool invocations re-enter your binary as ordinary clap argv, so the same code path serves humans and agents.

## Quick start

Two lines mount and dispatch the `mcp` subtree on any existing `clap` CLI:

```rust
use clap::Command;

#[tokio::main]
async fn main() -> brontes::Result<()> {
    let cli = Command::new("my-cli")
        .version("0.1.0")
        .subcommand(Command::new("greet").about("Say hi"))
        .subcommand(brontes::command(None));                  // [1] mount

    let matches = cli.clone().get_matches();
    match matches.subcommand() {
        Some(("mcp",   sub)) => brontes::handle(sub, &cli, None).await,  // [2] dispatch
        Some(("greet", _))   => { println!("hi"); Ok(()) }
        _ => Ok(()),
    }
}
```

For tiny CLIs whose only purpose is the MCP server, collapse the ceremony
into one line with `brontes::run`:

```rust
use clap::Command;

#[tokio::main]
async fn main() -> brontes::Result<()> {
    brontes::run(Command::new("my-cli").version("0.1.0"), None).await
}
```

## Editor integration

brontes ships built-in commands to register the resulting MCP server in the three major AI-aware editors. Each manager writes a JSON config file in the editor's standard location, snapshots the existing file to `<base>.backup.json` before any in-place mutation, and exposes `enable` / `disable` / `list` leaves for symmetric lifecycle control.

```bash
# Register the server in Claude Desktop
$ my-cli mcp claude enable
Wrote ~/Library/Application Support/Claude/claude_desktop_config.json
(backup at ~/Library/Application Support/Claude/claude_desktop_config.backup.json)

# Register in Cursor (user mode, ~/.cursor/mcp.json)
$ my-cli mcp cursor enable

# Register in Cursor (per-workspace, lives in $CWD/.cursor/mcp.json)
$ my-cli mcp cursor enable --workspace

# Register in VSCode (user mode)
$ my-cli mcp vscode enable

# Register in Zed (user mode, ~/.config/zed/settings.json on macOS/Linux,
# %APPDATA%\Zed\settings.json on Windows; preserves theme/font/keymap and
# any other unrelated top-level keys in settings.json on round-trip).
$ my-cli mcp zed enable

# Per-workspace Zed config (lives in $CWD/.zed/settings.json).
$ my-cli mcp zed enable --workspace

# List the configured servers for a given editor
$ my-cli mcp claude list

# Remove the brontes-managed entry
$ my-cli mcp cursor disable --workspace
```

Shared flags on every `enable`:

| Flag | Purpose |
|------|---------|
| `--config-path <PATH>` | Override the per-editor default config location |
| `--server-name <NAME>` | Override the MCP server key written into the config (defaults to the binary name) |
| `--env KEY=VAL` (`-e`, repeatable) | Append environment variables the editor will inject when launching the server |
| `--log-level <LEVEL>` | Set the server's tracing level (`trace`/`debug`/`info`/`warn`/`error`) |

`--workspace` is additionally accepted on `cursor`, `vscode`, AND `zed`'s
`enable`, `disable`, and `list` leaves — pass it whenever you want the
workspace-mode config (`$CWD/.cursor/mcp.json`, `$CWD/.vscode/mcp.json`,
or `$CWD/.zed/settings.json`) instead of the per-OS user config.

Zed differs structurally from the other three editors: its `settings.json`
also carries the user's theme, font, keymap, and other editor settings.
brontes parses the file as JSONC (line comments and trailing commas are
tolerated on load), writes back strict JSON, and preserves every
non-`context_servers` top-level key verbatim. The first write strips
JSONC comments — same trade-off the upstream Zed CLI accepts when it
rewrites the file.

Backups are **only** written when an existing file is mutated — first writes don't litter `.backup.json` files. See [SECURITY.md](SECURITY.md) for the editor-config threat surface.

## How brontes compares

| Feature | brontes (Rust) | [ophis](https://github.com/njayp/ophis) (Go) |
|---|---|---|
| CLI framework | clap | cobra |
| Stdio MCP server | yes | yes |
| Streamable HTTP MCP | yes (rmcp 1.6) | yes ([#15](https://github.com/njayp/ophis/pull/15)) |
| Editor managers: Claude / Cursor / VSCode / Zed | all four (Zed preserves unrelated `settings.json` keys + accepts JSONC on load) | Claude / Cursor / VSCode shipped; Zed pending [#46](https://github.com/njayp/ophis/pull/46) |
| Middleware (async wrap) | yes | yes ([#34](https://github.com/njayp/ophis/pull/34)) |
| Default env injection | `Config::default_env` | `DefaultEnv` ([#44](https://github.com/njayp/ophis/pull/44)) |
| Tool name prefix | `Config::tool_name_prefix` | yes ([#37](https://github.com/njayp/ophis/pull/37)) |
| Configurable MCP group name | `Config::command_name` | yes ([#40](https://github.com/njayp/ophis/pull/40)) |
| Per-flag JSON Schema override | `Config::flag_schema` | no |
| Per-flag type override | `Config::flag_type_override` | partial — base JSON Schema added ([#32](https://github.com/njayp/ophis/pull/32)) |
| Per-command annotations (read-only / destructive) | `Config::annotation` (path-keyed) | via cobra annotations ([#38](https://github.com/njayp/ophis/pull/38)) |
| `Example` / `after_help` appended to description | via clap's `after_help` | via `cmd.Example` ([#7](https://github.com/njayp/ophis/pull/7)) |
| **Per-command description override** | `Config::description(path, text)` | no |
| **Per-command description mode toggle** | `Config::description_mode_for` | no |
| Deprecation filter (hide cmds from agents) | `Config::deprecate` | no |
| Default description fallback | `"Execute the {name} command"` | no (empty if no Long/Short) |

## Advanced

### Middleware — auth, audit, tracing

A `Middleware` is an `Arc`'d async closure attached to a `Selector` that wraps tool execution. Use it to enforce auth, emit audit records, rate-limit, or attach distributed-tracing spans around every dispatched tool call.

```rust
use std::sync::Arc;
use brontes::{BoxedNext, Config, Middleware, MiddlewareCtx, Selector};
use clap::Command;
use tracing;

#[tokio::main]
async fn main() -> brontes::Result<()> {
    let audit: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
        Box::pin(async move {
            let tool = ctx.tool_name.clone();
            tracing::info!(%tool, "tool-call begin");
            let result = next(ctx).await;
            tracing::info!(%tool, ok = result.is_ok(), "tool-call end");
            result
        })
    });

    let cfg = Config::default().selector(Selector {
        middleware: Some(audit),
        ..Default::default()
    });

    let cli = Command::new("my-cli").version("0.1.0");
    brontes::run(cli, Some(cfg)).await
}
```

`MiddlewareCtx` carries the cancellation token, tool name, and deserialized `ToolInput`. `BoxedNext` is a one-shot `FnOnce`; call `next(ctx).await` exactly once to delegate to the wrapped exec step.

### Per-command description configuration

Three knobs control what text becomes the MCP tool description. Default is `DescriptionMode::Long` (prefer clap's `long_about`, fall back to `about`).

```rust
use brontes::{Config, DescriptionMode};

let cfg = Config::default()
    // 1) Flip the global default to the short field.
    .description_mode(DescriptionMode::Short)
    // 2) Restore long-form for one command that needs the verbose blurb.
    .description_mode_for("my-cli deploy prod", DescriptionMode::Long)
    // 3) Replace the entire description with LLM-targeted prompt text.
    //    Bypasses the long_about / about / after_help cascade entirely.
    .description(
        "my-cli apply",
        "Apply config changes. Always run with --dry-run first to preview drift.",
    );
```

The literal `description` override is **not** appended to by the `after_help` "Examples:" block — you control the exact bytes sent to the MCP client. Empty / whitespace-only override text is rejected at `generate_tools` time as `Error::Config`. Closes the [njayp/ophis#6](https://github.com/njayp/ophis/issues/6) gap.

### Default env injection

Inject environment variables into every tool invocation. Per-call `env` from the MCP client wins on key conflict.

```rust
use brontes::Config;

let cfg = Config::default()
    .default_env("LOG_FORMAT", "json")
    .default_env("REGION", "us-east-1");
```

When both maps are empty the `env` key is omitted from the MCP wire payload entirely.

### Per-flag schema and type overrides

`flag_schema` replaces the auto-derived JSON Schema for one flag wholesale (auto default/required/enum extraction is skipped). `flag_type_override` provides a coarse type hint for flags whose `value_parser` is opaque to brontes's type-ID introspection.

```rust
use brontes::{Config, SchemaType};

let cfg = Config::default()
    // Wholesale schema replacement.
    .flag_schema(
        "my-cli list",
        "limit",
        serde_json::json!({"type": "integer", "minimum": 0, "maximum": 1000}),
    )
    // Coarse type hint when value_parser is a custom function.
    .flag_type_override("my-cli list", "filter", SchemaType::Array);
```

### Server identity (registry-ready)

Set the MCP `Implementation` (server name and version) surfaced to MCP clients. Required when your binary name differs from the desired MCP server identity, or when publishing to the [MCP registry](https://registry.modelcontextprotocol.io/) — see the [Releasing](#releasing-an-mcp-server-built-with-brontes) section below.

```rust
use brontes::Config;
use rmcp::model::Implementation;

let cfg = Config::default()
    .implementation(Implementation::new("my-agent", "0.1.0"));
```

If unset, brontes falls through to `Implementation::default()`, which derives name/version from `CARGO_PKG_NAME` / `CARGO_PKG_VERSION`.

### Tool-name prefix and group name

`tool_name_prefix` replaces the root command name when constructing each MCP tool's name — useful when multiple brontes-mounted CLIs attach to the same MCP client and you want to avoid collisions. `command_name` renames the `mcp` subcommand group on the user's CLI — useful when your CLI already has an `mcp` subcommand.

```rust
use brontes::Config;

let cfg = Config::default()
    .tool_name_prefix("agent")     // tools become "agent_list", "agent_delete", etc.
    .command_name("agent");        // the brontes subtree mounts as `my-cli agent ...`
```

### Deprecation

Mark a command path as deprecated to filter it out of the generated tool list — the command still exists for humans on the CLI, but agents won't see it.

```rust
use brontes::Config;

let cfg = Config::default().deprecate("my-cli legacy-import");
```

This is brontes-only — ophis has no equivalent.

### Streamable HTTP — DNS-rebind allow-list

`mcp stream` exposes the MCP server over HTTP. rmcp's DNS-rebind guard defaults to allowing only `localhost`, `127.0.0.1`, and `::1` in the `Host:` header; requests from any other hostname get a silent 403. To widen the allow-list for LAN or public exposure, pass `--allow-host` once per reachable hostname:

```bash
$ my-cli mcp stream --host 0.0.0.0 --port 8080 \
    --allow-host myhost.local \
    --allow-host 192.168.1.10
```

`mcp stream` flags:

| Flag | Default | Notes |
|------|---------|-------|
| `--host <HOST>` | `0.0.0.0` (bind-all) | Bind address |
| `--port <PORT>` | `8080` | TCP port |
| `--log-level <LEVEL>` | `info` | trace / debug / info / warn / error |
| `--allow-host <HOST>` | *(none)* | Append to rmcp's DNS-rebind allow-list (repeatable) |

See [SECURITY.md](SECURITY.md) for the full HTTP-transport threat model.

## API reference

- `brontes::command(cfg)` / `brontes::handle(matches, cli, cfg)` /
  `brontes::run(cli, cfg)` — mount, dispatch, and one-shot runners for the
  `mcp` subtree (`mcp start` for stdio, `mcp stream` for streamable HTTP,
  `mcp tools` to export the tool list, `mcp claude {enable,disable,list}`,
  `mcp cursor {enable,disable,list}`, `mcp vscode {enable,disable,list}`).
- [`generate_tools`]`(root, cfg) -> Result<Vec<rmcp::model::Tool>>` —
  offline tool-list builder for consumers that wire their own server.
- `brontes::Config` — fluent builder for tool-name prefix, selectors,
  default env, annotations, deprecated commands, per-flag schema/type
  overrides, log level, MCP `Implementation` identity, and per-command
  description configuration.
- `brontes::DescriptionMode` — `Short` (prefer `about`) or `Long` (prefer
  `long_about`); default is `Long`.
- `brontes::Selector` + `brontes::selectors::{allow_cmds, exclude_cmds,
  allow_cmds_containing, exclude_cmds_containing, allow_flags, exclude_flags,
  no_flags}` — built-in matcher factories.
- `brontes::Middleware` / `brontes::MiddlewareCtx` / `brontes::MiddlewareResult` / `brontes::BoxedNext` —
  async wrap around tool execution.
- `brontes::ToolAnnotations` — typed mirror of rmcp's annotation surface.
- `brontes::ToolInput` / `brontes::ToolOutput` — the MCP tool-call payload
  shapes.
- `brontes::SchemaType` — coarse type classifier for per-flag overrides.
- `brontes::Error` / `brontes::Result` — error surface.

[`generate_tools`]: https://docs.rs/brontes/latest/brontes/fn.generate_tools.html

## Releasing an MCP server built with brontes

If you are shipping a CLI that mounts `brontes::command` and want the
resulting MCP server to land on the public
[MCP registry](https://registry.modelcontextprotocol.io/), brontes' own
[`.anodizer.yaml`](.anodizer.yaml) carries an annotated `mcp:` block
showing every field — registry name, package shape, transport,
auth method — that
[anodizer](https://github.com/tj-smith47/anodizer) needs to publish your
release end-to-end. The block is commented out in this repo because
brontes itself is a library, not a runnable server; copy it into your
own consumer's `.anodizer.yaml`, uncomment, and fill in your values.

## Repository

https://github.com/tj-smith47/brontes

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, the local
CI workflow, MSRV policy, and pull-request expectations.

## License

MIT. See [LICENSE](LICENSE).
