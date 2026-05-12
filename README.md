# brontes

> *brontes* (Greek: thunder). In myth, the Cyclops smith who forged Zeus's thunderbolts. This crate forges clap CLIs into MCP servers.

**Transform any clap CLI into an MCP server.**

brontes converts your `clap` commands into MCP tools and ships subcommands for one-shot integration with Claude Desktop, VSCode, and Cursor. Rust port for [clap](https://github.com/clap-rs/clap), inspired by [njayp/ophis](https://github.com/njayp/ophis).

## Quick start

Add the dependency:

```toml
[dependencies]
brontes = "0.1"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

Attach the `mcp` subtree to your root command:

```rust
use clap::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cmd = Command::new("my-cli")
        .subcommand(Command::new("greet").about("Say hi"))
        .subcommand(brontes::command(None));

    brontes::dispatch(cmd).await
}
```

Enable in an editor:

```bash
# Claude Desktop
./my-cli mcp claude enable
# (restart Claude Desktop)

# VSCode (Copilot Agent Mode required)
./my-cli mcp vscode enable

# Cursor
./my-cli mcp cursor enable
```

Your CLI commands are now available as MCP tools.

## Stream over HTTP

```bash
./my-cli mcp stream --host localhost --port 8080
```

## Commands

`brontes::command(None)` attaches this subtree (default name `mcp`, configurable via `Config::command_name`):

```text
mcp
  start            Start MCP server on stdio
  stream           Stream MCP server over HTTP
  tools            Export available MCP tools as JSON
  claude
    enable         Add server to Claude Desktop config
    disable        Remove server from Claude Desktop config
    list           List Claude Desktop MCP servers
  vscode
    enable / disable / list
  cursor
    enable / disable / list
```

## Configuration

Control which commands and flags are exposed using selectors. By default, all commands and flags are exposed (hidden and deprecated entries are always filtered).

```rust
use brontes::{Config, Selector, selectors};

let config = Config {
    selectors: vec![Selector {
        cmd: Some(selectors::allow_cmds_containing(&["get", "list"])),
        local_flag: Some(selectors::exclude_flags(&["token", "secret"])),
        inherited_flag: Some(selectors::no_flags()),
        middleware: None,
    }],
    ..Default::default()
};

let cmd = Command::new("my-cli")
    .subcommand(brontes::command(Some(config)));
```

### Default environment variables

Editors launch MCP servers with a minimal environment. On macOS the inherited `PATH` is typically `/usr/bin:/bin:/usr/sbin:/sbin`, so tools installed via `mise`, `homebrew`, or `nix` are not visible. Capture the current shell's environment at `enable` time with `default_env`:

```rust
use std::env;

let config = Config {
    default_env: [
        ("PATH".into(), env::var("PATH").unwrap_or_default()),
    ].into_iter().collect(),
    ..Default::default()
};
```

These values are merged into the editor config written by `enable`. User-supplied `--env` values take precedence on conflict.

### Custom command name

If your CLI already exposes an `mcp` subcommand, rename brontes's command:

```rust
let config = Config { command_name: "agent".into(), ..Default::default() };
```

The whole command tree, editor-config writers, and built-in filters use the configured name.

### Tool annotations

Set MCP tool annotations on a clap subcommand via the helper:

```rust
use clap::Command;
use brontes::annotations::{Annotations, with_annotations};

let list_cmd = with_annotations(
    Command::new("list").about("List resources"),
    Annotations {
        title: Some("List Resources".into()),
        read_only: Some(true),
        ..Annotations::default()
    },
);
```

## How it works

1. **Command discovery.** brontes walks your `clap::Command` tree.
2. **Schema generation.** It derives JSON Schemas from `Arg` types, `value_parser`, and `ArgAction`. Required, default, and array-valued args are reflected in the schema.
3. **Tool execution.** Each MCP tool call spawns the current executable as a subprocess and captures stdout, stderr, and exit code.

## Examples

See [`examples/make-mcp/`](examples/make-mcp) for a working end-to-end example wrapping `make`.

## License

MIT. See [LICENSE](LICENSE).
