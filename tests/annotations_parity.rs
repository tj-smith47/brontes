//! Parity tests for [`brontes::ToolAnnotations`] against the ophis Go suite
//! (`/tmp/ophis/annotations_test.go`, `TestToolAnnotationsFromCmd`).
//!
//! ## Dropped cases
//!
//! - `non-MCP annotations returns nil` — brontes is typed; consumers cannot
//!   pass arbitrary string key/value pairs, so there is nothing to test.
//! - `invalid bool value is skipped` — `Option<bool>` accepts only `true`,
//!   `false`, or `None`; invalid string parsing does not exist in this API.
//! - `mixed valid and invalid values` — same reason.
//! - `strconv.ParseBool variants` — same reason (no string parsing).

use brontes::ToolAnnotations;
use pretty_assertions::assert_eq;

// ---------------------------------------------------------------------------
// no annotations returns nil (ophis: "no annotations returns nil")
// ---------------------------------------------------------------------------

#[test]
fn default_returns_none() {
    assert!(
        ToolAnnotations::default().to_rmcp().is_none(),
        "all-None annotations must produce None"
    );
}

#[test]
fn title_empty_string_propagates() {
    // Empty-string title is treated as set (the consumer passed Some, not None),
    // so to_rmcp returns Some and the wire receives `"title": ""`. This pins the
    // behavior — a future change that suppresses empty-string titles will need
    // to update this test deliberately.
    let ann = ToolAnnotations {
        title: Some(String::new()),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("Some(\"\".into()) is a set field, should not collapse to None");
    assert_eq!(rmcp.title.as_deref(), Some(""));
    assert!(rmcp.read_only_hint.is_none());
    assert!(rmcp.destructive_hint.is_none());
    assert!(rmcp.idempotent_hint.is_none());
    assert!(rmcp.open_world_hint.is_none());
}

// ---------------------------------------------------------------------------
// All-fields round-trip pinned against the JSON wire shape.
//
// This single test replaces nine single-field identity tests that asserted
// e.g. `Some(true).read_only_hint → rmcp.read_only_hint == Some(true)` —
// tautologies proving only that the `to_rmcp()` mapping preserves field
// values, which is satisfied by construction. The current test instead
// pins:
//
//   1. Every clap-side `ToolAnnotations` field maps to the right rmcp
//      field name on the wire (`read_only_hint` → `"readOnlyHint"` etc.).
//      A serde rename or struct-field rename breaks this immediately.
//   2. Both `Some(true)` AND `Some(false)` survive — proving the
//      wire-shape divergence from ophis (which omits false-valued hints).
//      brontes forwards `false` explicitly so the MCP client receives an
//      unambiguous signal.
//
// The value pattern (title set, true/false alternating across hints) is
// deliberately mixed so a renamed-pair bug (e.g. read_only ↔ destructive
// swap) surfaces as an actual diff in the comparison.
// ---------------------------------------------------------------------------

#[test]
fn all_hints_serialize_to_pinned_wire_shape() {
    let ann = ToolAnnotations {
        title: Some("Delete Resource".into()),
        read_only_hint: Some(true),
        destructive_hint: Some(false),
        idempotent_hint: Some(true),
        open_world_hint: Some(false),
    };
    let rmcp = ann
        .to_rmcp()
        .expect("Some on at least one field — must return Some");
    let json = serde_json::to_value(&rmcp).expect("serialisation must succeed");
    assert_eq!(
        json,
        serde_json::json!({
            "title": "Delete Resource",
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        })
    );
}
