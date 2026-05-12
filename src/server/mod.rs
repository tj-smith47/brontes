//! [`BrontesServer`]: the [`rmcp::ServerHandler`] implementation that turns
//! a walked [`clap::Command`] tree into a running MCP server.
//!
//! `BrontesServer` is the runtime counterpart to [`crate::generate_tools`].
//! Where `generate_tools` builds a static [`Vec<Tool>`](rmcp::model::Tool)
//! for offline inspection, `BrontesServer` registers as an MCP handler so
//! it can both list those tools to a connected client AND execute them by
//! spawning the user's binary as a subprocess.
//!
//! Consumers do not construct `BrontesServer` directly in normal use —
//! [`crate::handle`] / [`crate::run`] wrap it. The type is exposed only
//! within the crate so the transport-specific subcommand modules
//! ([`crate::server::stdio`]) can drive it.

pub(crate) mod stdio;

use std::collections::HashMap;
use std::sync::Arc;

use clap::Command;
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};

use crate::Config;
use crate::Result;
use crate::tool::{ToolInput, ToolOutput};

/// MCP server handler that exposes a walked clap tree as MCP tools.
///
/// Construct via [`BrontesServer::new`] and feed to
/// [`rmcp::ServiceExt::serve`] over a stdio (or future HTTP) transport.
///
/// Tool listing is computed once at construction time and cached: every
/// `tools/list` and `tools/call` request consults the cached
/// [`Vec<Tool>`](rmcp::model::Tool). For v0.1.0 the [`Config`] is immutable
/// after server construction, so the cache cannot go stale; a future
/// hot-reload feature would need to invalidate it.
///
/// Marked `#[doc(hidden)]` because consumers are expected to drive the
/// server through [`crate::handle`] / [`crate::run`]; the type is exposed
/// solely so the integration test suite can drive it over an in-memory
/// duplex transport.
#[doc(hidden)]
pub struct BrontesServer {
    /// The user's full clap tree, cloned and `build()`-ed at construction
    /// time so global args are propagated before walking.
    cli: Command,
    /// User-facing configuration: selectors, annotations, default env, etc.
    cfg: Arc<Config>,
    /// Tool list, computed once at construction. See type-level docs.
    tools: Vec<Tool>,
}

impl BrontesServer {
    /// Build a new [`BrontesServer`] over the given clap tree and config.
    ///
    /// The clap command is `build()`-ed eagerly so subsequent tool-listing
    /// calls see a stable shape (global args propagated, defaults resolved).
    ///
    /// Returns [`crate::Error::Config`] / [`crate::Error::Schema`] if the
    /// pre-walk surfaces a bad config; this matches the existing
    /// [`crate::server::stdio::serve_stdio`] pre-walk warning pass while
    /// also seeding the per-server tool cache.
    ///
    /// # Errors
    ///
    /// Any error surfaced by [`crate::generate_tools`] (bad config, bad
    /// schema).
    #[doc(hidden)]
    pub fn new(mut cli: Command, cfg: Config) -> Result<Self> {
        cli.build();
        let tools = crate::generate_tools(&cli, &cfg)?;
        Ok(Self {
            cli,
            cfg: Arc::new(cfg),
            tools,
        })
    }

    /// Build the [`ServerInfo`] (a.k.a. [`InitializeResult`]) reported on
    /// MCP handshake.
    ///
    /// `Config.implementation` overrides the default identity (which derives
    /// from `CARGO_PKG_NAME` / `CARGO_PKG_VERSION` at build time of the
    /// brontes crate). Capability negotiation advertises `tools` only —
    /// brontes does not (yet) expose prompts, resources, or completions.
    fn build_server_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().build();

        let server_info = self.cfg.implementation.clone().unwrap_or_else(|| {
            Implementation::new(
                self.cli.get_name().to_string(),
                self.cli
                    .get_version()
                    .map_or_else(|| "0.0.0".to_string(), str::to_string),
            )
        });

        InitializeResult::new(capabilities).with_server_info(server_info)
    }

    /// Look up a tool by its MCP name in the cached tool list.
    ///
    /// Called by [`Self::call_tool`] to validate the request before
    /// dispatching to [`crate::exec`].
    fn find_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name.as_ref() == name).cloned()
    }
}

impl ServerHandler for BrontesServer {
    fn get_info(&self) -> ServerInfo {
        self.build_server_info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let name = request.name.as_ref();

        // Validate the tool exists in the current walked tree. The MCP
        // wrapper trait already calls `get_tool` for task-support routing,
        // but we want a clean per-call check at the exec boundary too.
        if self.find_tool(name).is_none() {
            return Err(McpError::invalid_params(
                format!("unknown tool: {name}"),
                None,
            ));
        }

        // Deserialize the client-supplied arguments into ToolInput. Default
        // to an empty payload when the client sends no arguments at all.
        let input: ToolInput = match request.arguments {
            Some(map) => serde_json::from_value(serde_json::Value::Object(map)).map_err(|e| {
                McpError::invalid_params(format!("invalid arguments for {name}: {e}"), None)
            })?,
            None => ToolInput::default(),
        };

        // Merge default_env with any tool-call-specific env overrides.
        // Per-call overrides win on conflict (none are exposed in this
        // task; Task #2 wires middleware-supplied overrides).
        let env: HashMap<String, String> = self.cfg.default_env.clone();

        match crate::exec::run_tool(name, &input, &env, context.ct.clone()).await {
            Ok(output) => Ok(tool_output_to_result(name, &output)),
            Err(e) => Err(McpError::internal_error(
                format!("tool '{name}' failed to execute: {e}"),
                None,
            )),
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.find_tool(name)
    }
}

/// Render a [`ToolOutput`] (captured stdout/stderr/exit code) as the MCP
/// [`CallToolResult`] handed back to the client.
///
/// A zero exit code is a successful result whose body is the captured
/// stdout. A non-zero exit code is reported as an error result whose body
/// concatenates stdout and stderr; the structured payload retains the full
/// triple so the client can inspect machine-readable details.
fn tool_output_to_result(tool_name: &str, output: &ToolOutput) -> CallToolResult {
    let structured = serde_json::to_value(output).unwrap_or_else(|_| {
        serde_json::json!({
            "stdout": output.stdout,
            "stderr": output.stderr,
            "exit_code": output.exit_code,
        })
    });

    if output.exit_code == 0 {
        let body = if output.stdout.is_empty() && !output.stderr.is_empty() {
            output.stderr.clone()
        } else {
            output.stdout.clone()
        };
        let mut r = CallToolResult::success(vec![Content::text(body)]);
        r.structured_content = Some(structured);
        r
    } else {
        let mut body = String::new();
        if !output.stdout.is_empty() {
            body.push_str(&output.stdout);
        }
        if !output.stderr.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&output.stderr);
        }
        if body.is_empty() {
            body = format!("tool '{tool_name}' exited with code {}", output.exit_code);
        }
        let mut r = CallToolResult::error(vec![Content::text(body)]);
        r.structured_content = Some(structured);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Command {
        Command::new("myapp")
            .version("1.2.3")
            .subcommand(Command::new("greet").about("Say hi"))
    }

    #[test]
    fn server_info_uses_root_name_and_version_by_default() {
        let s = BrontesServer::new(root(), Config::default()).expect("construct");
        let info = s.build_server_info();
        assert_eq!(info.server_info.name, "myapp");
        assert_eq!(info.server_info.version, "1.2.3");
        assert!(info.capabilities.tools.is_some());
    }

    #[test]
    fn server_info_respects_config_implementation() {
        let imp = Implementation::new("custom-name", "9.9.9");
        let cfg = Config::default().implementation(imp);
        let s = BrontesServer::new(root(), cfg).expect("construct");
        let info = s.build_server_info();
        assert_eq!(info.server_info.name, "custom-name");
        assert_eq!(info.server_info.version, "9.9.9");
    }

    #[test]
    fn find_tool_locates_walked_command() {
        let s = BrontesServer::new(root(), Config::default()).expect("construct");
        assert!(s.find_tool("myapp_greet").is_some());
        assert!(s.find_tool("nonexistent").is_none());
    }

    #[test]
    fn tools_cached_at_construction() {
        // Cache invariance: after construction, mutating the held cli or cfg
        // cannot be observed (we just count that tools is exactly what
        // generate_tools produced once).
        let s = BrontesServer::new(root(), Config::default()).expect("construct");
        // Same number of tools is reported every time find_tool runs.
        let n1 = s.tools.len();
        let _ = s.find_tool("myapp_greet");
        let n2 = s.tools.len();
        assert_eq!(n1, n2);
        assert!(n1 >= 1, "at least one tool from the walked tree");
    }

    #[test]
    fn tool_output_zero_exit_is_success() {
        let out = ToolOutput {
            stdout: "hi\n".into(),
            stderr: String::new(),
            exit_code: 0,
        };
        let result = tool_output_to_result("myapp_greet", &out);
        assert_eq!(result.is_error, Some(false));
        assert!(result.structured_content.is_some());
    }

    #[test]
    fn tool_output_non_zero_is_error() {
        let out = ToolOutput {
            stdout: String::new(),
            stderr: "boom\n".into(),
            exit_code: 2,
        };
        let result = tool_output_to_result("myapp_greet", &out);
        assert_eq!(result.is_error, Some(true));
    }
}
