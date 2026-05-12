//! JSON Schema generation for the MCP tool surface.
//!
//! The cache module holds the base schemas for `ToolInput` and `ToolOutput`;
//! the types module exposes [`SchemaType`] for coarse flag-type classification
//! and `Config.flag_type_overrides`.

// `cache` is crate-internal — only the per-tool orchestrator consumes it.
// `types` is public because consumers reference `SchemaType` via
// `Config.flag_type_overrides`.
pub(crate) mod cache;
pub(crate) mod flag;
pub mod types;

pub use types::SchemaType;
