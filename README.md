<div align="center">

<img src="https://raw.githubusercontent.com/tj-smith47/brontes/master/.github/logo.svg" width="180" alt="brontes logo">

# brontes

A Rust library that transforms [`clap`](https://docs.rs/clap) CLIs into [MCP](https://modelcontextprotocol.io) servers. Inspired by [njayp/ophis](https://github.com/njayp/ophis).

[![CI](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![Release](https://github.com/tj-smith47/brontes/actions/workflows/release.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/release.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/tj-smith47/brontes/badges/coverage.json)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/brontes.svg)](https://crates.io/crates/brontes)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

</div>

> *brontes* (Greek: thunder). In myth, the Cyclops smith who forged Zeus's thunderbolts. This crate forges clap CLIs into MCP servers.

> **Status:** Beta — used in production by anodizer + cfgd; APIs stabilizing toward 1.0.

## Contents

**Start here**

- [Why brontes](#why-brontes) — what it does and why it's shaped this way
- [Quick start](#quick-start) — two lines on an existing CLI
- [How it works](#how-it-works) — the walker, the runtime, the editor configs

**Running a server**

- [Editor integration](#editor-integration) — Claude Desktop, Cursor, VSCode, Zed
- [Trimming the tool list](#trimming-the-tool-list) — groups and selection flags
- [Features](#features) — the full surface at a glance

**Going further**

- [Advanced](#advanced) — middleware, descriptions, env, schema overrides, HTTP
- [Protocol support](#protocol-support) — what `2026-07-28` changed, cache hints, header promotion, tasks
- [API reference](#api-reference) — every public item
- [Releasing an MCP server built with brontes](#releasing-an-mcp-server-built-with-brontes)
- [Contributing](#contributing) · [License](#license)

## Why brontes

- **Two lines, no rewrite.** Mount `brontes::command`, dispatch with
  `brontes::handle`, and every clap subcommand is an MCP tool. There is no
  trait to implement and no schema to hand-write — the tool list *is* your
  clap tree, and a tool call re-enters your binary as ordinary argv, so one
  code path serves humans and agents.
- **Current with the protocol.** brontes speaks MCP `2026-07-28` — stateless
  requests, `server/discover`, Tasks, MRTR, header promotion, cache hints —
  and still negotiates every revision back to `2024-11-05`.
- **Built for small context budgets.** Group your commands and let users spin
  up a server with just the ones they need. Per-command description overrides
  and a `Short`/`Long` toggle trim the rest.
- **Long-running commands don't block.** Mark a command detached and
  `tools/call` returns a handle the client polls, answers, and cancels,
  instead of holding a connection open for the length of a release.
- **One extension point that covers the rest.** `Middleware` wraps every tool
  call, so auth, audit logging, rate limiting, and tracing are your code, not
  a fork of the runtime.
- **Sensible defaults.** HTTP is loopback-only until you widen it. A config
  that names a command you don't have fails at startup, not on the first
  request.

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
| `--group` / `--command` / `--tool` and `--hide-*` (repeatable), `--all` | Register a trimmed tool list — written into the `mcp start` args the editor spawns. See [Trimming the tool list](#trimming-the-tool-list) |

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

## Trimming the tool list

A forty-command CLI makes a bad MCP server. Every tool descriptor costs context
before any work happens, and a model picking from forty tools picks worse than
one picking from five.

Group the commands that go together:

```rust
use brontes::Config;

let cfg = Config::default()
    .group("release", ["release", "publish"])
    .group_description("release", "Cut, sign, and publish a release")
    .group("inspect", ["status", "verify"])
    .group_description("inspect", "Read-only checks");
```

Then users start a server with only what they want:

```bash
$ anodizer mcp tools --groups
inspect  2 tools  Read-only checks
release  3 tools  Cut, sign, and publish a release

$ anodizer mcp start --group release
$ anodizer mcp start --command release --command publish
$ anodizer mcp start --tool anodizer_release --tool anodizer_publish
$ anodizer mcp start --group release --hide-tool anodizer_release_notes
```

`mcp start`, `mcp stream`, and `mcp tools` all take the same seven flags, so
`mcp tools` shows you exactly what a server would serve:

| | select | remove |
|---|---|---|
| by group | `--group <NAME>` | `--hide-group <NAME>` |
| by command path | `--command <PATH>` | `--hide-command <PATH>` |
| by MCP tool name | `--tool <NAME>` | `--hide-tool <NAME>` |
| everything | `--all` | — |

A CLI can pin a default selection in `Config` — sensible when the command tree
is large enough that serving all of it costs more context than it's worth.
Since every other flag above is additive, `--all` is how a user gets back to
the full list:

```bash
$ cfgd mcp tools                   # what the CLI pins: 9 tools
$ cfgd mcp tools --group modules   # 19
$ cfgd mcp tools --all             # 86
```

`--all` discards the exposing side only. A `hide_*` the CLI's author set stays
set, so `--all --hide-command secrets` reads the way it looks.

`mcp tools --groups` lists what a CLI defines and takes none of them — it
describes the CLI, not one server's subset.

The editor managers pass them through, so the trim sticks:

```bash
$ anodizer mcp claude enable --group release
# writes args: ["mcp", "start", "--group", "release"]
```

Details worth knowing:

- Paths are relative to the CLI itself: `--command release`, not
  `--command "anodizer release"`. Both work, since the leading segment is read
  as the CLI's own name when it matches.
- A group or `--command` covers the whole subtree — `release` picks up
  `anodizer release notes` too, so groups don't go stale when you add a
  subcommand. Use `--tool` when you want a command without its children.
- Matching respects path boundaries: `--command release` won't grab
  `anodizer releases`. Paths use the command's real name — a clap alias isn't
  a path.
- Removing beats selecting, and a `hide_*` set in `Config` can't be overridden
  from the command line.
- Everything you ask for has to arrive. A name the CLI doesn't have fails at
  startup, and so does one that resolves to a real command which won't be
  served anyway — because it's deprecated, hidden, or excluded by a selector.
  A server quietly missing a tool you asked for is indistinguishable from a
  CLI that never had it. `mcp <editor> enable` runs the same check before it
  writes anything, so a typo can't reach a config file.

  ```console
  $ anodizer mcp start --group relase
  Error: Config("no such group \"relase\"; this CLI defines inspect, release")

  $ anodizer mcp start --command publish
  Error: Config("command \"anodizer publish\" was selected but exposes no tools;
  it is removed by a hide flag, deprecated, or excluded by a selector")
  ```

Trimming happens at launch rather than per-request because `2026-07-28` dropped
protocol sessions — `tools/list` returns the same thing to every caller. Two
tool sets means two servers, which is how editor configs work anyway.

## Features

- **Stdio MCP server** — `mcp start` runs an stdio server fronting your clap CLI; the launch transport every editor manager wires up.
- **Streamable HTTP MCP server** — `mcp stream` exposes the same tool list over HTTP via rmcp 3.0; loopback-only by default, widen with `--allow-host`.
- **MCP `2026-07-28` support** — stateless requests, `server/discover`, the Tasks extension, MRTR, SEP-2243 header promotion, and SEP-2549 cache hints, with every earlier protocol revision back to `2024-11-05` still negotiable. See [Protocol support](#protocol-support).
- **Command groups** — `Config::group(name, paths)` names a bundle of related commands so users can ask for it by name. See [Trimming the tool list](#trimming-the-tool-list).
- **Launch-time tool selection** — `--group` / `--command` / `--tool` and their `--hide-` counterparts on `mcp start`, `mcp stream`, and `mcp tools` start a server with only the tools a given job needs, and `--all` discards a selection the CLI pinned. The editor managers write the selection into the config they register.
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

### Tool-name prefix and subcommand name

`tool_name_prefix` replaces the root command name when constructing each MCP tool's name — useful when multiple brontes-mounted CLIs attach to the same MCP client and you want to avoid collisions. `command_name` renames the `mcp` subcommand on the user's CLI — useful when your CLI already has an `mcp` subcommand.

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
| `--group` / `--command` / `--tool`, `--hide-*`, `--all` | *(none)* | Trim the tool list; see [Trimming the tool list](#trimming-the-tool-list) |

See [SECURITY.md](SECURITY.md) for the full HTTP-transport threat model.

## Protocol support

brontes negotiates every revision rmcp knows, newest first: `2026-07-28`,
`2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05`. Clients on the older
ones keep the `initialize` handshake and, over HTTP, a session id.

On `2026-07-28` you get the stateless request model (no handshake, no
`Mcp-Session-Id`), `server/discover`, `resultType` on every result, the Tasks
extension, MRTR for anything a middleware needs to ask the client,
`Mcp-Method` / `Mcp-Name` request headers, `x-mcp-header` parameter promotion,
`ttlMs` / `cacheScope` cache hints, and W3C trace context lowered onto the
spawned process. Tool listings come back in your clap declaration order, stable
across calls, so clients and prompt caches can cache them.

The parts that don't apply: brontes serves no resources, prompts, or
completions, so `subscriptions/listen`, the resource error-code change, and the
JSON Schema loosening don't reach it. Roots, Sampling, and Logging are
deprecated in this revision and brontes implements none of them — it logs to
stderr via `tracing`, and `Middleware` covers what a server would otherwise ask
a client to do. Authorization changes are client-side.

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
    .promote_flag("deploy", "region")               // Mcp-Param-region
    .promote_flag_as("deploy", "api-key", "Api-Key"); // Mcp-Param-Api-Key
```

```text
tools/list — before                     tools/list — after
{                                       {
  "properties": {                         "properties": {
    "flags": {                              "flags": {
      "properties": {                         "properties": {}
        "region": { … }                     },
      }                                     "args": { "type": "array" },
    },                                      "region": {
    "args": { "type": "array" }               "type": "string",
  }                                           "x-mcp-header": "region"
}                                           }
                                          }
                                        }
```

The promoted flag then travels at the top level of `tools/call`:

```json
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
    .task_mode_for("release", TaskMode::Detached)
    .task_mode_for("publish", TaskMode::Detached)
    .task_poll_interval(Duration::from_secs(2));
```

`tools/call` is answered immediately, with a handle instead of a result:

```json
{ "resultType": "task", "task": { "taskId": "0b7f…", "status": "working", "pollIntervalMs": 2000 } }
```

`tasks/get` reports progress while it runs:

```json
{ "taskId": "0b7f…", "status": "working", "statusMessage": "running anodizer release" }
```

and once it exits, carries the same result a blocking call would have returned:

```json
{ "taskId": "0b7f…", "status": "completed", "result": { "structuredContent": { "stdout": "…", "exit_code": 0 } } }
```

`tasks/cancel` kills the process; the task settles as `cancelled`. A command
that finishes anyway reports what it did, because the side effects already
happened. A middleware asking for input works unchanged — the question leaves
through `tasks/get` and `tasks/update` answers it, then the chain re-enters
with `input_responses` exactly as it would on a blocking retry.

Two things to know before turning it on. A client that never declared the
extension gets the blocking result no matter what the config says, so detaching
is never a compatibility break. And `task_ttl` is unset by default, meaning no
time limit — a finite TTL *aborts* a command still running when it elapses, but
it's also what sweeps finished task records, so set one on a long-lived
`mcp stream` server.

Task ids are the only thing standing between a caller and a task, so brontes
treats them as bearer tokens: 122 bits from the OS CSPRNG each, an unknown id
gets the same `-32602` whether it's well-formed or garbage, and the three task
methods are closed to clients that never negotiated the extension.

## API reference

- `brontes::command(cfg)` / `brontes::handle(matches, cli, cfg)` /
  `brontes::run(cli, cfg)` — mount, dispatch, and one-shot runners for the
  `mcp` subtree (`mcp start` for stdio, `mcp stream` for streamable HTTP,
  `mcp tools` to export the tool list, `mcp claude {enable,disable,list}`,
  `mcp cursor {enable,disable,list}`, `mcp vscode {enable,disable,list}`).
- [`generate_tools`]`(root, cfg) -> Result<Vec<rmcp::model::Tool>>` —
  offline tool-list builder for consumers that wire their own server.
- `brontes::Config` — fluent builder for tool-name prefix, selectors,
  command groups and tool selection, default env, annotations, deprecated
  commands, per-flag schema/type overrides, SEP-2243 header promotion,
  trace-context propagation, task mode, log level, MCP `Implementation`
  identity, and per-command description configuration.
- `brontes::Group` / `brontes::ToolFilter` — the shapes behind
  `Config::group` and the `--group` / `--command` / `--tool` launch flags.
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
