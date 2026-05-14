# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.x.x   | :white_check_mark: |

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Please report security issues privately via [GitHub Security Advisories](https://github.com/tj-smith47/brontes/security/advisories/new). Include:

- Description of the vulnerability
- Steps to reproduce
- Impact assessment
- Any suggested fix (optional)

### Response Timeline

- **48 hours** — Acknowledgment of receipt
- **7 days** — Initial assessment and severity rating
- **30-90 days** — Resolution, depending on complexity

## Threat Surface

brontes transforms a `clap` CLI into an MCP server. The following surfaces matter
for any consumer ("the consumer" = the CLI that mounts `brontes::command`):

- **Network-reachable MCP servers** — `mcp stream` runs an MCP server over
  streamable HTTP via [rmcp](https://github.com/modelcontextprotocol/rust-sdk).
  The primary control against DNS-rebinding and unintended LAN exposure is
  rmcp's host allow-list, which defaults to loopback only (`localhost`,
  `127.0.0.1`, `::1`); requests from any other `Host:` header receive a silent
  403. The `--allow-host` flag widens that list; operators who expose the
  server beyond loopback are responsible for fronting it with TLS and
  authentication. brontes ships no built-in TLS termination.

- **Editor config writes** — `mcp claude|cursor|vscode enable` modifies per-OS
  user-config files (Claude Desktop's per-OS user-config directory — macOS:
  `~/Library/Application Support/Claude/`; Linux: `$XDG_CONFIG_HOME/Claude/`
  or `~/.config/Claude/`; Windows: `%APPDATA%\Claude\` — Cursor's
  `~/.cursor/mcp.json`; VSCode's per-OS `Code/User/mcp.json`) or the
  per-workspace `$CWD/.cursor/mcp.json` / `$CWD/.vscode/mcp.json` variants;
  see `src/manager/paths.rs` for the full per-OS resolution. brontes performs
  **backup-before-write** — existing config is copied to `<base>.backup.json`
  before the primary file is replaced; a failed backup aborts the save and
  leaves the original file untouched. Consumers are responsible for not
  invoking these subcommands on shared or untrusted machines.

- **Child-process execution** — `brontes::handle` and downstream tool calls
  spawn the wrapped CLI binary as a child process. The child inherits the
  parent environment, merged with `Config::default_env` and any per-call env
  supplied by the MCP client. brontes does **not** sanitize env values; the
  caller is responsible for vetting `default_env` contents and any per-call
  inputs before they reach the child.

- **Middleware as the auth boundary** — `brontes::Middleware` is the
  documented hook for authentication, rate-limiting, and audit logging. brontes
  ships **no built-in auth** — any HTTP-exposed server without middleware is
  unauthenticated.

- **Tool-name namespace** — `Config::tool_name_prefix` exists to prevent tool
  collisions when multiple brontes-mounted CLIs attach to one MCP client.
  Without a prefix, a malicious or careless second tool with the same name can
  shadow a trusted one in the client's tool list.

## Best Practices

- **Always attach `Middleware`** before exposing an MCP server over HTTP.
- **Prefer stdio over HTTP** when the consumer runs locally — stdio inherits
  the host's process boundary and has no network surface.
- **Use `--allow-host` sparingly.** Default to localhost; widen only when you
  control the network path (and pair with TLS + auth at the edge).
- **Audit `Config::default_env` for secrets** before publishing a release —
  values flow into every child-process spawn.
- **Review `mcp tools` JSON output** as part of the release checklist — the
  emitted tool surface is what every connected client will see.
- **Set `Config::tool_name_prefix`** when shipping a brontes-mounted CLI that
  may run alongside other MCP servers in the same client.
