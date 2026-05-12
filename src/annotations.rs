//! Tool annotation hints forwarded to MCP clients.
//!
//! [`ToolAnnotations`] is the brontes-side representation of an optional
//! title and four optional behavior hints defined by the Model Context
//! Protocol.  When all fields are `None` it serialises to nothing; when
//! at least one field is set it converts to an `rmcp::model::ToolAnnotations`
//! that carries exactly the fields that were provided.
//!
//! ## Wire-shape divergence from ophis
//!
//! ophis (the Go reference) treats boolean hints as a three-state value via
//! `*bool`: unset (omitted), `true`, or `false`.  When a boolean hint is
//! explicitly set to `false`, ophis omits it from the JSON output in some
//! code paths because the zero-value of `bool` in Go is `false` and the MCP
//! field defaults are often `false`.
//!
//! brontes uses `Option<bool>` end-to-end.  `Some(false)` is a deliberate
//! override and is forwarded to rmcp, which serialises it as
//! `"readOnlyHint": false` (or whichever field) via
//! `#[serde(skip_serializing_if = "Option::is_none")]`.  The result is that
//! brontes always emits explicit `false` values when they are set, giving
//! MCP clients an unambiguous signal.  Consumers that want the field omitted
//! should leave it as `None`.

/// Annotation hints attached to an MCP tool.
///
/// All fields are optional.  A [`ToolAnnotations`] where every field is
/// `None` (i.e. [`Default::default()`]) carries no information and
/// [`to_rmcp`](ToolAnnotations::to_rmcp) returns `None` for it.
///
/// Setting a boolean hint to `Some(false)` is an explicit override and
/// is forwarded to the wire as-is; see the crate-level divergence note
/// on wire-shape behaviour for details.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ToolAnnotations {
    /// A human-readable title for the tool.
    pub title: Option<String>,

    /// When `Some(true)` the tool does not modify its environment.
    ///
    /// Corresponds to `readOnlyHint` on the wire.
    pub read_only_hint: Option<bool>,

    /// When `Some(true)` the tool may perform destructive updates.
    /// When `Some(false)` the tool performs only additive updates.
    ///
    /// Meaningful only when `read_only_hint` is not `Some(true)`.
    /// Corresponds to `destructiveHint` on the wire.
    pub destructive_hint: Option<bool>,

    /// When `Some(true)` calling the tool repeatedly with identical
    /// arguments has no additional effect on its environment.
    ///
    /// Meaningful only when `read_only_hint` is not `Some(true)`.
    /// Corresponds to `idempotentHint` on the wire.
    pub idempotent_hint: Option<bool>,

    /// When `Some(true)` the tool may interact with an open world of
    /// external entities (e.g. a web search). When `Some(false)` its
    /// domain of interaction is closed (e.g. an in-process memory store).
    ///
    /// Corresponds to `openWorldHint` on the wire.
    pub open_world_hint: Option<bool>,
}

impl ToolAnnotations {
    /// Convert to rmcp's wire-shape [`rmcp::model::ToolAnnotations`].
    ///
    /// Returns `None` when every field is unset — there is nothing to
    /// attach to the tool.  Returns `Some(_)` as soon as at least one
    /// field carries a value; the resulting rmcp struct holds exactly
    /// the fields that were provided, forwarding `Some(false)` explicitly
    /// so that an explicit `false` appears on the wire rather than being
    /// omitted.
    #[must_use]
    pub fn to_rmcp(&self) -> Option<rmcp::model::ToolAnnotations> {
        let any_set = self.title.is_some()
            || self.read_only_hint.is_some()
            || self.destructive_hint.is_some()
            || self.idempotent_hint.is_some()
            || self.open_world_hint.is_some();

        if !any_set {
            return None;
        }

        Some(rmcp::model::ToolAnnotations::from_raw(
            self.title.clone(),
            self.read_only_hint,
            self.destructive_hint,
            self.idempotent_hint,
            self.open_world_hint,
        ))
    }
}
