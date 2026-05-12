//! Parity tests for [`brontes::selectors`] against the ophis Go suite
//! (`/tmp/ophis/selectors_test.go`).
//!
//! Each Rust test corresponds 1-to-1 with a Go sub-test. The helper
//! [`path_from`] mirrors Go's `buildCommandTree`: it joins command-name parts
//! with spaces, matching the `CommandPath()` behaviour that the Rust walker
//! replicates via space-joined paths.
//!
//! ## Dropped cases
//!
//! None — every case in the Go file has a direct Rust analog.

use brontes::selectors::{
    allow_cmds, allow_cmds_containing, allow_flags, exclude_cmds, exclude_cmds_containing,
    exclude_flags, no_flags,
};

// ---------------------------------------------------------------------------
// Helper — mirrors buildCommandTree(names...).CommandPath()
// ---------------------------------------------------------------------------

/// Builds a path string the same way the Rust walker would:
/// `["kubectl", "get", "pods"]` → `"kubectl get pods"`.
fn path_from(parts: &[&str]) -> String {
    parts.join(" ")
}

// ---------------------------------------------------------------------------
// TestAllowCmdsContaining
// ---------------------------------------------------------------------------

#[test]
fn allow_cmds_containing_matches_single_phrase() {
    let m = allow_cmds_containing(["get"]);
    assert!(m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_containing_matches_one_of_multiple_phrases() {
    let m = allow_cmds_containing(["get", "list"]);
    assert!(m(&path_from(&["helm", "list"])));
}

#[test]
fn allow_cmds_containing_matches_exact_command_name() {
    let m = allow_cmds_containing(["kubectl get"]);
    assert!(m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_containing_does_not_match_when_phrase_absent() {
    let m = allow_cmds_containing(["delete"]);
    assert!(!m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_containing_does_not_match_any_of_multiple_phrases() {
    let m = allow_cmds_containing(["delete", "remove"]);
    assert!(!m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_containing_partial_match_in_middle_of_path() {
    let m = allow_cmds_containing(["admin"]);
    assert!(m(&path_from(&["cli", "admin", "user"])));
}

#[test]
fn allow_cmds_containing_empty_phrases_list_rejects_everything() {
    let m = allow_cmds_containing([] as [&str; 0]);
    assert!(!m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_containing_case_sensitive_matching() {
    let m = allow_cmds_containing(["Get"]);
    assert!(!m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_containing_matches_substring_in_command_name() {
    let m = allow_cmds_containing(["pod"]);
    assert!(m(&path_from(&["kubectl", "get", "pods"])));
}

// ---------------------------------------------------------------------------
// TestExcludeCmdsContaining
// ---------------------------------------------------------------------------

#[test]
fn exclude_cmds_containing_excludes_matching_phrase() {
    let m = exclude_cmds_containing(["delete"]);
    assert!(!m(&path_from(&["kubectl", "delete", "pod"])));
}

#[test]
fn exclude_cmds_containing_excludes_when_any_phrase_matches() {
    let m = exclude_cmds_containing(["delete", "remove"]);
    assert!(!m(&path_from(&["kubectl", "delete", "pod"])));
}

#[test]
fn exclude_cmds_containing_allows_when_no_phrase_matches() {
    let m = exclude_cmds_containing(["delete"]);
    assert!(m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn exclude_cmds_containing_allows_when_none_of_multiple_phrases_match() {
    let m = exclude_cmds_containing(["delete", "remove", "destroy"]);
    assert!(m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn exclude_cmds_containing_excludes_partial_match_in_path() {
    let m = exclude_cmds_containing(["admin"]);
    assert!(!m(&path_from(&["cli", "admin", "user"])));
}

#[test]
fn exclude_cmds_containing_empty_phrases_list_allows_everything() {
    let m = exclude_cmds_containing([] as [&str; 0]);
    assert!(m(&path_from(&["kubectl", "delete", "pod"])));
}

#[test]
fn exclude_cmds_containing_case_sensitive_matching() {
    let m = exclude_cmds_containing(["Delete"]);
    assert!(m(&path_from(&["kubectl", "delete", "pod"])));
}

#[test]
fn exclude_cmds_containing_excludes_exact_command_name() {
    let m = exclude_cmds_containing(["kubectl delete"]);
    assert!(!m(&path_from(&["kubectl", "delete", "pod"])));
}

#[test]
fn exclude_cmds_containing_excludes_substring_in_command_name() {
    let m = exclude_cmds_containing(["dele"]);
    assert!(!m(&path_from(&["kubectl", "delete", "pod"])));
}

// ---------------------------------------------------------------------------
// TestAllowCmds
// ---------------------------------------------------------------------------

#[test]
fn allow_cmds_matches_exact_command_path() {
    let m = allow_cmds(["kubectl get"]);
    assert!(m(&path_from(&["kubectl", "get"])));
}

#[test]
fn allow_cmds_does_not_match_partial_path() {
    let m = allow_cmds(["kubectl get"]);
    assert!(!m(&path_from(&["kubectl", "get", "pods"])));
}

#[test]
fn allow_cmds_matches_one_of_multiple_commands() {
    let m = allow_cmds(["kubectl get", "helm list", "docker ps"]);
    assert!(m(&path_from(&["helm", "list"])));
}

#[test]
fn allow_cmds_does_not_match_if_not_in_list() {
    let m = allow_cmds(["kubectl get", "helm list"]);
    assert!(!m(&path_from(&["docker", "ps"])));
}

#[test]
fn allow_cmds_empty_list_rejects_everything() {
    let m = allow_cmds([] as [&str; 0]);
    assert!(!m(&path_from(&["kubectl", "get"])));
}

#[test]
fn allow_cmds_case_sensitive_matching() {
    let m = allow_cmds(["kubectl Get"]);
    assert!(!m(&path_from(&["kubectl", "get"])));
}

#[test]
fn allow_cmds_requires_exact_match_no_substring() {
    let m = allow_cmds(["kubectl"]);
    assert!(!m(&path_from(&["kubectl", "get"])));
}

#[test]
fn allow_cmds_single_command_name() {
    let m = allow_cmds(["kubectl"]);
    assert!(m(&path_from(&["kubectl"])));
}

// ---------------------------------------------------------------------------
// TestExcludeCmds
// ---------------------------------------------------------------------------

#[test]
fn exclude_cmds_excludes_exact_command_path() {
    let m = exclude_cmds(["kubectl delete"]);
    assert!(!m(&path_from(&["kubectl", "delete"])));
}

#[test]
fn exclude_cmds_allows_partial_path_not_in_list() {
    let m = exclude_cmds(["kubectl delete"]);
    assert!(m(&path_from(&["kubectl", "delete", "pod"])));
}

#[test]
fn exclude_cmds_excludes_if_in_list_of_multiple_commands() {
    let m = exclude_cmds(["kubectl delete", "helm uninstall", "docker rm"]);
    assert!(!m(&path_from(&["helm", "uninstall"])));
}

#[test]
fn exclude_cmds_allows_if_not_in_exclusion_list() {
    let m = exclude_cmds(["kubectl delete", "helm uninstall"]);
    assert!(m(&path_from(&["kubectl", "get"])));
}

#[test]
fn exclude_cmds_empty_list_allows_everything() {
    let m = exclude_cmds([] as [&str; 0]);
    assert!(m(&path_from(&["kubectl", "delete"])));
}

#[test]
fn exclude_cmds_case_sensitive_matching() {
    let m = exclude_cmds(["kubectl Delete"]);
    assert!(m(&path_from(&["kubectl", "delete"])));
}

#[test]
fn exclude_cmds_requires_exact_match_no_substring() {
    let m = exclude_cmds(["kubectl"]);
    assert!(m(&path_from(&["kubectl", "delete"])));
}

#[test]
fn exclude_cmds_single_command_exclusion() {
    let m = exclude_cmds(["kubectl"]);
    assert!(!m(&path_from(&["kubectl"])));
}

// ---------------------------------------------------------------------------
// TestAllowFlags
// ---------------------------------------------------------------------------

#[test]
fn allow_flags_allows_single_matching_flag() {
    let m = allow_flags(["namespace"]);
    assert!(m(&clap::Arg::new("namespace")));
}

#[test]
fn allow_flags_allows_one_of_multiple_flags() {
    let m = allow_flags(["namespace", "output", "verbose"]);
    assert!(m(&clap::Arg::new("output")));
}

#[test]
fn allow_flags_rejects_non_matching_flag() {
    let m = allow_flags(["namespace"]);
    assert!(!m(&clap::Arg::new("kubeconfig")));
}

#[test]
fn allow_flags_rejects_when_not_in_multiple_allowed_flags() {
    let m = allow_flags(["namespace", "output"]);
    assert!(!m(&clap::Arg::new("kubeconfig")));
}

#[test]
fn allow_flags_empty_list_rejects_all_flags() {
    let m = allow_flags([] as [&str; 0]);
    assert!(!m(&clap::Arg::new("namespace")));
}

#[test]
fn allow_flags_exact_name_match_required() {
    let m = allow_flags(["namespace"]);
    assert!(!m(&clap::Arg::new("namespaces")));
}

#[test]
fn allow_flags_case_sensitive_matching() {
    let m = allow_flags(["Namespace"]);
    assert!(!m(&clap::Arg::new("namespace")));
}

#[test]
fn allow_flags_allows_multiple_matching_flags() {
    let m = allow_flags(["verbose", "debug", "quiet"]);
    assert!(m(&clap::Arg::new("debug")));
}

// ---------------------------------------------------------------------------
// TestExcludeFlags
// ---------------------------------------------------------------------------

#[test]
fn exclude_flags_excludes_matching_flag() {
    let m = exclude_flags(["token"]);
    assert!(!m(&clap::Arg::new("token")));
}

#[test]
fn exclude_flags_excludes_one_of_multiple_flags() {
    let m = exclude_flags(["token", "insecure", "force"]);
    assert!(!m(&clap::Arg::new("insecure")));
}

#[test]
fn exclude_flags_allows_non_matching_flag() {
    let m = exclude_flags(["token"]);
    assert!(m(&clap::Arg::new("namespace")));
}

#[test]
fn exclude_flags_allows_when_not_in_exclude_list() {
    let m = exclude_flags(["token", "insecure"]);
    assert!(m(&clap::Arg::new("namespace")));
}

#[test]
fn exclude_flags_empty_list_allows_all_flags() {
    let m = exclude_flags([] as [&str; 0]);
    assert!(m(&clap::Arg::new("token")));
}

#[test]
fn exclude_flags_exact_name_match_required() {
    let m = exclude_flags(["token"]);
    assert!(m(&clap::Arg::new("tokens")));
}

#[test]
fn exclude_flags_case_sensitive_matching() {
    let m = exclude_flags(["Token"]);
    assert!(m(&clap::Arg::new("token")));
}

#[test]
fn exclude_flags_excludes_all_listed_flags() {
    let m = exclude_flags(["password", "secret", "api-key"]);
    assert!(!m(&clap::Arg::new("secret")));
}

// ---------------------------------------------------------------------------
// TestNoFlags
// ---------------------------------------------------------------------------

#[test]
fn no_flags_rejects_regular_flag() {
    let m = no_flags();
    assert!(!m(&clap::Arg::new("namespace")));
}

#[test]
fn no_flags_rejects_verbose_flag() {
    let m = no_flags();
    assert!(!m(&clap::Arg::new("verbose")));
}

#[test]
fn no_flags_rejects_output_flag() {
    let m = no_flags();
    assert!(!m(&clap::Arg::new("output")));
}

#[test]
fn no_flags_rejects_empty_flag_name() {
    let m = no_flags();
    assert!(!m(&clap::Arg::new("")));
}

#[test]
fn no_flags_rejects_special_character_flag() {
    let m = no_flags();
    assert!(!m(&clap::Arg::new("@special")));
}

#[test]
fn no_flags_rejects_long_flag_name() {
    let m = no_flags();
    assert!(!m(&clap::Arg::new(
        "very-long-flag-name-that-should-still-be-rejected"
    )));
}
