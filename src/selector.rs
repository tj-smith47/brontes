//! Selector types for command/flag filtering and middleware composition.
//!
//! A [`Selector`] is a filtering rule that decides:
//!
//! - **Which commands** become MCP tools (via [`CmdMatcher`])
//! - **Which flags** those tools expose (via [`FlagMatcher`] for local and
//!   inherited flags separately)
//! - **How tool calls are wrapped** (via optional [`Middleware`])
//!
//! Selectors are evaluated in configuration order by the tool-generation
//! layer. The first selector whose `cmd` matcher accepts a command claims
//! that command; commands not claimed by any selector are excluded from
//! the tool list.
//!
//! # Typical usage
//!
//! ```rust
//! use std::sync::Arc;
//! use brontes::{Selector, CmdMatcher};
//!
//! // Include only commands under the "deploy" subtree.
//! let matcher: CmdMatcher = Arc::new(|path: &str| path.starts_with("my-cli deploy"));
//!
//! let sel = Selector {
//!     cmd: Some(matcher),
//!     ..Default::default()
//! };
//! ```

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use rmcp::model::{
    ClientCapabilities, InputRequiredResult, InputResponses, ProtocolVersion, RequestMetaObject,
};

use crate::{
    Result,
    tool::{ToolInput, ToolOutput},
    trace::TraceContext,
};

/// Match a command by its space-joined path (e.g., `"my-cli sub leaf"`).
///
/// When placed in [`Selector::cmd`], the matcher is called with the
/// space-joined path of each candidate command. Return `true` to claim the
/// command for this selector.
///
/// The path always starts at the CLI's own name. Everywhere else a command
/// path is *written* — a [`Config`](crate::Config) key, a group member, a
/// `--command` flag — the leading name may be omitted, because brontes can
/// fill it in. A matcher is a closure, so there is nothing to fill in: it
/// sees the resolved path and must match against that.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use brontes::CmdMatcher;
///
/// let m: CmdMatcher = Arc::new(|path: &str| path.starts_with("my-cli deploy"));
/// assert!(m("my-cli deploy prod"));
/// assert!(!m("my-cli rollback"));
/// ```
pub type CmdMatcher = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Match a flag by inspecting its [`clap::Arg`] descriptor.
///
/// Placed in [`Selector::local_flag`] or [`Selector::inherited_flag`], the
/// matcher is called for each flag on a claimed command. Return `true` to
/// include the flag in the generated tool schema, `false` to omit it.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use brontes::FlagMatcher;
///
/// // Expose only the `--verbose` flag.
/// let m: FlagMatcher = Arc::new(|arg: &clap::Arg| {
///     arg.get_id().as_str() == "verbose"
/// });
/// ```
pub type FlagMatcher = Arc<dyn Fn(&clap::Arg) -> bool + Send + Sync>;

/// Per-call context handed to [`Middleware`].
///
/// `MiddlewareCtx` carries everything a middleware implementation needs:
/// a cancellation token that fires when the MCP client cancels the request,
/// the name of the tool being invoked, and the deserialized [`ToolInput`].
///
/// Middleware may clone the context before forwarding it via `next(ctx).await`.
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use brontes::{BoxedNext, Middleware, MiddlewareCtx};
///
/// // Middleware receives a `MiddlewareCtx` from brontes — it does not
/// // construct one itself.
/// let mw: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
///     Box::pin(async move {
///         let tool = ctx.tool_name.clone();
///         let result = next(ctx).await;
///         tracing::debug!(%tool, "after call");
///         result
///     })
/// });
/// # let _ = mw;
/// ```
///
/// # Forward compatibility
///
/// `MiddlewareCtx` is `#[non_exhaustive]`. Downstream code receives a
/// `MiddlewareCtx` value from brontes (as the first argument to a
/// [`Middleware`] closure); it does not construct one directly. Additional
/// per-call fields (request id, parameters, etc.) may be added in minor
/// releases without bumping the major version.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MiddlewareCtx {
    /// Fires when the MCP client cancels the in-flight request.
    pub cancellation_token: CancellationToken,
    /// The MCP tool name on which the call dispatched.
    pub tool_name: String,
    /// Deserialized input for the tool invocation.
    pub input: ToolInput,
    /// W3C Trace Context the client attached to this request's `_meta`
    /// (SEP-414), after validation.
    ///
    /// Populated regardless of
    /// [`Config::propagate_trace_context`](crate::Config::propagate_trace_context):
    /// that setting governs only whether brontes lowers the values onto the
    /// spawned CLI's environment, never whether middleware can read them.
    pub trace_context: TraceContext,

    /// The request's `_meta` map, verbatim.
    ///
    /// Carries the progress token (`RequestMetaObject::get_progress_token`),
    /// the client's identity and capabilities under the `2026-07-28` stateless
    /// revision, and any extension keys the client set. [`MiddlewareCtx`]
    /// surfaces the resolved forms of the fields brontes itself needs, but the
    /// raw map is here so a middleware is never blocked on brontes adding an
    /// accessor.
    pub meta: RequestMetaObject,

    /// The protocol revision in force for this request, when known.
    ///
    /// Resolved through the same path rmcp uses: the request's `_meta` under the
    /// stateless revision, falling back to the handshake for older peers.
    /// Returning [`MiddlewareOutcome::InputRequired`] requires `2026-07-28` or
    /// newer.
    pub protocol_version: Option<ProtocolVersion>,

    /// What the calling client declared it can do, when known.
    ///
    /// Check [`ClientCapabilities::elicitation`] before returning an
    /// [`InputRequest::Elicitation`](rmcp::model::InputRequest) — a client that
    /// never declared it cannot answer, and brontes turns the attempt into a
    /// tool error rather than letting it reach the wire.
    pub client_capabilities: Option<ClientCapabilities>,

    /// The client's answers to a previous [`MiddlewareOutcome::InputRequired`],
    /// present only on an MRTR retry (SEP-2322).
    ///
    /// Keys match the ones the middleware chose when it built the
    /// [`InputRequests`](rmcp::model::InputRequests) map. `None` means this is
    /// a first attempt.
    pub input_responses: Option<InputResponses>,

    /// The opaque `requestState` echoed back by the client on an MRTR retry.
    ///
    /// # This value is untrusted
    ///
    /// The client returns whatever it likes here. Per SEP-2322 a server that
    /// lets `requestState` influence authorization or resource access MUST
    /// verify its integrity first — seal it with `rmcp`'s `RequestStateCodec`
    /// (its `request-state` feature) or keep the real state server-side and
    /// treat this only as a lookup handle.
    pub request_state: Option<String>,
}

/// What a middleware chain produced for one tool call.
///
/// The exec step only ever produces [`MiddlewareOutcome::Complete`]; a
/// subprocess has no way to ask the client anything. A middleware may instead
/// return [`MiddlewareOutcome::InputRequired`] to suspend the call and ask the
/// client for input first (SEP-2322) — under the stateless `2026-07-28`
/// revision that is the only channel a server has for reaching its client.
///
/// # Retries re-enter the chain from the top
///
/// MCP has no "resume" operation: the client answers the input requests and
/// **re-sends the whole `tools/call`**, with
/// [`MiddlewareCtx::input_responses`] and [`MiddlewareCtx::request_state`]
/// populated. A middleware that has already run the exec step before asking
/// for input will therefore run it again on the retry. That is legitimate for a
/// read-only or dry-run command and wrong for a destructive one, so brontes
/// leaves the choice with the middleware rather than guessing: branch on
/// `ctx.input_responses.is_some()` to tell a retry from a first attempt.
///
/// # Example: confirm before a destructive command
///
/// ```
/// use std::collections::BTreeMap;
/// use std::sync::Arc;
///
/// use brontes::{
///     BoxedNext, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
///     ElicitationSchema, InputRequest, InputRequiredResult, InputRequests, Middleware,
///     MiddlewareCtx, MiddlewareOutcome, ToolOutput,
/// };
///
/// const CONFIRM: &str = "confirm";
///
/// let middleware: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
///     Box::pin(async move {
///         if !ctx.tool_name.ends_with("_destroy") {
///             return next(ctx).await;
///         }
///
///         // A retry is a fresh call carrying the answers, not a resumption:
///         // ask on the first attempt, act only once an answer is in.
///         let Some(answers) = ctx.input_responses.as_ref() else {
///             let mut requests = InputRequests::new();
///             requests.insert(
///                 CONFIRM.to_owned(),
///                 InputRequest::Elicitation(ElicitRequest::new(
///                     ElicitRequestParams::FormElicitationParams {
///                         meta: None,
///                         message: "Really destroy the environment?".to_owned(),
///                         requested_schema: ElicitationSchema::new(BTreeMap::new()),
///                     },
///                 )),
///             );
///             return Ok(InputRequiredResult::new(Some(requests), None).into());
///         };
///
///         let accepted = answers
///             .get(CONFIRM)
///             .cloned()
///             .and_then(|v| serde_json::from_value::<ElicitResult>(v).ok())
///             .is_some_and(|r| r.action == ElicitationAction::Accept);
///
///         if accepted {
///             next(ctx).await
///         } else {
///             Ok(MiddlewareOutcome::Complete(ToolOutput {
///                 stdout: String::new(),
///                 stderr: "aborted at the user's request\n".to_owned(),
///                 exit_code: 1,
///             }))
///         }
///     })
/// });
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum MiddlewareOutcome {
    /// The tool call finished; the payload is the process outcome.
    Complete(ToolOutput),

    /// The call cannot finish until the client fulfills the carried input
    /// requests and retries (SEP-2322).
    ///
    /// Reaching the wire requires a peer on protocol `2026-07-28` or newer, and
    /// an elicitation request requires the client to have declared the
    /// elicitation capability. brontes checks both and converts a violation
    /// into a tool error, so a middleware cannot accidentally emit a result the
    /// peer is unable to parse.
    InputRequired(Box<InputRequiredResult>),
}

impl From<ToolOutput> for MiddlewareOutcome {
    fn from(output: ToolOutput) -> Self {
        Self::Complete(output)
    }
}

impl From<InputRequiredResult> for MiddlewareOutcome {
    fn from(result: InputRequiredResult) -> Self {
        Self::InputRequired(Box::new(result))
    }
}

/// What a middleware (and the underlying exec step) ultimately produces.
///
/// Both the success and error paths of a tool invocation flow through this
/// type. On success the [`MiddlewareOutcome`] carries either the process
/// outcome or an MRTR input request. On error the crate-level [`crate::Error`]
/// is returned.
pub type MiddlewareResult = Result<MiddlewareOutcome>;

/// One-shot async callable that runs the wrapped exec step.
///
/// Middleware implementations call `next(ctx).await` to delegate to the
/// next layer (or ultimately to the exec step itself). Because `BoxedNext`
/// is `FnOnce`, calling it twice would be a compile error — each call chain
/// gets exactly one delegation.
///
/// # Example
///
/// ```rust,no_run
/// use brontes::{BoxedNext, MiddlewareCtx};
///
/// async fn my_middleware(ctx: MiddlewareCtx, next: BoxedNext) {
///     let result = next(ctx).await;
///     // inspect result ...
/// }
/// ```
pub type BoxedNext = Box<dyn FnOnce(MiddlewareCtx) -> BoxFuture<'static, MiddlewareResult> + Send>;

/// Wrap tool-call execution with custom async logic.
///
/// A `Middleware` is an `Arc`-wrapped async closure of the form
/// `|(ctx, next)| async { ... }`. It receives a [`MiddlewareCtx`] and a
/// [`BoxedNext`]; calling `next(ctx).await` delegates to the wrapped exec
/// step (or the next middleware in a chain).
///
/// Because `Middleware` is held inside [`Selector::middleware`] behind an
/// `Arc`, a single instance can be shared across concurrent async tasks at
/// no extra allocation cost.
///
/// # Lifetime
///
/// `BoxedNext` and `Middleware` both return `BoxFuture<'static, _>`, which
/// means any data a middleware closure references after `next(ctx).await`
/// must be owned or `Arc`-shared — not borrowed from `ctx`. `MiddlewareCtx`
/// derives `Clone` precisely so a middleware can keep a copy locally before
/// moving the original into `next`:
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use brontes::{BoxedNext, Middleware, MiddlewareCtx};
///
/// let mw: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
///     Box::pin(async move {
///         let ctx_for_logging = ctx.clone();
///         let result = next(ctx).await;
///         tracing::debug!(
///             tool = %ctx_for_logging.tool_name,
///             ok = result.is_ok(),
///             "middleware post-call",
///         );
///         result
///     })
/// });
/// # let _ = mw;
/// ```
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use brontes::{Middleware, MiddlewareCtx, BoxedNext};
/// # use tracing::debug;
///
/// let mw: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
///     Box::pin(async move {
///         debug!("before: {}", ctx.tool_name);
///         let result = next(ctx).await;
///         debug!("after");
///         result
///     })
/// });
/// ```
pub type Middleware =
    Arc<dyn Fn(MiddlewareCtx, BoxedNext) -> BoxFuture<'static, MiddlewareResult> + Send + Sync>;

/// Filtering rules that decide which commands become MCP tools and which
/// flags those tools expose.
///
/// A `Selector` bundles four optional filters. The tool-generation layer
/// evaluates selectors in configuration order; the first selector whose
/// `cmd` matcher accepts a command claims it. Commands not claimed by any
/// selector are excluded from the generated tool list.
///
/// All fields are optional — an all-`None` `Selector` matches every
/// command and exposes every flag, making it useful as a catch-all at the
/// end of the selector list.
///
/// # Cloning
///
/// Cloning a `Selector` is cheap: all non-`None` fields are `Arc`-wrapped,
/// so each clone shares the underlying closures without copying them.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use brontes::Selector;
///
/// // Catch-all: match everything, expose everything.
/// let catch_all = Selector::default();
///
/// // Targeted: only commands starting with "my-cli deploy".
/// let deploy_only = Selector {
///     cmd: Some(Arc::new(|path: &str| path.starts_with("my-cli deploy"))),
///     ..Default::default()
/// };
/// ```
#[derive(Default, Clone)]
pub struct Selector {
    /// If `Some(matcher)`, this selector applies only to commands whose
    /// space-joined path the matcher returns `true` for. If `None`, every
    /// command that passed the safety filters is matched.
    pub cmd: Option<CmdMatcher>,
    /// Filter applied to a matched command's local (non-global) flags.
    /// `None` means expose all local flags.
    pub local_flag: Option<FlagMatcher>,
    /// Filter applied to a matched command's inherited (global) flags.
    /// `None` means expose all inherited flags.
    pub inherited_flag: Option<FlagMatcher>,
    /// Optional middleware wrapping the exec step for tools claimed by this
    /// selector. `None` means the exec step runs unwrapped.
    pub middleware: Option<Middleware>,
}

impl std::fmt::Debug for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let opt_str = |o: bool| if o { "Some(<fn>)" } else { "None" };
        write!(
            f,
            "Selector {{ cmd: {}, local_flag: {}, inherited_flag: {}, middleware: {} }}",
            opt_str(self.cmd.is_some()),
            opt_str(self.local_flag.is_some()),
            opt_str(self.inherited_flag.is_some()),
            opt_str(self.middleware.is_some()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_is_cheap() {
        let matcher: CmdMatcher = Arc::new(|p: &str| p.contains("foo"));
        let sel = Selector {
            cmd: Some(matcher),
            ..Default::default()
        };
        let sel2 = sel.clone();

        let m1 = sel.cmd.as_ref().unwrap();
        let m2 = sel2.cmd.as_ref().unwrap();

        // Both clones agree on "foo bar" (match) and "baz qux" (no match).
        assert!(m1("foo bar"));
        assert!(m2("foo bar"));
        assert!(!m1("baz qux"));
        assert!(!m2("baz qux"));

        // Verify that the clone shares the Arc allocation, not a copy.
        assert!(
            Arc::ptr_eq(sel.cmd.as_ref().unwrap(), sel2.cmd.as_ref().unwrap()),
            "clone must share Arc allocation, not produce a copy"
        );
    }

    #[test]
    fn cmd_matcher_accepts_str() {
        let m: CmdMatcher = Arc::new(|path: &str| path == "my-cli list");
        assert!(m("my-cli list"), "exact match must succeed");
        assert!(!m("my-cli"), "prefix-only must not match");
        assert!(
            !m("my-cli list --all"),
            "suffix-extended path must not match"
        );
    }

    #[test]
    fn flag_matcher_inspects_clap_arg() {
        let m: FlagMatcher = Arc::new(|a: &clap::Arg| a.get_id().as_str() == "verbose");

        let verbose = clap::Arg::new("verbose");
        let force = clap::Arg::new("force");

        assert!(m(&verbose), "verbose arg must match");
        assert!(!m(&force), "force arg must not match");
    }

    #[test]
    fn debug_impl_for_selector() {
        let sel = Selector {
            cmd: Some(Arc::new(|_: &str| true)),
            local_flag: None,
            inherited_flag: Some(Arc::new(|_: &clap::Arg| true)),
            middleware: None,
        };
        let s = format!("{sel:?}");
        assert!(
            s.contains("Some(<fn>)"),
            "Debug should label Some-slots as Some(<fn>): got {s}"
        );
        assert!(
            s.contains("None"),
            "Debug should label None-slots as None: got {s}"
        );
        assert!(
            s.contains("Selector"),
            "Debug should name the type: got {s}"
        );
    }

    #[test]
    fn middleware_type_compiles() {
        // This test exists to prove the Middleware type alias is correctly
        // formed: all trait-object bounds are satisfied and the closure
        // coerces to the Arc<dyn Fn(...)> shape. If Send + Sync bounds or
        // the BoxFuture lifetime are wrong, this will not compile.
        let _mw: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
            Box::pin(async move { next(ctx).await })
        });
        // No need to actually call it — the compile-time coercion is the proof.
    }
}
