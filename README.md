<div align="center">

<img src="https://raw.githubusercontent.com/tj-smith47/brontes/master/.github/logo.svg" width="180" alt="brontes logo">

# brontes

A Rust library that transforms `clap` CLIs into [MCP](https://modelcontextprotocol.io) servers.

[![CI](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![Release](https://github.com/tj-smith47/brontes/actions/workflows/release.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/release.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/tj-smith47/brontes/badges/coverage.json)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

</div>

> *brontes* (Greek: thunder). In myth, the Cyclops smith who forged Zeus's thunderbolts. This crate forges clap CLIs into MCP servers.

Inspired by [njayp/ophis](https://github.com/njayp/ophis). Written by [Claude](https://claude.ai) (Opus 4.6 / 4.7); maintained by us.

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

## Features

- **Stdio MCP server** — `mcp start` runs an stdio server fronting your clap CLI; the launch transport every editor manager wires up.
- **Streamable HTTP MCP server** — `mcp stream` exposes the same tool list over HTTP via rmcp 3.0; loopback-only by default, widen with `--allow-host`.
- **MCP `2026-07-28` support** — stateless requests, `server/discover`, the Tasks extension, MRTR, SEP-2243 header promotion, and SEP-2549 cache hints, with every earlier protocol revision back to `2024-11-05` still negotiable. See [Protocol support](#protocol-support).
- **Editor managers** for Claude Desktop, Cursor, VSCode, and Zed — each with `enable` / `disable` / `list` leaves, `--workspace` per-project mode where applicable, and snapshot-before-write backups. Zed preserves unrelated `settings.json` keys (theme, font, keymap) and tolerates JSONC on load.
- **Long-running commands as tasks** — `Config::task_mode_for(path, TaskMode::Detached)` answers `tools/call` with a handle the client polls, answers, and cancels, instead of blocking for the length of a release. See [Long-running commands as tasks](#long-running-commands-as-tasks-sep-2663).
- **Async middleware** — `Middleware` wraps tool execution for auth, audit logging, rate limiting, or distributed tracing without forking the runtime.
- **Default env injection** — `Config::default_env(key, val)` ships env vars with every tool launch; per-call `env` from the MCP client wins on key conflict.
- **Tool-name prefix** — `Config::tool_name_prefix` replaces the root command name in every generated tool name, avoiding cross-CLI collisions on the same MCP client.
- **Configurable MCP group name** — `Config::command_name` renames the `mcp` subtree if your CLI already has an `mcp` subcommand.
- **Per-flag JSON Schema override** — `Config::flag_schema(cmd_path, flag, schema)` swaps the auto-derived schema for one flag wholesale.
- **Per-flag type override** — `Config::flag_type_override` gives a coarse type hint when a flag's `value_parser` is opaque to type-ID introspection.
- **Per-command annotations** — read-only / destructive / idempotent / open-world hints via `Config::annotation(path, ToolAnnotations)`.
- **`Examples:` from `after_help`** — clap's `after_help` block is appended to the MCP tool description, so `--help` examples reach LLM clients unchanged.
- **Per-command description override** — `Config::description(path, text)` replaces the entire description with LLM-targeted prompt text, bypassing the `long_about` / `about` / `after_help` cascade.
- **Per-command description-mode toggle** — `Config::description_mode_for(path, mode)` overrides the global `Short` / `Long` default on a single command.
- **Deprecation filter** — `Config::deprecate(path)` hides commands from agents while keeping them visible to humans on the CLI.
- **Default description fallback** — every tool gets a sensible `"Execute the {name} command"` description when clap's `about` / `long_about` are both absent, so the MCP tool list is never silently empty-descriptioned.
- **MCP `Implementation` identity** — `Config::implementation` sets the server name/version surfaced to MCP clients and the MCP registry; falls through to `CARGO_PKG_*` if unset.

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

## Protocol support

brontes negotiates every MCP protocol revision rmcp knows, newest first:

| Revision | Notes |
|---|---|
| `2026-07-28` | Stateless — no `initialize` handshake, no `Mcp-Session-Id`. Serves `server/discover`, carries `resultType`, and honours SEP-2243 standard headers (`Mcp-Method` / `Mcp-Name`). |
| `2025-11-25` | Fallback offered to clients that request an unknown version. |
| `2025-06-18`, `2025-03-26`, `2024-11-05` | Still negotiable; these keep the handshake and, over HTTP, a session id. |

A brontes server advertises `tools`, plus the `tasks` extension once any
command is configured to detach. The `2026-07-28` revision deprecates Roots,
Sampling, and Logging, and brontes implements none of them: it logs to stderr
via `tracing` (the migration the spec suggests for Logging), and `Middleware`
is the extension point for anything a server would otherwise ask the client to
do.

### What `2026-07-28` changed, and where it lands

| Change | brontes |
|---|---|
| Stateless: no handshake, no `Mcp-Session-Id` (SEP-2575, SEP-2567) | Served. Nothing in a brontes server was per-session to begin with — the tool list is walked once from an immutable `clap` tree |
| `server/discover` (SEP-2575) | Implemented, with the same cache hints as `tools/list` |
| `resultType` on every result (SEP-2322) | Carried |
| Multi Round-Trip Requests (SEP-2322) | `MiddlewareOutcome::InputRequired`; refused rather than sent when the peer cannot answer |
| Tasks extension (SEP-2663) | `Config::task_mode_for(path, TaskMode::Detached)` — see below |
| `Mcp-Method` / `Mcp-Name` request headers (SEP-2243) | Validated on streamable HTTP |
| `x-mcp-header` parameter promotion (SEP-2243) | `Config::promote_flag` |
| `ttlMs` / `cacheScope` (SEP-2549) | `Config::cache_ttl` / `Config::cache_scope` |
| Trace context in `_meta` (SEP-414) | Lowered onto the child process as `TRACEPARENT` / `TRACESTATE` / `BAGGAGE` |
| Deterministic `tools/list` order | Depth-first `clap` walk order, identical across calls and processes |
| `extensions` on capabilities | Carries the `tasks` declaration |
| Reserved error range `-32020`–`-32099` | brontes mints no error codes of its own; a failed command is a tool result, not a protocol error |

The rest of the revision does not reach a CLI wrapper: `subscriptions/listen`,
the removed `ping` / `logging/setLevel` / roots notifications, the resource
error-code renumbering, and the JSON Schema loosening all concern surfaces
brontes does not expose (it serves no resources, prompts, or completions), and
the authorization changes (SEP-2468, SEP-837, SEP-2352) are client-side.

### Cache hints (SEP-2549)

`tools/list` and `server/discover` results carry `ttlMs` and `cacheScope` so
clients stop re-listing on every turn. A brontes tool list is walked once at
server construction from an immutable `clap` tree and depends on nothing
per-client, so the defaults are five minutes and `public`:

```rust
use std::time::Duration;
use brontes::{CacheScope, Config};

let cfg = Config::default()
    .cache_ttl(Duration::from_secs(30))     // Duration::ZERO = "do not cache"
    .cache_scope(CacheScope::Private);      // for a shared caching proxy
```

Narrow the scope to `Private` when a caching intermediary sits between
brontes and clients in different trust domains.

### Promoting a flag to an HTTP header (SEP-2243)

SEP-2243 lets a streamable-HTTP client mirror a tool argument into an
`Mcp-Param-*` header so proxies and gateways can route on it without parsing
the body. The annotation is honored only on **top-level** properties, and
brontes normally nests every flag under `flags` — so `promote_flag` hoists the
one you name:

```rust
use brontes::Config;

let cfg = Config::default()
    .promote_flag("myapp deploy", "region")               // Mcp-Param-region
    .promote_flag_as("myapp deploy", "api-key", "Api-Key"); // Mcp-Param-Api-Key
```

```jsonc
// tools/list — before                    // tools/list — after
{                                         {
  "properties": {                           "properties": {
    "flags": {                                "flags": {
      "properties": {                           "properties": {
        "region": { "type": "string" }            /* region is gone */
      }                                         }
    },                                        },
    "args": { "type": "array" }               "args": { "type": "array" },
  }                                           "region": {
}                                               "type": "string",
                                                "x-mcp-header": "region"
                                              }
                                            }
                                          }
```

```jsonc
// tools/call — the promoted flag travels at the top level…
{ "region": "us-east-1", "flags": { "dry-run": true }, "args": [] }
```
```http
Mcp-Param-region: us-east-1
```

brontes folds `region` back into `flags` before running the command, so the CLI
receives `--region us-east-1` exactly as it did before promotion.

Three rules keep this from failing silently:

| Rule | Why | On violation |
|---|---|---|
| The flag is no longer accepted under `flags` | A value in two places is a value two callers can disagree about | Tool error naming where it belongs |
| Type must be `string`, `integer`, or `boolean` | A header cannot carry an array or object | `generate_tools` returns `Err` |
| Header names unique per command, valid HTTP tokens | HTTP folds case; an invalid token cannot be sent | `generate_tools` returns `Err` |

`x-mcp-header` inside a `Config::flag_schema` override still does nothing —
that schema lands nested under `flags`, where the annotation is not read — and
logs a warning pointing at `promote_flag`.

### Long-running commands as tasks (SEP-2663)

A `tools/call` blocks until the process exits. That is fine for `version` and
wrong for `release`: the client waits with no progress, no way to check on it,
and no way to stop it. Name those commands and they hand back a handle instead:

```rust
use std::time::Duration;
use brontes::{Config, TaskMode};

let cfg = Config::default()
    .task_mode_for("anodizer release", TaskMode::Detached)
    .task_mode_for("anodizer publish", TaskMode::Detached)
    .task_poll_interval(Duration::from_secs(2));
```

```jsonc
// tools/call — answered immediately
{ "resultType": "task", "task": { "taskId": "0b7f…", "status": "working",
                                  "pollIntervalMs": 2000 } }

// tasks/get — while it runs
{ "taskId": "0b7f…", "status": "working", "statusMessage": "running anodizer release" }

// tasks/get — once it exits, carrying the same result a blocking call returns
{ "taskId": "0b7f…", "status": "completed",
  "result": { "structuredContent": { "stdout": "…", "exit_code": 0 } } }
```

`tasks/cancel` kills the process; the task settles as `cancelled`. A command
that finishes anyway reports what it did, because the side effects already
happened. A middleware asking for input works unchanged — the question leaves
through `tasks/get` and `tasks/update` answers it, then the chain re-enters
with `input_responses` exactly as it would on a blocking retry.

Two properties are worth knowing before you flip it on:

| | |
|---|---|
| A client that never declared the extension | Gets the blocking result, whatever the config says. Detaching is never a compatibility break |
| `Config::task_ttl` unset (the default) | No time limit — the command runs until it exits or is cancelled. A finite TTL **aborts** a command still running when it elapses, and is also what eventually sweeps finished task records; set one on a long-lived `mcp stream` server |

The handle is the security boundary. `2026-07-28` removed protocol sessions, and
the tasks extension replaces them with exactly this: a server-minted handle that
acts as a bearer token for the state behind it. Three properties make that safe,
each covered by a test in [`tests/tasks.rs`](tests/tasks.rs):

| | |
|---|---|
| Handles are unguessable | 122 bits from the OS CSPRNG (`getrandom`) per handle. `handles_are_unguessable_and_never_repeat` rejects a sequence or a counter |
| A handle the server does not hold is never served | `tasks/get`, `tasks/update`, and `tasks/cancel` all answer `-32602` for an unknown id, identically for a well-formed one and a malformed one — so there is no oracle for probing which handles exist, and no `tasks/list` to enumerate them |
| The three task methods are closed to clients that never negotiated the extension | `-32021 Missing Required Client Capability`, checked before the store is touched |

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
  overrides, SEP-2243 header promotion, trace-context propagation, task mode,
  log level, MCP `Implementation` identity, and per-command description
  configuration.
- `brontes::DescriptionMode` — `Short` (prefer `about`) or `Long` (prefer
  `long_about`); default is `Long`.
- `brontes::TaskMode` — `Blocking` (default) or `Detached`, per command via
  `Config::task_mode_for`; bounded by `Config::task_ttl` and paced by
  `Config::task_poll_interval`.
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

[**anodizer**](https://github.com/tj-smith47/anodizer) is the
recommended release tool for brontes-powered CLIs. It is a single
Rust binary that ships your MCP server end-to-end in one
`anodizer release` invocation:

- **Multi-platform binaries** — cross-compiled archives for every
  Tier-1 target (Linux / macOS / Windows × x86_64 / aarch64).
- **Crates.io publish** — `cargo publish` with deterministic builds
  and immutable-version safety rails.
- **GitHub Releases** — auto-generated changelog (Conventional
  Commits), uploaded archives, checksums, and signatures.
- **MCP registry** — direct publish to
  [registry.modelcontextprotocol.io](https://registry.modelcontextprotocol.io/)
  via the `mcp:` block (server name, package shape, transport, auth).
- **Cosign signatures** — keyless OIDC-signed artifacts.
- **Auto-tagging** — Conventional Commits drive the semver bump;
  CI mints the tag, no manual `git tag` step.

brontes' own [`.anodizer.yaml`](.anodizer.yaml) is the worked
reference: it carries an annotated `mcp:` block showing every
registry-publish field, commented out because brontes itself is a
library. Copy the block into your own consumer's `.anodizer.yaml`,
uncomment, and fill in your values — that is the full release
pipeline.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, the local
CI workflow, MSRV policy, and pull-request expectations.

## License

MIT. See [LICENSE](LICENSE).
