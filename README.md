# brontes

[![CI](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![Release](https://github.com/tj-smith47/brontes/actions/workflows/release.yml/badge.svg)](https://github.com/tj-smith47/brontes/actions/workflows/release.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/tj-smith47/brontes/badges/coverage.json)](https://github.com/tj-smith47/brontes/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> *brontes* (Greek: thunder). In myth, the Cyclops smith who forged Zeus's thunderbolts. This crate will forge clap CLIs into MCP servers.

A Rust library for transforming `clap` CLIs into [MCP](https://modelcontextprotocol.io) servers, inspired by [njayp/ophis](https://github.com/njayp/ophis).

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

## Public surface

- `brontes::command(cfg)` / `brontes::handle(matches, cli, cfg)` /
  `brontes::run(cli, cfg)` — mount, dispatch, and one-shot runners for the
  `mcp` subtree (`mcp start` for stdio, `mcp stream --host <H> --port <P>`
  for streamable HTTP, `mcp tools` to export the tool list,
  `mcp claude {enable,disable,list}` to manage Claude Desktop's
  MCP server config, and `mcp cursor {enable,disable,list}` with
  `--workspace` to manage Cursor's user or workspace `mcp.json`).

  `mcp stream` flags:

  | Flag | Default | Notes |
  |------|---------|-------|
  | `--host <HOST>` | `0.0.0.0` (bind-all) | Bind address |
  | `--port <PORT>` | `8080` | TCP port |
  | `--log-level <LEVEL>` | `info` | trace / debug / info / warn / error |
  | `--allow-host <HOST>` | *(none)* | Append to rmcp's DNS-rebind allow-list (repeatable) |

  rmcp's DNS-rebind guard defaults to allowing only `localhost`, `127.0.0.1`,
  and `::1`. Requests from any other `Host:` header get a silent 403. For LAN
  or public exposure, add each reachable hostname:

  ```bash
  my-cli mcp stream --host 0.0.0.0 --port 8080 \
      --allow-host myhost.local \
      --allow-host 192.168.1.10
  ```
- `brontes::generate_tools(root, cfg) -> Result<Vec<rmcp::model::Tool>>` —
  offline tool-list builder for consumers that wire their own server.
- `brontes::Config` — fluent builder for tool-name prefix, selectors,
  default env, annotations, deprecated commands, per-flag schema overrides,
  log level, and MCP `Implementation` identity.
- `brontes::Selector` + `brontes::selectors::{allow_cmds, exclude_cmds,
  allow_cmds_containing, exclude_cmds_containing, allow_flags, exclude_flags,
  no_flags}` — built-in matcher factories.
- `brontes::ToolAnnotations` — typed mirror of rmcp's annotation surface.
- `brontes::ToolInput` / `brontes::ToolOutput` — the MCP tool-call payload
  shapes.
- `brontes::SchemaType` — coarse type classifier for per-flag overrides.
- `brontes::Error` / `brontes::Result` — error surface.

```rust
use brontes::{generate_tools, Config, ToolAnnotations};
use clap::{Arg, Command};

fn build() -> brontes::Result<Vec<rmcp::model::Tool>> {
    let cli = Command::new("my-cli")
        .subcommand(Command::new("list").about("List things"))
        .subcommand(
            Command::new("delete")
                .about("Delete a thing")
                .arg(Arg::new("name").required(true)),
        );

    let cfg = Config::default()
        .annotation(
            "my-cli list",
            ToolAnnotations { read_only_hint: Some(true), ..Default::default() },
        )
        .annotation(
            "my-cli delete",
            ToolAnnotations { destructive_hint: Some(true), ..Default::default() },
        );

    generate_tools(&cli, &cfg)
}
```

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
