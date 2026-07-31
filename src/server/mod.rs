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
//! ([`crate::server::stdio`], [`crate::server::http`]) can drive it.

pub mod http;
pub mod stdio;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use clap::Command;
use futures::future::BoxFuture;
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ClientCapabilities,
    ContentBlock, CreateTaskResult, DiscoverResult, GetTaskParams, GetTaskResult, Implementation,
    InitializeResult, InputRequest, InputRequiredResult, InputResponses, JsonObject,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, RequestMetaObject,
    ServerCapabilities, ServerInfo, Tool, UpdateTaskParams,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::task_manager::{TaskContext, TaskExit, TaskManager, TaskOptions};
use tokio_util::sync::CancellationToken;

use crate::Config;
use crate::Result;
use crate::command::ResolvedTool;
use crate::config::TaskMode;
use crate::schema::FlagSpec;
use crate::selector::{BoxedNext, Middleware, MiddlewareCtx, MiddlewareOutcome, MiddlewareResult};
use crate::tool::{ToolInput, ToolOutput};
use crate::trace::TraceContext;

/// MCP server handler that exposes a walked clap tree as MCP tools.
///
/// Construct via [`BrontesServer::new`] and feed to
/// [`rmcp::ServiceExt::serve`] over a stdio (or future HTTP) transport.
///
/// Tool listing is computed once at construction time and cached: every
/// `tools/list` and `tools/call` request consults the cached
/// [`Vec<Tool>`](rmcp::model::Tool). The [`Config`] is immutable after
/// server construction, so the cache cannot go stale; a future
/// hot-reload feature would need to invalidate it.
///
/// Every field is shared rather than owned, so cloning a server is cheap and
/// every clone answers from the same tool cache and the same task store. The
/// streamable-HTTP transport takes a handler factory and calls it per session;
/// handing out clones keeps a task created by one request reachable from the
/// `tasks/get` that follows it, and keeps the clap walk a startup cost rather
/// than a per-request one.
///
/// Marked `#[doc(hidden)]` because consumers are expected to drive the
/// server through [`crate::handle`] / [`crate::run`]; the type is exposed
/// solely so the integration test suite can drive it over an in-memory
/// duplex transport.
#[doc(hidden)]
#[derive(Clone)]
pub struct BrontesServer {
    /// The user's full clap tree, cloned and `build()`-ed at construction
    /// time so global args are propagated before walking.
    cli: Arc<Command>,
    /// User-facing configuration: selectors, annotations, default env, etc.
    cfg: Arc<Config>,
    /// Resolved tool list (descriptor + claimed middleware + clap path),
    /// computed once at construction. See type-level docs.
    tools: Arc<Vec<ResolvedTool>>,
    /// Store and executor for detached commands (SEP-2663). Present
    /// unconditionally — it costs an empty map when no command is detached,
    /// and the `tasks` capability, not this field, is what tells clients the
    /// extension is live.
    tasks: TaskManager,
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
        let tools = crate::command::generate_tools_with_middleware(&cli, &cfg)?;
        Ok(Self {
            cli: Arc::new(cli),
            cfg: Arc::new(cfg),
            tools: Arc::new(tools),
            tasks: TaskManager::new(),
        })
    }

    /// Build the [`ServerInfo`] (a.k.a. [`InitializeResult`]) reported on
    /// MCP handshake.
    ///
    /// `Config.implementation` overrides the default identity (which derives
    /// from `CARGO_PKG_NAME` / `CARGO_PKG_VERSION` at build time of the
    /// brontes crate). Capability negotiation advertises `tools`, plus the
    /// `tasks` extension when some command is detached — brontes does not
    /// expose prompts, resources, or completions.
    fn build_server_info(&self) -> ServerInfo {
        let mut builder = ServerCapabilities::builder().enable_tools();
        if self.cfg.tasks_enabled() {
            builder = builder.enable_tasks();
        }
        let capabilities = builder.build();

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

    /// Look up a tool descriptor by its MCP name in the cached tool list.
    ///
    /// Returns the [`Tool`] half of the cached [`ResolvedTool`] entry so
    /// callers that only care about the descriptor (e.g. `get_tool` for
    /// rmcp's task-support routing) do not have to know about the internal
    /// middleware-cache shape.
    fn find_tool(&self, name: &str) -> Option<Tool> {
        self.find_resolved(name).map(|r| r.tool.clone())
    }

    /// Look up the full [`ResolvedTool`] (descriptor + claimed middleware +
    /// command path) by MCP name. Used by [`Self::call_tool`] to dispatch
    /// the middleware chain against the exec step.
    fn find_resolved(&self, name: &str) -> Option<&ResolvedTool> {
        self.tools.iter().find(|t| t.tool.name.as_ref() == name)
    }
}

impl ServerHandler for BrontesServer {
    fn get_info(&self) -> ServerInfo {
        self.build_server_info()
    }

    /// Answer `server/discover` (SEP-2575) with cache hints attached.
    ///
    /// rmcp's default implementation builds the right payload but leaves
    /// `ttlMs` at zero, which tells clients to re-discover on every
    /// connection. brontes' discovery response is as static as its tool
    /// list — supported versions, `tools`-only capabilities, and the host
    /// CLI's identity are all fixed at construction — so it carries the
    /// same hints as `tools/list`.
    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<DiscoverResult, McpError> {
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        )
        .with_ttl_ms(self.cfg.resolved_cache_ttl_ms())
        .with_cache_scope(self.cfg.resolved_cache_scope()))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, McpError> {
        // Project the cache down to the wire shape: clients receive
        // descriptors only, not the runtime-side middleware references.
        // Ordering is the walk's depth-first clap order and is stable across
        // calls — the 2026-07-28 spec asks servers to keep it that way so
        // clients (and LLM prompt caches) can cache the listing.
        let tools: Vec<Tool> = self.tools.iter().map(|r| r.tool.clone()).collect();
        // SEP-2549 cache hints: rmcp leaves both fields `None` unless the
        // handler fills them, and a client that sees no `ttlMs` will not
        // cache at all.
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(self.cfg.resolved_cache_ttl_ms())
            .with_cache_scope(self.cfg.resolved_cache_scope()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, McpError> {
        let name = request.name.as_ref();

        // Validate the tool exists in the current walked tree. The MCP
        // wrapper trait already calls `get_tool` for task-support routing,
        // but we want a clean per-call check at the exec boundary too.
        let Some(resolved) = self.find_resolved(name) else {
            return Err(McpError::invalid_params(
                format!("unknown tool: {name}"),
                None,
            ));
        };

        // Deserialize the client-supplied arguments into ToolInput. Default
        // to an empty payload when the client sends no arguments at all.
        let input: ToolInput = match request.arguments {
            Some(mut map) => {
                fold_promoted_flags(&mut map, &resolved.promoted_flags).map_err(|misplaced| {
                    McpError::invalid_params(
                        format!(
                            "flag(s) for {name} must be supplied as top-level arguments, not \
                             under `flags`: {misplaced}"
                        ),
                        None,
                    )
                })?;
                serde_json::from_value(serde_json::Value::Object(map)).map_err(|e| {
                    McpError::invalid_params(format!("invalid arguments for {name}: {e}"), None)
                })?
            }
            None => ToolInput::default(),
        };

        // The input schema advertises `additionalProperties: false` on `flags`,
        // and nothing in the MCP layer enforces it. Honoring it here is what
        // makes that advertisement true; forwarding an unknown flag instead
        // reaches the CLI as an opaque clap usage error.
        if let Some(unknown) = unknown_flags(&input, &resolved.flag_specs) {
            return Err(McpError::invalid_params(
                format!("unknown flag(s) for {name}: {unknown}"),
                None,
            ));
        }

        let trace_context = TraceContext::from_meta(&context.meta, name);
        let plan = CallPlan {
            tool_name: name.to_string(),
            command_path: resolved.command_path.clone(),
            flag_specs: Arc::new(resolved.flag_specs.clone()),
            env: Arc::new(resolve_call_env(&self.cfg, &trace_context)),
            middleware: resolved.middleware.clone(),
            input,
            trace_context,
            meta: context.meta.clone(),
            protocol_version: context.protocol_version(),
            client_capabilities: context.client_capabilities(),
        };

        // SEP-2663: hand the call back as a task when this command is
        // configured for one and the client declared the extension. A client
        // that did not declare it gets the blocking result no matter what the
        // config says — the handle is a shape it has no way to parse.
        let detached = self.cfg.resolved_task_mode(&plan.command_path) == TaskMode::Detached
            && plan
                .client_capabilities
                .as_ref()
                .is_some_and(rmcp::model::ClientCapabilities::supports_tasks);
        if detached {
            let options = TaskOptions::new()
                .with_ttl_ms(self.cfg.resolved_task_ttl_ms())
                .with_poll_interval_ms(self.cfg.resolved_task_poll_interval_ms())
                .with_status_message(format!("running {}", plan.command_path));
            let task = self.tasks.spawn(options, move |task_ctx| {
                Box::pin(run_detached(plan, task_ctx))
            });
            return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
        }

        let result = plan
            .run_once(
                context.ct.clone(),
                request.input_responses,
                request.request_state,
            )
            .await;

        Ok(outcome_to_response(
            result,
            &plan.tool_name,
            &plan.command_path,
            plan.protocol_version.as_ref(),
            plan.client_capabilities.as_ref(),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.find_tool(name)
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<GetTaskResult, McpError> {
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<(), McpError> {
        self.tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<(), McpError> {
        self.tasks.cancel_task(&request.task_id)
    }
}

/// Everything one `tools/call` needs to run its middleware chain, owned so the
/// chain can be re-entered without the originating request.
///
/// Re-entry is what a detached call needs: MCP has no resume operation, so an
/// input request is answered by running the chain again from the top, and for a
/// task that second run happens long after the `tools/call` that created it has
/// been answered.
struct CallPlan {
    tool_name: String,
    command_path: String,
    flag_specs: Arc<BTreeMap<String, FlagSpec>>,
    env: Arc<HashMap<String, String>>,
    middleware: Option<Middleware>,
    input: ToolInput,
    trace_context: TraceContext,
    meta: RequestMetaObject,
    protocol_version: Option<ProtocolVersion>,
    client_capabilities: Option<ClientCapabilities>,
}

impl CallPlan {
    /// Build the leaf of the chain: a one-shot async closure that spawns the
    /// subprocess via [`crate::exec::run_tool`]. Owned and `Arc`-shared
    /// captures keep the future `'static`.
    fn next(&self) -> BoxedNext {
        let tool_name = self.tool_name.clone();
        let flag_specs = Arc::clone(&self.flag_specs);
        let env = Arc::clone(&self.env);
        Box::new(
            move |ctx: MiddlewareCtx| -> BoxFuture<'static, MiddlewareResult> {
                Box::pin(async move {
                    crate::exec::run_tool(
                        &tool_name,
                        &ctx.input,
                        &flag_specs,
                        &env,
                        ctx.cancellation_token,
                    )
                    .await
                    .map(MiddlewareOutcome::Complete)
                })
            },
        )
    }

    /// Run the middleware chain once, with whatever the client has answered so
    /// far.
    ///
    /// A panic anywhere in the chain becomes [`crate::Error::Panic`] rather
    /// than unwinding: the chain always runs inside `tokio::spawn`, so the rmcp
    /// service task survives a middleware that panics.
    async fn run_once(
        &self,
        cancellation_token: CancellationToken,
        input_responses: Option<InputResponses>,
        request_state: Option<String>,
    ) -> MiddlewareResult {
        let ctx = MiddlewareCtx {
            cancellation_token,
            tool_name: self.tool_name.clone(),
            input: self.input.clone(),
            trace_context: self.trace_context.clone(),
            meta: self.meta.clone(),
            protocol_version: self.protocol_version.clone(),
            client_capabilities: self.client_capabilities.clone(),
            input_responses,
            request_state,
        };
        let next = self.next();

        // Always wrap the chain in `tokio::spawn` (whether or not middleware
        // is present) so a panic in either layer becomes a recoverable
        // `JoinError` rather than tearing down the rmcp service task.
        let join_handle = if let Some(mw) = self.middleware.clone() {
            tokio::spawn(async move { mw(ctx, next).await })
        } else {
            tokio::spawn(async move { next(ctx).await })
        };

        match join_handle.await {
            Ok(result) => result,
            Err(join_err) if join_err.is_panic() => {
                let payload = join_err.try_into_panic().ok().map_or_else(
                    || "unknown panic payload".to_string(),
                    |b| panic_message_from(&*b),
                );
                Err(crate::Error::Panic(payload))
            }
            Err(join_err) => Err(crate::Error::Panic(format!(
                "middleware/exec task join error: {join_err}"
            ))),
        }
    }
}

/// How many times a detached call may re-enter the middleware chain after an
/// input request before brontes calls it a loop.
///
/// A blocking call is bounded by the client, which decides whether to retry.
/// A detached call answers its own retries, so a middleware that asks for input
/// unconditionally would otherwise spin forever inside a task the client can
/// only watch.
const MAX_TASK_INPUT_ROUNDS: usize = 16;

/// Run a call as a SEP-2663 task: execute the chain, resolve any input request
/// through `tasks/update`, and settle with the command's result.
///
/// The middleware sees exactly what it sees in a blocking call — an input
/// request ends its run, and the answer arrives on a fresh run through
/// [`MiddlewareCtx::input_responses`] and [`MiddlewareCtx::request_state`].
/// Only the transport differs: the questions leave via `tasks/get` instead of
/// the `tools/call` response.
async fn run_detached(
    plan: CallPlan,
    task_ctx: TaskContext,
) -> std::result::Result<CallToolResult, TaskExit> {
    // The `tools/call` that created this task was answered the moment the
    // handle went out, taking its cancellation token with it. `tasks/cancel`
    // is the only channel left, so bridge it onto a token of our own —
    // without this the child process outlives a cancelled task.
    let cancellation_token = CancellationToken::new();
    let bridge = tokio::spawn({
        let task_ctx = task_ctx.clone();
        let cancellation_token = cancellation_token.clone();
        async move {
            task_ctx.cancelled().await;
            cancellation_token.cancel();
        }
    });

    let settled = detached_rounds(&plan, &task_ctx, cancellation_token).await;

    // The bridge parks on a watch channel the task manager keeps alive for as
    // long as it retains the task record — which, at the default unlimited
    // TTL, is the lifetime of the server.
    bridge.abort();
    settled
}

/// The retry loop behind [`run_detached`], split out so the cancellation bridge
/// is torn down on every exit path.
async fn detached_rounds(
    plan: &CallPlan,
    task_ctx: &TaskContext,
    cancellation_token: CancellationToken,
) -> std::result::Result<CallToolResult, TaskExit> {
    let mut input_responses: Option<InputResponses> = None;
    let mut request_state: Option<String> = None;

    for _ in 0..MAX_TASK_INPUT_ROUNDS {
        let outcome = plan
            .run_once(
                cancellation_token.clone(),
                input_responses.take(),
                request_state.take(),
            )
            .await;

        match outcome {
            // A command that ran to completion reports what it did, even if a
            // cancellation arrived while it was finishing: the side effects
            // already happened, and hiding them behind `cancelled` is the one
            // outcome the caller cannot recover from.
            Ok(MiddlewareOutcome::Complete(output)) => {
                return Ok(tool_output_to_result(&plan.tool_name, &output));
            }

            Ok(MiddlewareOutcome::InputRequired(input_required)) => {
                if let Some(reason) = reject_unsendable_input_request(
                    &input_required,
                    plan.protocol_version.as_ref(),
                    plan.client_capabilities.as_ref(),
                ) {
                    tracing::warn!(
                        target: "brontes::server",
                        tool = %plan.tool_name,
                        task = %task_ctx.task_id(),
                        %reason,
                        "dropping an input request the client cannot answer"
                    );
                    return Ok(tool_error_result(
                        &plan.tool_name,
                        &plan.command_path,
                        &crate::Error::Config(reason),
                    ));
                }

                let requests = input_required.input_requests.unwrap_or_default();
                task_ctx.set_status_message(format!(
                    "waiting for {} client response(s)",
                    requests.len()
                ));

                let mut answers = InputResponses::new();
                for (key, request) in requests {
                    let answer = match task_ctx.request_input(key.clone(), request).await {
                        Ok(answer) => answer,
                        // `tasks/cancel` clears the pending inputs out from
                        // under a task that is waiting on one.
                        Err(TaskExit::Cancelled) => return Err(TaskExit::Cancelled),
                        // Reusing a key the task already asked under — legal in
                        // a blocking call, where each retry is a fresh request,
                        // and rejected here because a task's keys are unique
                        // over its lifetime. Report it the way every other
                        // middleware mistake is reported rather than failing
                        // the task at the protocol level.
                        Err(TaskExit::Error(err)) => {
                            return Ok(tool_error_result(
                                &plan.tool_name,
                                &plan.command_path,
                                &crate::Error::Config(format!(
                                    "middleware could not ask the client for input under \
                                     key {key:?}: {}",
                                    err.message
                                )),
                            ));
                        }
                    };
                    answers.insert(key, answer);
                }

                task_ctx.set_status_message(format!("running {}", plan.command_path));
                input_responses = Some(answers);
                request_state = input_required.request_state;
            }

            Err(e) => {
                if task_ctx.is_cancel_requested() {
                    return Err(TaskExit::Cancelled);
                }
                return Ok(tool_error_result(&plan.tool_name, &plan.command_path, &e));
            }
        }
    }

    Ok(tool_error_result(
        &plan.tool_name,
        &plan.command_path,
        &crate::Error::Config(format!(
            "middleware asked the client for input {MAX_TASK_INPUT_ROUNDS} times without \
             completing; giving up rather than looping"
        )),
    ))
}

/// Turn what the middleware chain produced into the `tools/call` response.
///
/// Every path answers with a result rather than a JSON-RPC error, including a
/// middleware that asked for input the peer cannot supply: one misbehaving call
/// must not be able to fail the request at the protocol level.
fn outcome_to_response(
    result: MiddlewareResult,
    name: &str,
    command_path: &str,
    protocol_version: Option<&ProtocolVersion>,
    client_capabilities: Option<&ClientCapabilities>,
) -> CallToolResponse {
    match result {
        Ok(MiddlewareOutcome::Complete(output)) => tool_output_to_result(name, &output).into(),
        Ok(MiddlewareOutcome::InputRequired(input_required)) => input_required_to_response(
            input_required,
            name,
            command_path,
            protocol_version,
            client_capabilities,
        ),
        // Middleware-level or exec-level errors propagate as `tool_error` so a
        // single misbehaving call cannot kill the server. Spawn failures,
        // timeouts, cancellation, etc. all land here.
        Err(e) => tool_error_result(name, command_path, &e).into(),
    }
}

/// Send the middleware's input request, or degrade it to a tool error when the
/// peer could not act on it.
fn input_required_to_response(
    input_required: Box<InputRequiredResult>,
    name: &str,
    command_path: &str,
    protocol_version: Option<&ProtocolVersion>,
    client_capabilities: Option<&ClientCapabilities>,
) -> CallToolResponse {
    if let Some(reason) =
        reject_unsendable_input_request(&input_required, protocol_version, client_capabilities)
    {
        tracing::warn!(
            target: "brontes::server",
            tool = %name,
            %reason,
            "dropping an input request the client cannot answer"
        );
        return tool_error_result(name, command_path, &crate::Error::Config(reason)).into();
    }
    CallToolResponse::InputRequired(*input_required)
}

/// Explain why an [`InputRequiredResult`] must not reach this peer, or `None`
/// when it is safe to send.
///
/// Two conditions make the result unparseable at the other end, and rmcp guards
/// neither in a way brontes can live with:
///
/// - Below protocol `2026-07-28` the result type does not exist. rmcp turns the
///   attempt into a JSON-RPC `-32600`, which would break brontes' invariant that
///   a tool call always answers with a result rather than a protocol error.
/// - An elicitation request needs a client that declared the elicitation
///   capability. rmcp enforces this for the Tasks extension only, so an
///   elicitation would otherwise be sent to a client with no way to answer it,
///   hanging the call.
///
/// Sampling and roots requests are deliberately not capability-checked here:
/// both are deprecated as of `2026-07-28`, and a middleware that reaches for one
/// has made a decision brontes should not silently override.
fn reject_unsendable_input_request(
    input_required: &InputRequiredResult,
    protocol_version: Option<&ProtocolVersion>,
    client_capabilities: Option<&ClientCapabilities>,
) -> Option<String> {
    // Compared exactly the way rmcp compares it, so brontes' gate and rmcp's
    // can never disagree about a given peer. ISO `YYYY-MM-DD` versions order
    // lexically the same as chronologically.
    let mrtr_supported =
        protocol_version.is_some_and(|v| v.as_str() >= ProtocolVersion::V_2026_07_28.as_str());
    if !mrtr_supported {
        let reported = protocol_version.map_or("unknown", ProtocolVersion::as_str);
        return Some(format!(
            "middleware asked the client for input, which requires MCP protocol \
             2026-07-28 or newer; this client negotiated {reported}"
        ));
    }

    let wants_elicitation = input_required
        .input_requests
        .as_ref()
        .is_some_and(|requests| {
            requests
                .values()
                .any(|r| matches!(r, InputRequest::Elicitation(_)))
        });
    if wants_elicitation && client_capabilities.is_none_or(|caps| caps.elicitation.is_none()) {
        return Some(
            "middleware asked the client to elicit input, but the client did not \
             declare the elicitation capability"
                .to_owned(),
        );
    }

    None
}

/// Move SEP-2243-promoted values from the top level of a call's arguments back
/// into `flags`, so the rest of the pipeline never learns promotion happened.
///
/// A promoted flag is advertised beside `flags` because that is the only place
/// `x-mcp-header` is honored, but nothing downstream — [`ToolInput`], the
/// unknown-flag check, argv rendering — should have to know that. Folding here
/// keeps promotion a property of the wire shape alone.
///
/// Only promoted names move. Any other top-level key is left alone, to be
/// handled exactly as it was before promotion existed — this function is not
/// the place to change what brontes accepts, only where a promoted value lives.
///
/// Returns the misplaced names when the call supplies a promoted flag under
/// `flags` instead. Accepting it there would defeat the promotion silently: the
/// value would reach the command, no `Mcp-Param-*` header would accompany it,
/// and a call that sent both places would leave brontes guessing. The caller
/// turns this into an error that says where the value belongs.
fn fold_promoted_flags(
    arguments: &mut JsonObject,
    promoted: &BTreeSet<String>,
) -> std::result::Result<(), String> {
    if promoted.is_empty() {
        return Ok(());
    }

    if let Some(serde_json::Value::Object(flags)) = arguments.get("flags") {
        let misplaced: Vec<&str> = promoted
            .iter()
            .filter(|name| flags.contains_key(*name))
            .map(String::as_str)
            .collect();
        if !misplaced.is_empty() {
            return Err(misplaced.join(", "));
        }
    }

    let hoisted: Vec<(String, serde_json::Value)> = promoted
        .iter()
        .filter_map(|name| arguments.remove(name).map(|v| (name.clone(), v)))
        .collect();
    if hoisted.is_empty() {
        return Ok(());
    }

    let flags = arguments
        .entry("flags")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(flags) = flags {
        for (name, value) in hoisted {
            flags.insert(name, value);
        }
    }

    Ok(())
}

/// `_meta` key carrying the brontes error category on a failed tool call.
///
/// Reverse-DNS namespaced per the spec's rule for `_meta` extension keys, so it
/// cannot collide with a reserved key or another server's extension.
const META_KEY_ERROR_CATEGORY: &str = "io.github.tj-smith47.brontes/errorCategory";

/// `_meta` key carrying the space-joined clap path of the failed command.
const META_KEY_COMMAND_PATH: &str = "io.github.tj-smith47.brontes/commandPath";

/// Report the flag names a call supplied that the tool does not expose, as a
/// comma-separated list, or `None` when every name is known.
///
/// The names are sorted so the message is identical across calls — a client
/// diffing two failures should see a difference only when the input differed.
fn unknown_flags(input: &ToolInput, flag_specs: &BTreeMap<String, FlagSpec>) -> Option<String> {
    let mut unknown: Vec<&str> = input
        .flags
        .keys()
        .filter(|name| !flag_specs.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    if unknown.is_empty() {
        return None;
    }
    unknown.sort_unstable();
    Some(unknown.join(", "))
}

/// Build the environment for one tool call: [`Config::default_env`] overlaid
/// with the request's W3C Trace Context when propagation is enabled.
///
/// Per-request trace context wins over `default_env` on conflict. A statically
/// pinned `TRACEPARENT` would fold every call into a single span, so treating
/// it as the weaker value is what keeps traces correct.
fn resolve_call_env(cfg: &Config, trace_context: &TraceContext) -> HashMap<String, String> {
    let mut env = cfg.default_env.clone();
    if cfg.resolved_propagate_trace_context() {
        for (key, value) in trace_context.env_pairs() {
            env.insert(key.to_owned(), value.to_owned());
        }
    }
    env
}

/// Render a middleware-or-exec failure as a [`CallToolResult`] with
/// `is_error: true`. The error message is included in the text content; the
/// structured payload carries the brontes error category so clients can
/// distinguish (e.g.) a spawn failure from a panic.
///
/// `command_path` is the space-joined clap path for the underlying subcommand
/// (e.g. `"myapp greet"`). When non-empty it is appended to the human-readable
/// body so operators can immediately see which CLI command failed without
/// cross-referencing the tool name against the walked tree.
fn tool_error_result(name: &str, command_path: &str, e: &crate::Error) -> CallToolResult {
    let base = format!("tool '{name}' failed to execute: {e}");
    let body = if command_path.is_empty() {
        base
    } else {
        format!("{base} (command: \"{command_path}\")")
    };
    let mut r = CallToolResult::error(vec![ContentBlock::text(body.clone())]);
    // `structuredContent` must satisfy the tool's advertised `outputSchema`,
    // which is `ToolOutput` with all three fields required. A failure that
    // never produced a process outcome still has to answer in that shape, or a
    // schema-validating client rejects the result instead of reading the error.
    // `exit_code: -1` is the same "the OS reported no exit code" sentinel
    // `ToolOutput` already documents.
    // `ToolOutput` is three owned primitives, so serialization cannot fail.
    // Degrading to `null` rather than panicking keeps a hypothetical serde
    // change from taking the server down mid-call.
    r.structured_content = Some(
        serde_json::to_value(ToolOutput {
            stdout: String::new(),
            stderr: body,
            exit_code: -1,
        })
        .unwrap_or(serde_json::Value::Null),
    );
    // The brontes-specific detail moves to `_meta`, where the spec allows
    // namespaced extension keys, so a client can still tell a spawn failure
    // from a panic without the payload violating `outputSchema`.
    let mut meta = rmcp::model::MetaObject::new();
    meta.0.insert(
        META_KEY_ERROR_CATEGORY.to_owned(),
        serde_json::Value::String(brontes_error_category(e).to_owned()),
    );
    meta.0.insert(
        META_KEY_COMMAND_PATH.to_owned(),
        serde_json::Value::String(command_path.to_owned()),
    );
    r.meta = Some(meta);
    r
}

/// Short, stable string category for the [`crate::Error`] variant.
///
/// Used in the `structured_content` of a `tool_error` result so a client can
/// programmatically tell `Spawn` (subprocess could not be started) apart
/// from `Panic` (middleware/exec task panicked) without parsing
/// human-readable text.
const fn brontes_error_category(e: &crate::Error) -> &'static str {
    // `Error` is `#[non_exhaustive]` for downstream callers but exhaustive
    // inside the crate; any future variant added here MUST extend this
    // match (no wildcard arm by design).
    match e {
        crate::Error::Config(_) => "config",
        crate::Error::Io { .. } => "io",
        crate::Error::Spawn(_) => "spawn",
        crate::Error::Schema(_) => "schema",
        crate::Error::EditorConfigRead { .. }
        | crate::Error::EditorConfigJson { .. }
        | crate::Error::EditorConfigBackup { .. }
        | crate::Error::EditorConfigWrite { .. } => "editor_config",
        crate::Error::Panic(_) => "panic",
        crate::Error::McpInitialize(_) => "mcp_initialize",
        crate::Error::Mcp(_) => "mcp",
    }
}

/// Best-effort recovery of a panic payload's message.
///
/// `tokio::task::JoinError::try_into_panic` returns the `Box<dyn Any + Send>`
/// payload that the panicking task carried. Standard panic macros stash a
/// `&'static str` or `String` there; we downcast in that order, falling back
/// to a generic label so the propagated [`crate::Error::Panic`] always
/// carries *something* useful in its `Display` impl.
fn panic_message_from(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic payload".to_string()
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
        let mut r = CallToolResult::success(vec![ContentBlock::text(body)]);
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
        let mut r = CallToolResult::error(vec![ContentBlock::text(body)]);
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

    /// Request `_meta` carrying the W3C specification's own example
    /// traceparent plus vendor state and baggage.
    fn traced_meta() -> rmcp::model::RequestMetaObject {
        let mut meta = rmcp::model::RequestMetaObject::new();
        meta.0
            .set_traceparent("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01");
        meta.0.set_tracestate("vendor=state");
        meta.0.set_baggage("tenant=acme");
        meta
    }

    #[test]
    fn call_env_lowers_trace_context_onto_the_w3c_variables() {
        let trace = TraceContext::from_meta(&traced_meta(), "tool");
        let env = resolve_call_env(&Config::default(), &trace);

        assert_eq!(
            env.get("TRACEPARENT").map(String::as_str),
            Some("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01")
        );
        assert_eq!(
            env.get("TRACESTATE").map(String::as_str),
            Some("vendor=state")
        );
        assert_eq!(env.get("BAGGAGE").map(String::as_str), Some("tenant=acme"));
    }

    #[test]
    fn call_env_is_only_default_env_when_the_request_carries_no_trace() {
        let cfg = Config::default().default_env("EXISTING", "kept");
        let env = resolve_call_env(&cfg, &TraceContext::default());

        assert_eq!(env.get("EXISTING").map(String::as_str), Some("kept"));
        assert!(
            !env.contains_key("TRACEPARENT"),
            "an untraced request must not fabricate a traceparent"
        );
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn per_request_trace_context_wins_over_a_pinned_default_env() {
        let cfg = Config::default().default_env("TRACEPARENT", "pinned-and-wrong");
        let trace = TraceContext::from_meta(&traced_meta(), "tool");
        let env = resolve_call_env(&cfg, &trace);

        assert_eq!(
            env.get("TRACEPARENT").map(String::as_str),
            Some("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01"),
            "a pinned TRACEPARENT must not fold every call into one span"
        );
    }

    #[test]
    fn propagation_off_leaves_default_env_untouched() {
        let cfg = Config::default()
            .propagate_trace_context(false)
            .default_env("TRACEPARENT", "pinned");
        let trace = TraceContext::from_meta(&traced_meta(), "tool");
        let env = resolve_call_env(&cfg, &trace);

        assert_eq!(
            env.get("TRACEPARENT").map(String::as_str),
            Some("pinned"),
            "with propagation off the request must not touch the environment"
        );
        assert!(!env.contains_key("TRACESTATE"));
        assert!(!env.contains_key("BAGGAGE"));
    }

    /// An `InputRequiredResult` carrying one elicitation request.
    fn elicitation_required() -> InputRequiredResult {
        let mut requests = rmcp::model::InputRequests::new();
        requests.insert(
            "confirm".to_owned(),
            InputRequest::Elicitation(rmcp::model::ElicitRequest::new(
                rmcp::model::ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: "Proceed?".to_owned(),
                    requested_schema: rmcp::model::ElicitationSchema::new(
                        std::collections::BTreeMap::new(),
                    ),
                },
            )),
        );
        InputRequiredResult::new(Some(requests), Some("opaque-state".to_owned()))
    }

    /// An `InputRequiredResult` with no requests at all — a bare state carrier.
    fn state_only_required() -> InputRequiredResult {
        InputRequiredResult::from_request_state("opaque-state")
    }

    fn elicitation_capable() -> ClientCapabilities {
        ClientCapabilities::builder().enable_elicitation().build()
    }

    #[test]
    fn input_request_reaches_a_capable_2026_peer() {
        assert!(
            reject_unsendable_input_request(
                &elicitation_required(),
                Some(&ProtocolVersion::V_2026_07_28),
                Some(&elicitation_capable()),
            )
            .is_none()
        );
    }

    #[test]
    fn input_request_is_refused_below_the_2026_revision() {
        // rmcp would turn this into a JSON-RPC -32600, which breaks brontes'
        // invariant that a tool call always answers with a result.
        let reason = reject_unsendable_input_request(
            &elicitation_required(),
            Some(&ProtocolVersion::V_2025_11_25),
            Some(&elicitation_capable()),
        )
        .expect("must be refused");
        assert!(reason.contains("2026-07-28"), "{reason}");
        assert!(
            reason.contains("2025-11-25"),
            "must name the peer: {reason}"
        );
    }

    #[test]
    fn input_request_is_refused_when_the_peer_version_is_unknown() {
        let reason = reject_unsendable_input_request(
            &elicitation_required(),
            None,
            Some(&elicitation_capable()),
        )
        .expect("must be refused");
        assert!(reason.contains("unknown"), "{reason}");
    }

    #[test]
    fn elicitation_is_refused_when_the_client_never_declared_it() {
        // rmcp capability-checks the Tasks extension but not MRTR, so an
        // elicitation would otherwise reach a client with no way to answer and
        // hang the call.
        for caps in [None, Some(&ClientCapabilities::default())] {
            let reason = reject_unsendable_input_request(
                &elicitation_required(),
                Some(&ProtocolVersion::V_2026_07_28),
                caps,
            )
            .expect("must be refused");
            assert!(reason.contains("elicitation capability"), "{reason}");
        }
    }

    #[test]
    fn a_state_only_input_request_needs_no_elicitation_capability() {
        // Nothing is being asked of the client beyond echoing state back, so
        // the capability check must not fire.
        assert!(
            reject_unsendable_input_request(
                &state_only_required(),
                Some(&ProtocolVersion::V_2026_07_28),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn a_refused_input_request_becomes_a_tool_error_not_a_protocol_error() {
        let response = outcome_to_response(
            Ok(crate::selector::MiddlewareOutcome::InputRequired(Box::new(
                elicitation_required(),
            ))),
            "myapp_greet",
            "myapp greet",
            Some(&ProtocolVersion::V_2025_11_25),
            Some(&elicitation_capable()),
        );
        let CallToolResponse::Complete(result) = response else {
            panic!("a refused input request must degrade to a completed error result");
        };
        assert_eq!(result.is_error, Some(true));
        // And it must still satisfy the advertised outputSchema.
        let sc = result.structured_content.expect("structured_content");
        let parsed: ToolOutput = serde_json::from_value(sc).expect("conforms to ToolOutput");
        assert_eq!(parsed.exit_code, -1);
        assert!(parsed.stderr.contains("2026-07-28"), "{}", parsed.stderr);
    }

    #[test]
    fn an_accepted_input_request_passes_through_unchanged() {
        let response = outcome_to_response(
            Ok(crate::selector::MiddlewareOutcome::InputRequired(Box::new(
                elicitation_required(),
            ))),
            "myapp_greet",
            "myapp greet",
            Some(&ProtocolVersion::V_2026_07_28),
            Some(&elicitation_capable()),
        );
        let CallToolResponse::InputRequired(result) = response else {
            panic!("a sendable input request must not be degraded");
        };
        assert_eq!(result.request_state.as_deref(), Some("opaque-state"));
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

    #[test]
    fn tool_error_result_includes_command_path_in_body() {
        let e = crate::Error::Panic("test panic".to_string());
        let result = tool_error_result("myapp_greet", "myapp greet", &e);

        assert_eq!(result.is_error, Some(true));

        // The human-readable text body must contain the command path.
        let body = result
            .content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            body.contains("command: \"myapp greet\""),
            "body must include command path; got: {body:?}"
        );

        // The command path moved to `_meta`: `structuredContent` must stay
        // inside the advertised `outputSchema`, which has no `command` field.
        let meta = result.meta.as_ref().expect("_meta must be Some");
        assert_eq!(
            meta.0[META_KEY_COMMAND_PATH].as_str(),
            Some("myapp greet"),
            "_meta must carry the command path"
        );
        assert_eq!(
            meta.0[META_KEY_ERROR_CATEGORY].as_str(),
            Some("panic"),
            "_meta must carry the error category"
        );
    }

    #[test]
    fn error_results_satisfy_the_advertised_output_schema() {
        // rmcp does not validate `structuredContent` server-side, so a payload
        // that misses a required key fails only at a validating client — the
        // quietest possible failure. Assert the contract here instead.
        let required: Vec<String> = crate::schema::build_output_schema()["required"]
            .as_array()
            .expect("outputSchema.required")
            .iter()
            .map(|v| v.as_str().expect("required entries are strings").to_owned())
            .collect();
        assert_eq!(required, ["stdout", "stderr", "exit_code"]);

        // Every Error variant travels this path: spawn failure, panic,
        // cancellation, config, IO.
        let errors = [
            crate::Error::Panic("boom".to_string()),
            crate::Error::Config("bad".to_string()),
            crate::Error::Spawn(std::io::Error::other("nope")),
        ];
        for e in &errors {
            let result = tool_error_result("myapp_greet", "myapp greet", e);
            let sc = result
                .structured_content
                .as_ref()
                .expect("structured_content must be Some");
            for key in &required {
                assert!(
                    sc.get(key).is_some(),
                    "structuredContent for {e:?} must carry the required key {key:?}: {sc}"
                );
            }
            assert_eq!(sc["exit_code"], -1, "no process outcome uses the sentinel");
            assert!(
                sc["stderr"]
                    .as_str()
                    .is_some_and(|s| s.contains("myapp greet")),
                "the failure detail must survive in stderr: {sc}"
            );
            // It must also round-trip as the type the schema describes.
            let parsed: ToolOutput =
                serde_json::from_value(sc.clone()).expect("must deserialize as ToolOutput");
            assert_eq!(parsed.exit_code, -1);
        }
    }

    #[test]
    fn tool_error_result_empty_command_path_omits_parenthetical() {
        let e = crate::Error::Panic("boom".to_string());
        let result = tool_error_result("myapp_greet", "", &e);
        assert_eq!(result.is_error, Some(true));
        let body = result
            .content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            !body.contains("command:"),
            "empty command_path must not add the parenthetical; got: {body:?}"
        );
    }
}
