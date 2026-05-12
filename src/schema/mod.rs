//! JSON Schema generation for the MCP tool surface.
//!
//! The cache module holds the base schemas for `ToolInput` and `ToolOutput`;
//! the types module exposes [`SchemaType`] for coarse flag-type classification
//! and `Config.flag_type_overrides`.

// `args` is crate-internal — Task 11's orchestrator will consume it to build
// the `args` property description for per-tool input schemas.
// `cache` is crate-internal — only the per-tool orchestrator consumes it.
// `types` is public because consumers reference `SchemaType` via
// `Config.flag_type_overrides`.
pub(crate) mod args;
pub(crate) mod cache;
pub(crate) mod flag;
pub mod types;

pub use types::SchemaType;
