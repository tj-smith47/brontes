//! JSON Schema generation for the MCP tool surface.
//!
//! The cache module holds the base schemas for `ToolInput` and `ToolOutput`;
//! the types module exposes [`SchemaType`] for coarse flag-type classification
//! and `Config.flag_type_overrides`.

pub(crate) mod cache;
pub mod types;

pub use types::SchemaType;
