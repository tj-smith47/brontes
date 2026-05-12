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
// title (ophis: "title")
// ---------------------------------------------------------------------------

#[test]
fn title_propagates() {
    let ann = ToolAnnotations {
        title: Some("My Tool".into()),
        ..Default::default()
    };
    let rmcp = ann.to_rmcp().expect("title set — must return Some");
    assert_eq!(rmcp.title, Some("My Tool".to_owned()));
}

// ---------------------------------------------------------------------------
// readOnlyHint true/false (ophis: "readOnlyHint true", "readOnlyHint false")
// ---------------------------------------------------------------------------

#[test]
fn read_only_hint_true() {
    let ann = ToolAnnotations {
        read_only_hint: Some(true),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("read_only_hint set — must return Some");
    assert_eq!(rmcp.read_only_hint, Some(true));
}

#[test]
fn read_only_hint_false() {
    let ann = ToolAnnotations {
        read_only_hint: Some(false),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("read_only_hint set — must return Some");
    assert_eq!(rmcp.read_only_hint, Some(false));
}

// ---------------------------------------------------------------------------
// destructiveHint true/false (ophis: "destructiveHint true", "destructiveHint false")
// ---------------------------------------------------------------------------

#[test]
fn destructive_hint_true() {
    let ann = ToolAnnotations {
        destructive_hint: Some(true),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("destructive_hint set — must return Some");
    assert_eq!(rmcp.destructive_hint, Some(true));
}

#[test]
fn destructive_hint_false() {
    let ann = ToolAnnotations {
        destructive_hint: Some(false),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("destructive_hint set — must return Some");
    assert_eq!(rmcp.destructive_hint, Some(false));
}

// ---------------------------------------------------------------------------
// idempotentHint true AND false (ophis covers true; wire-shape divergence divergence adds false)
// ---------------------------------------------------------------------------

#[test]
fn idempotent_hint_true() {
    let ann = ToolAnnotations {
        idempotent_hint: Some(true),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("idempotent_hint set — must return Some");
    assert_eq!(rmcp.idempotent_hint, Some(true));
}

#[test]
fn idempotent_hint_false() {
    // wire-shape divergence divergence: brontes forwards Some(false) explicitly rather than
    // omitting the field, so the client receives an unambiguous signal.
    let ann = ToolAnnotations {
        idempotent_hint: Some(false),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("idempotent_hint set — must return Some");
    assert_eq!(rmcp.idempotent_hint, Some(false));
}

// ---------------------------------------------------------------------------
// openWorldHint true/false (ophis: "openWorldHint true", "openWorldHint false")
// ---------------------------------------------------------------------------

#[test]
fn open_world_hint_true() {
    let ann = ToolAnnotations {
        open_world_hint: Some(true),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("open_world_hint set — must return Some");
    assert_eq!(rmcp.open_world_hint, Some(true));
}

#[test]
fn open_world_hint_false() {
    let ann = ToolAnnotations {
        open_world_hint: Some(false),
        ..Default::default()
    };
    let rmcp = ann
        .to_rmcp()
        .expect("open_world_hint set — must return Some");
    assert_eq!(rmcp.open_world_hint, Some(false));
}

// ---------------------------------------------------------------------------
// all fields together — "Delete Resource" scenario
// (ophis: "all fields together")
// ---------------------------------------------------------------------------

#[test]
fn all_fields_together() {
    let ann = ToolAnnotations {
        title: Some("Delete Resource".into()),
        read_only_hint: Some(false),
        destructive_hint: Some(true),
        idempotent_hint: Some(true),
        open_world_hint: Some(false),
    };
    let rmcp = ann.to_rmcp().expect("all fields set — must return Some");
    assert_eq!(rmcp.title, Some("Delete Resource".to_owned()));
    assert_eq!(rmcp.read_only_hint, Some(false));
    assert_eq!(rmcp.destructive_hint, Some(true));
    assert_eq!(rmcp.idempotent_hint, Some(true));
    assert_eq!(rmcp.open_world_hint, Some(false));
}

// ---------------------------------------------------------------------------
// Extra: wire-shape proof (cannot be expressed in ophis suite)
//
// Serialise the rmcp output and confirm that Some(false) survives as an
// explicit JSON field, proving the wire-shape divergence divergence behaves as documented.
// ---------------------------------------------------------------------------

#[test]
fn some_false_survives_on_wire() {
    let ann = ToolAnnotations {
        read_only_hint: Some(false),
        destructive_hint: Some(false),
        idempotent_hint: Some(false),
        open_world_hint: Some(false),
        ..Default::default()
    };
    let rmcp = ann.to_rmcp().expect("hints set — must return Some");
    let json = serde_json::to_value(&rmcp).expect("serialisation must succeed");

    assert_eq!(
        json.get("readOnlyHint"),
        Some(&serde_json::Value::Bool(false)),
        "readOnlyHint: false must appear on the wire"
    );
    assert_eq!(
        json.get("destructiveHint"),
        Some(&serde_json::Value::Bool(false)),
        "destructiveHint: false must appear on the wire"
    );
    assert_eq!(
        json.get("idempotentHint"),
        Some(&serde_json::Value::Bool(false)),
        "idempotentHint: false must appear on the wire"
    );
    assert_eq!(
        json.get("openWorldHint"),
        Some(&serde_json::Value::Bool(false)),
        "openWorldHint: false must appear on the wire"
    );
}
