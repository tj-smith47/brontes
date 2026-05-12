# brontes

> *brontes* (Greek: thunder). In myth, the Cyclops smith who forged Zeus's thunderbolts. This crate will forge clap CLIs into MCP servers.

A Rust library for transforming `clap` CLIs into [MCP](https://modelcontextprotocol.io) servers, inspired by [njayp/ophis](https://github.com/njayp/ophis).

## Status

Early development. The library currently exposes the [`generate_tools`] entry
point and its supporting types — a consumer can build the MCP tool list for
their `clap::Command` tree right now without a running MCP server. The MCP
server runtime itself (the `mcp` subcommand tree, stdio and HTTP transports,
editor-config helpers for Claude / VSCode / Cursor) is not yet shipped.

Public surface at this revision:

- `brontes::generate_tools(root, cfg) -> Result<Vec<rmcp::model::Tool>>` —
  walks the command tree, applies safety filters and first-match-wins
  selectors, builds per-tool JSON Schemas, returns rmcp-ready tools.
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

## Repository

https://github.com/tj-smith47/brontes

## License

MIT. See [LICENSE](LICENSE).
