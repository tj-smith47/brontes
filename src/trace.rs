//! W3C Trace Context propagation from MCP `_meta` into the spawned CLI.
//!
//! MCP `2026-07-28` reserves the `traceparent`, `tracestate`, and `baggage`
//! keys in a request's `_meta` map (SEP-414) so a client can hand a server the
//! span it is calling from.  brontes' unit of work is a subprocess, and the
//! conventional way to hand a trace to a subprocess is the environment — so
//! [`TraceContext`] lowers those three `_meta` keys onto the `TRACEPARENT`,
//! `TRACESTATE`, and `BAGGAGE` environment variables that OpenTelemetry's
//! process-level propagators read.
//!
//! An instrumented CLI therefore joins the agent's trace with no configuration
//! on either side: the client sets `_meta`, brontes sets the environment, the
//! CLI's tracing init picks it up.
//!
//! # Values arrive untrusted
//!
//! Every value here originates with the MCP client, so each is validated
//! against the W3C wire format before it reaches the child's environment.  A
//! value that fails validation is dropped with a `tracing::warn!` rather than
//! forwarded: W3C requires a receiver that reads a malformed `traceparent` to
//! restart the trace, and forwarding a malformed one would corrupt the
//! consumer's trace silently instead of loudly.

use rmcp::model::RequestMetaObject;

/// Environment variable carrying the W3C `traceparent`.
pub const ENV_TRACEPARENT: &str = "TRACEPARENT";
/// Environment variable carrying the W3C `tracestate`.
pub const ENV_TRACESTATE: &str = "TRACESTATE";
/// Environment variable carrying W3C `baggage`.
pub const ENV_BAGGAGE: &str = "BAGGAGE";

/// Length of a version-`00` `traceparent`: `00-<32 hex>-<16 hex>-<2 hex>`.
const TRACEPARENT_V0_LEN: usize = 55;
/// W3C caps `tracestate` at 512 characters.
const MAX_TRACESTATE_LEN: usize = 512;
/// W3C caps the total `baggage` header at 8192 bytes.
const MAX_BAGGAGE_LEN: usize = 8192;

/// W3C Trace Context carried on an MCP request's `_meta` (SEP-414).
///
/// Each field is `None` when the client omitted it or when the value it sent
/// failed validation.  Obtain one from a [`crate::MiddlewareCtx`]; brontes
/// builds it per tool call and never asks a consumer to construct one.
///
/// # Example
///
/// ```rust
/// use brontes::{BoxedNext, Middleware, MiddlewareCtx};
/// use std::sync::Arc;
///
/// let mw: Middleware = Arc::new(|ctx: MiddlewareCtx, next: BoxedNext| {
///     Box::pin(async move {
///         if let Some(parent) = ctx.trace_context.traceparent() {
///             tracing::debug!(%parent, "tool call is part of a client trace");
///         }
///         next(ctx).await
///     })
/// });
/// # let _ = mw;
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TraceContext {
    traceparent: Option<String>,
    tracestate: Option<String>,
    baggage: Option<String>,
}

impl TraceContext {
    /// The validated W3C `traceparent`, if the client sent a well-formed one.
    #[must_use]
    pub fn traceparent(&self) -> Option<&str> {
        self.traceparent.as_deref()
    }

    /// The validated W3C `tracestate`, if the client sent a well-formed one.
    ///
    /// Always `None` when [`TraceContext::traceparent`] is `None`: `tracestate`
    /// describes vendor state for a span that, without a `traceparent`, does
    /// not exist.
    #[must_use]
    pub fn tracestate(&self) -> Option<&str> {
        self.tracestate.as_deref()
    }

    /// The validated W3C `baggage`, if the client sent a well-formed one.
    ///
    /// Unlike [`TraceContext::tracestate`] this is independent of
    /// `traceparent` — W3C Baggage is a separate specification and propagates
    /// with or without an active trace.
    #[must_use]
    pub fn baggage(&self) -> Option<&str> {
        self.baggage.as_deref()
    }

    /// True when no field survived validation, so nothing would be propagated.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.traceparent.is_none() && self.tracestate.is_none() && self.baggage.is_none()
    }

    /// The `(variable, value)` pairs this context contributes to the child's
    /// environment, in W3C order.
    pub(crate) fn env_pairs(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            (ENV_TRACEPARENT, self.traceparent.as_deref()),
            (ENV_TRACESTATE, self.tracestate.as_deref()),
            (ENV_BAGGAGE, self.baggage.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| (name, v)))
    }

    /// Extract and validate the trace context from a request's `_meta`.
    ///
    /// `tool_name` only labels the warnings emitted for rejected values.
    pub(crate) fn from_meta(meta: &RequestMetaObject, tool_name: &str) -> Self {
        let traceparent = meta.0.get_traceparent().and_then(|raw| {
            if is_valid_traceparent(raw) {
                Some(raw.to_owned())
            } else {
                tracing::warn!(
                    target: "brontes::trace",
                    tool = %tool_name,
                    "ignoring malformed W3C traceparent in request _meta; \
                     the tool call will not join the client's trace"
                );
                None
            }
        });

        // A `tracestate` without a `traceparent` names vendor state for a span
        // that does not exist; W3C requires dropping it in that case.
        let tracestate = traceparent.as_ref().and_then(|_| {
            meta.0.get_tracestate().and_then(|raw| {
                if is_valid_list_member_value(raw, MAX_TRACESTATE_LEN) {
                    Some(raw.to_owned())
                } else {
                    tracing::warn!(
                        target: "brontes::trace",
                        tool = %tool_name,
                        "ignoring malformed or oversized W3C tracestate in request _meta"
                    );
                    None
                }
            })
        });

        let baggage = meta.0.get_baggage().and_then(|raw| {
            if is_valid_list_member_value(raw, MAX_BAGGAGE_LEN) {
                Some(raw.to_owned())
            } else {
                tracing::warn!(
                    target: "brontes::trace",
                    tool = %tool_name,
                    "ignoring malformed or oversized W3C baggage in request _meta"
                );
                None
            }
        });

        Self {
            traceparent,
            tracestate,
            baggage,
        }
    }
}

/// True when `raw` is a valid W3C `traceparent`.
///
/// Enforces the version-`00` field widths, lowercase hex, the reserved-`ff`
/// version, and the all-zero trace-id / parent-id prohibitions.  Versions
/// above `00` may carry additional dash-separated fields, which the spec
/// requires a `00` implementation to tolerate rather than reject.
fn is_valid_traceparent(raw: &str) -> bool {
    let mut parts = raw.split('-');
    let (Some(version), Some(trace_id), Some(span_id), Some(flags)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };

    // `ff` is reserved as an invalid version marker.
    if version.len() != 2 || !is_lower_hex(version) || version == "ff" {
        return false;
    }
    if trace_id.len() != 32 || !is_lower_hex(trace_id) || trace_id.bytes().all(|b| b == b'0') {
        return false;
    }
    if span_id.len() != 16 || !is_lower_hex(span_id) || span_id.bytes().all(|b| b == b'0') {
        return false;
    }
    if flags.len() != 2 || !is_lower_hex(flags) {
        return false;
    }

    // Version `00` is exactly four fields; a longer string is a different
    // version's format wearing the `00` prefix and must be rejected.
    if version == "00" {
        return raw.len() == TRACEPARENT_V0_LEN && parts.next().is_none();
    }
    true
}

/// True when every byte is a lowercase hexadecimal digit.
fn is_lower_hex(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// True when `raw` fits `max_len` and contains only the printable ASCII that
/// `tracestate` and `baggage` list members permit.
///
/// Rejecting control bytes matters beyond format correctness: these values
/// reach a child process's environment, and a newline or NUL there is a
/// smuggling primitive rather than a trace.
fn is_valid_list_member_value(raw: &str, max_len: usize) -> bool {
    !raw.is_empty()
        && raw.len() <= max_len
        && raw.bytes().all(|b| (0x21..=0x7E).contains(&b) || b == b' ')
}

#[cfg(test)]
mod tests {
    use rmcp::model::RequestMetaObject;

    use super::*;

    /// A well-formed version-`00` traceparent, from the W3C specification's
    /// own example.
    const VALID_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";

    fn meta_with(
        traceparent: Option<&str>,
        tracestate: Option<&str>,
        baggage: Option<&str>,
    ) -> RequestMetaObject {
        let mut meta = RequestMetaObject::new();
        if let Some(v) = traceparent {
            meta.0.set_traceparent(v);
        }
        if let Some(v) = tracestate {
            meta.0.set_tracestate(v);
        }
        if let Some(v) = baggage {
            meta.0.set_baggage(v);
        }
        meta
    }

    #[test]
    fn absent_meta_yields_an_empty_context_that_propagates_nothing() {
        let ctx = TraceContext::from_meta(&RequestMetaObject::new(), "tool");
        assert!(ctx.is_empty());
        assert_eq!(ctx.env_pairs().count(), 0);
    }

    #[test]
    fn valid_trio_lowers_onto_the_three_w3c_env_vars() {
        let meta = meta_with(Some(VALID_PARENT), Some("vendor=state"), Some("key=value"));
        let ctx = TraceContext::from_meta(&meta, "tool");

        assert_eq!(ctx.traceparent(), Some(VALID_PARENT));
        assert_eq!(ctx.tracestate(), Some("vendor=state"));
        assert_eq!(ctx.baggage(), Some("key=value"));
        assert!(!ctx.is_empty());

        let pairs: Vec<_> = ctx.env_pairs().collect();
        assert_eq!(
            pairs,
            vec![
                (ENV_TRACEPARENT, VALID_PARENT),
                (ENV_TRACESTATE, "vendor=state"),
                (ENV_BAGGAGE, "key=value"),
            ]
        );
    }

    #[test]
    fn malformed_traceparent_is_dropped_rather_than_forwarded() {
        for bad in [
            "",
            "not-a-traceparent",
            // `ff` is the reserved invalid version.
            "ff-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01",
            // All-zero trace id.
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            // All-zero parent id.
            "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01",
            // Uppercase hex: W3C mandates lowercase.
            "00-0AF7651916CD43DD8448EB211C80319C-00f067aa0ba902b7-01",
            // Truncated trace id.
            "00-0af7651916cd43dd8448eb211c8031-00f067aa0ba902b7-01",
            // Version `00` with a trailing field.
            "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01-extra",
            // Missing the flags field entirely.
            "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7",
        ] {
            let ctx = TraceContext::from_meta(&meta_with(Some(bad), None, None), "tool");
            assert_eq!(ctx.traceparent(), None, "must reject {bad:?}");
            assert!(ctx.is_empty(), "must propagate nothing for {bad:?}");
        }
    }

    #[test]
    fn future_versions_may_carry_extra_fields() {
        // A `01` implementation is allowed to append fields; a `00` reader
        // must tolerate the prefix it understands rather than reject it.
        let raw = "01-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01-future";
        let ctx = TraceContext::from_meta(&meta_with(Some(raw), None, None), "tool");
        assert_eq!(ctx.traceparent(), Some(raw));
    }

    #[test]
    fn tracestate_without_a_traceparent_is_dropped() {
        let ctx = TraceContext::from_meta(&meta_with(None, Some("vendor=state"), None), "tool");
        assert_eq!(ctx.tracestate(), None);
        assert!(ctx.is_empty());
    }

    #[test]
    fn baggage_survives_without_a_traceparent() {
        // W3C Baggage is a separate specification with no traceparent
        // dependency, so it propagates on its own.
        let ctx = TraceContext::from_meta(&meta_with(None, None, Some("tenant=acme")), "tool");
        assert_eq!(ctx.baggage(), Some("tenant=acme"));
        assert_eq!(ctx.traceparent(), None);
        assert!(!ctx.is_empty());
        assert_eq!(
            ctx.env_pairs().collect::<Vec<_>>(),
            vec![(ENV_BAGGAGE, "tenant=acme")]
        );
    }

    #[test]
    fn control_bytes_are_rejected_so_nothing_smuggles_into_the_child_env() {
        for bad in ["a=b\nc=d", "a=b\0c", "a=b\rc", "a=b\tc"] {
            let ctx = TraceContext::from_meta(
                &meta_with(Some(VALID_PARENT), Some(bad), Some(bad)),
                "tool",
            );
            assert_eq!(ctx.tracestate(), None, "tracestate must reject {bad:?}");
            assert_eq!(ctx.baggage(), None, "baggage must reject {bad:?}");
        }
    }

    #[test]
    fn oversized_values_are_rejected_at_the_w3c_limits() {
        let long_state = "a".repeat(MAX_TRACESTATE_LEN + 1);
        let long_baggage = "b".repeat(MAX_BAGGAGE_LEN + 1);
        let ctx = TraceContext::from_meta(
            &meta_with(Some(VALID_PARENT), Some(&long_state), Some(&long_baggage)),
            "tool",
        );
        assert_eq!(ctx.tracestate(), None);
        assert_eq!(ctx.baggage(), None);

        // Exactly at the limit is accepted — the check is inclusive.
        let at_state = "a".repeat(MAX_TRACESTATE_LEN);
        let at_baggage = "b".repeat(MAX_BAGGAGE_LEN);
        let ctx = TraceContext::from_meta(
            &meta_with(Some(VALID_PARENT), Some(&at_state), Some(&at_baggage)),
            "tool",
        );
        assert_eq!(ctx.tracestate(), Some(at_state.as_str()));
        assert_eq!(ctx.baggage(), Some(at_baggage.as_str()));
    }
}
