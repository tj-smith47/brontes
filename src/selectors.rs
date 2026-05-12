//! Built-in factory functions for [`CmdMatcher`] and [`FlagMatcher`].
//!
//! Every function in this module returns an `Arc`-wrapped closure that
//! implements a common matching strategy. Callers compose them inside
//! [`crate::Selector`] fields:
//!
//! ```rust
//! use brontes::{Selector, selectors};
//!
//! let sel = Selector {
//!     cmd:  Some(selectors::allow_cmds(["kubectl get", "helm list"])),
//!     local_flag: Some(selectors::exclude_flags(["token", "password"])),
//!     ..Default::default()
//! };
//! ```
//!
//! # Introspectability cooperation
//!
//! The tool-validation layer (responsible for checking that every `cmd_path`
//! named in a [`crate::Config`] actually exists in the walked command tree)
//! cannot peer inside an opaque `Arc<dyn Fn>`. The built-in factories
//! cooperate by registering the strings they capture in a process-global
//! side-table keyed by the `Arc`'s data-pointer identity. The validator can
//! call `lookup` on a matcher to recover the original arguments and emit
//! actionable warnings when a configured path matches nothing in the tree.
//!
//! # Memory note
//!
//! Each factory call inserts one entry into the `MATCHER_REGISTRY` static.
//! Entries are never reclaimed even after the owning `Arc` is dropped (a drop
//! hook on a trait-object `Arc` would require unsafe code). This is a
//! deliberate trade-off: CLI tools build their [`crate::Config`] once at
//! startup, so the total number of entries is bounded by the number of
//! matchers in use — typically a dozen or fewer. If you are building a
//! long-running application that creates matchers in a loop, be aware of this
//! soft leak.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use crate::selector::{CmdMatcher, FlagMatcher};

// ---------------------------------------------------------------------------
// Side-table for introspectability (§2.7)
// ---------------------------------------------------------------------------

/// The kind of built-in matcher stored in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatcherKind {
    /// [`allow_cmds`] — exact path allow-list.
    AllowCmds,
    /// [`exclude_cmds`] — exact path deny-list.
    ExcludeCmds,
    /// [`allow_cmds_containing`] — substring allow-list.
    AllowCmdsContaining,
    /// [`exclude_cmds_containing`] — substring deny-list.
    ExcludeCmdsContaining,
    /// [`allow_flags`] — flag-name allow-list.
    AllowFlags,
    /// [`exclude_flags`] — flag-name deny-list.
    ExcludeFlags,
    /// [`no_flags`] — unconditional reject.
    NoFlags,
}

/// Captured constructor arguments for a built-in matcher.
#[derive(Debug, Clone)]
pub(crate) struct MatcherSpec {
    /// Which factory produced this matcher.
    pub kind: MatcherKind,
    /// The strings passed to the factory, in insertion order. **Not
    /// deduplicated** — the validator that consumes this list treats
    /// duplicates as separate entries (matching the runtime closure's
    /// behavior).
    pub args: Vec<String>,
}

/// Process-global registry mapping `Arc` data-pointer → [`MatcherSpec`].
///
/// Keyed by `Arc::as_ptr(arc) as *const () as usize` so that `Arc::clone`
/// (which shares the same allocation) continues to resolve through the same
/// entry.
//
// Mutex is sufficient: registry writes happen synchronously during
// factory construction (effectively at Config-build time, single-
// threaded for typical consumers); reads happen once during
// generate_tools and are not on any hot path. RwLock would add API
// surface (read/write call-sites) for no measurable benefit.
static MATCHER_REGISTRY: LazyLock<Mutex<HashMap<usize, MatcherSpec>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Insert a [`MatcherSpec`] for `arc` into the registry.
fn register<T: ?Sized>(arc: &Arc<T>, spec: MatcherSpec) {
    let key = Arc::as_ptr(arc).cast::<()>() as usize;
    MATCHER_REGISTRY
        .lock()
        .expect("MATCHER_REGISTRY mutex poisoned")
        .insert(key, spec);
}

/// Look up the [`MatcherSpec`] registered for `arc`, if any.
///
/// Returns `None` for matchers that were not produced by the built-in
/// factories (e.g., hand-written closures).
pub(crate) fn lookup<T: ?Sized>(arc: &Arc<T>) -> Option<MatcherSpec> {
    let key = Arc::as_ptr(arc).cast::<()>() as usize;
    MATCHER_REGISTRY
        .lock()
        .expect("MATCHER_REGISTRY mutex poisoned")
        .get(&key)
        .cloned()
}

// ---------------------------------------------------------------------------
// CmdMatcher factories
// ---------------------------------------------------------------------------

/// Returns a [`CmdMatcher`] that accepts commands whose space-joined path is
/// **exactly** one of `paths`.
///
/// An empty `paths` iterator produces a matcher that always returns `false`.
///
/// # Example
///
/// ```rust
/// use brontes::selectors;
///
/// let m = selectors::allow_cmds(["kubectl get", "helm list"]);
/// assert!(m("kubectl get"));
/// assert!(!m("kubectl get pods"));     // not an exact match
/// assert!(!m("kubectl"));
/// ```
#[must_use]
pub fn allow_cmds<I, S>(paths: I) -> CmdMatcher
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = paths.into_iter().map(Into::into).collect();
    let owned = args.clone();
    let arc: CmdMatcher = Arc::new(move |path: &str| owned.iter().any(|p| p == path));
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::AllowCmds,
            args,
        },
    );
    arc
}

/// Returns a [`CmdMatcher`] that accepts commands whose space-joined path is
/// **not** in `paths` (exact match).
///
/// An empty `paths` iterator produces a matcher that always returns `true`
/// (nothing is excluded).
///
/// # Example
///
/// ```rust
/// use brontes::selectors;
///
/// let m = selectors::exclude_cmds(["kubectl delete"]);
/// assert!(!m("kubectl delete"));
/// assert!(m("kubectl get"));
/// ```
#[must_use]
pub fn exclude_cmds<I, S>(paths: I) -> CmdMatcher
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = paths.into_iter().map(Into::into).collect();
    let owned = args.clone();
    let arc: CmdMatcher = Arc::new(move |path: &str| !owned.iter().any(|p| p == path));
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::ExcludeCmds,
            args,
        },
    );
    arc
}

/// Returns a [`CmdMatcher`] that accepts commands whose space-joined path
/// **contains** any of the listed substrings.
///
/// An empty `needles` iterator produces a matcher that always returns `false`.
///
/// # Example
///
/// ```rust
/// use brontes::selectors;
///
/// let m = selectors::allow_cmds_containing(["get", "list"]);
/// assert!(m("kubectl get pods"));
/// assert!(m("helm list"));
/// assert!(!m("kubectl delete pod"));
/// ```
#[must_use]
pub fn allow_cmds_containing<I, S>(needles: I) -> CmdMatcher
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = needles.into_iter().map(Into::into).collect();
    let owned = args.clone();
    let arc: CmdMatcher =
        Arc::new(move |path: &str| owned.iter().any(|n| path.contains(n.as_str())));
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::AllowCmdsContaining,
            args,
        },
    );
    arc
}

/// Returns a [`CmdMatcher`] that accepts commands whose space-joined path does
/// **not contain** any of the listed substrings.
///
/// An empty `needles` iterator produces a matcher that always returns `true`
/// (nothing is excluded).
///
/// # Example
///
/// ```rust
/// use brontes::selectors;
///
/// let m = selectors::exclude_cmds_containing(["delete", "remove"]);
/// assert!(!m("kubectl delete pod"));
/// assert!(m("kubectl get pods"));
/// ```
#[must_use]
pub fn exclude_cmds_containing<I, S>(needles: I) -> CmdMatcher
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = needles.into_iter().map(Into::into).collect();
    let owned = args.clone();
    let arc: CmdMatcher =
        Arc::new(move |path: &str| !owned.iter().any(|n| path.contains(n.as_str())));
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::ExcludeCmdsContaining,
            args,
        },
    );
    arc
}

// ---------------------------------------------------------------------------
// FlagMatcher factories
// ---------------------------------------------------------------------------

/// Returns a [`FlagMatcher`] that accepts flags whose `get_id()` string is in
/// `names`.
///
/// An empty `names` iterator produces a matcher that always returns `false`.
///
/// # Example
///
/// ```rust
/// use brontes::selectors;
///
/// let m = selectors::allow_flags(["namespace", "output"]);
/// assert!(m(&clap::Arg::new("namespace")));
/// assert!(!m(&clap::Arg::new("kubeconfig")));
/// ```
#[must_use]
pub fn allow_flags<I, S>(names: I) -> FlagMatcher
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = names.into_iter().map(Into::into).collect();
    let owned = args.clone();
    let arc: FlagMatcher =
        Arc::new(move |arg: &clap::Arg| owned.iter().any(|n| n == arg.get_id().as_str()));
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::AllowFlags,
            args,
        },
    );
    arc
}

/// Returns a [`FlagMatcher`] that accepts flags whose `get_id()` string is
/// **not** in `names`.
///
/// An empty `names` iterator produces a matcher that always returns `true`
/// (nothing is excluded).
///
/// # Example
///
/// ```rust
/// use brontes::selectors;
///
/// let m = selectors::exclude_flags(["token", "password"]);
/// assert!(!m(&clap::Arg::new("token")));
/// assert!(m(&clap::Arg::new("namespace")));
/// ```
#[must_use]
pub fn exclude_flags<I, S>(names: I) -> FlagMatcher
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = names.into_iter().map(Into::into).collect();
    let owned = args.clone();
    let arc: FlagMatcher =
        Arc::new(move |arg: &clap::Arg| !owned.iter().any(|n| n == arg.get_id().as_str()));
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::ExcludeFlags,
            args,
        },
    );
    arc
}

/// Returns a [`FlagMatcher`] that **always returns `false`**, unconditionally
/// excluding every flag.
///
/// Useful as a `local_flag` or `inherited_flag` override when a command's
/// flags should be entirely hidden from MCP callers.
///
/// # Example
///
/// ```rust
/// use brontes::{Selector, selectors};
///
/// let sel = Selector {
///     local_flag: Some(selectors::no_flags()),
///     ..Default::default()
/// };
/// ```
#[must_use]
pub fn no_flags() -> FlagMatcher {
    let arc: FlagMatcher = Arc::new(|_arg: &clap::Arg| false);
    register(
        &arc,
        MatcherSpec {
            kind: MatcherKind::NoFlags,
            args: Vec::new(),
        },
    );
    arc
}

// ---------------------------------------------------------------------------
// Unit tests — side-table registration
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_cmds_registers_spec() {
        let m = allow_cmds(["a", "b"]);
        let spec = lookup(&m).expect("allow_cmds must register a spec");
        assert_eq!(spec.kind, MatcherKind::AllowCmds);
        assert_eq!(spec.args, vec!["a", "b"]);
    }

    #[test]
    fn exclude_cmds_registers_spec() {
        let m = exclude_cmds(["x", "y", "z"]);
        let spec = lookup(&m).expect("exclude_cmds must register a spec");
        assert_eq!(spec.kind, MatcherKind::ExcludeCmds);
        assert_eq!(spec.args, vec!["x", "y", "z"]);
    }

    #[test]
    fn allow_cmds_containing_registers_spec() {
        let m = allow_cmds_containing(["get", "list"]);
        let spec = lookup(&m).expect("allow_cmds_containing must register a spec");
        assert_eq!(spec.kind, MatcherKind::AllowCmdsContaining);
        assert_eq!(spec.args, vec!["get", "list"]);
    }

    #[test]
    fn exclude_cmds_containing_registers_spec() {
        let m = exclude_cmds_containing(["delete"]);
        let spec = lookup(&m).expect("exclude_cmds_containing must register a spec");
        assert_eq!(spec.kind, MatcherKind::ExcludeCmdsContaining);
        assert_eq!(spec.args, vec!["delete"]);
    }

    #[test]
    fn allow_flags_registers_spec() {
        let m = allow_flags(["namespace", "output"]);
        let spec = lookup(&m).expect("allow_flags must register a spec");
        assert_eq!(spec.kind, MatcherKind::AllowFlags);
        assert_eq!(spec.args, vec!["namespace", "output"]);
    }

    #[test]
    fn exclude_flags_registers_spec() {
        let m = exclude_flags(["token", "password"]);
        let spec = lookup(&m).expect("exclude_flags must register a spec");
        assert_eq!(spec.kind, MatcherKind::ExcludeFlags);
        assert_eq!(spec.args, vec!["token", "password"]);
    }

    #[test]
    fn no_flags_registers_spec() {
        let m = no_flags();
        let spec = lookup(&m).expect("no_flags must register a spec");
        assert_eq!(spec.kind, MatcherKind::NoFlags);
        assert!(spec.args.is_empty(), "no_flags args must be empty");
    }

    #[test]
    fn arc_clone_shares_registry_entry() {
        let m = allow_cmds(["shared"]);
        let m2 = Arc::clone(&m);
        // Both the original and the clone must resolve to the same spec.
        let spec1 = lookup(&m).expect("original must have spec");
        let spec2 = lookup(&m2).expect("clone must have spec");
        assert_eq!(spec1.kind, spec2.kind);
        assert_eq!(spec1.args, spec2.args);
    }

    #[test]
    fn hand_written_closure_returns_none() {
        let m: CmdMatcher = Arc::new(|_path: &str| true);
        assert!(
            lookup(&m).is_none(),
            "hand-written closures must not appear in the registry"
        );
    }
}
