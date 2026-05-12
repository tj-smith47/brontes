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
//! Each entry holds a `Weak` reference to the produced `Arc`, so dropped
//! matchers are reaped lazily: the next `lookup` whose key collides with a
//! freed slot detects the dead entry (via `Weak::strong_count() == 0`),
//! evicts it, and reports a miss. CLI tools that build their
//! [`crate::Config`] once at startup pay essentially no overhead; long-
//! running applications that churn matchers in a loop still see bounded
//! growth because keys are reused by the allocator and stale entries are
//! evicted on contact.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use crate::selector::{CmdMatcher, FlagMatcher};

// ---------------------------------------------------------------------------
// Side-table mapping each registered matcher's `Arc` pointer back to its
// factory inputs, so callers (e.g. `generate_tools`) can validate paths and
// flag names against the original configuration after type-erasure.
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

/// A liveness probe boxed alongside each [`MatcherSpec`] entry.
///
/// The closure captures a [`std::sync::Weak`] reference to the registered
/// `Arc` and returns `true` while at least one strong clone is alive. A
/// trait-object alias is used so the registry `HashMap` can store a uniform
/// value type even though each entry is bound to a different concrete `T`
/// (`CmdMatcher` vs `FlagMatcher`).
type LivenessCheck = Box<dyn Fn() -> bool + Send + Sync>;

/// Process-global registry mapping `Arc` data-pointer → ([`LivenessCheck`],
/// [`MatcherSpec`]).
///
/// Keyed by `Arc::as_ptr(arc) as *const () as usize` so that `Arc::clone`
/// (which shares the same allocation) continues to resolve through the same
/// entry.
///
/// Entries are weak-referenced: each value carries a [`LivenessCheck`] that
/// reports whether the original `Arc` is still alive. A [`lookup`] whose key
/// matches a stale entry (original `Arc` and every clone dropped, slot
/// potentially reused by the allocator) returns `None` and evicts the stale
/// entry, which both prevents a dropped factory matcher from being
/// misidentified as a coincidentally-reused hand-written closure and keeps
/// the registry's footprint bounded over time.
//
// Mutex is sufficient: registry writes happen synchronously during
// factory construction (effectively at Config-build time, single-
// threaded for typical consumers); reads happen once during
// generate_tools and are not on any hot path. RwLock would add API
// surface (read/write call-sites) for no measurable benefit.
static MATCHER_REGISTRY: LazyLock<Mutex<HashMap<usize, (LivenessCheck, MatcherSpec)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Insert a [`MatcherSpec`] for `arc` into the registry.
///
/// Stores a [`std::sync::Weak`]-backed [`LivenessCheck`] alongside the spec
/// so [`lookup`] can detect and evict entries whose underlying `Arc` has
/// been dropped (and whose pointer key may have been reused by the
/// allocator).
fn register<T: ?Sized + Send + Sync + 'static>(arc: &Arc<T>, spec: MatcherSpec) {
    let key = Arc::as_ptr(arc).cast::<()>() as usize;
    let weak = Arc::downgrade(arc);
    // `strong_count() > 0` avoids the temporary Arc allocation that
    // `Weak::upgrade()` would require on every lookup. The read is
    // non-atomic per `std::sync::Weak::strong_count`, but the registry is
    // mutated only under `MATCHER_REGISTRY`'s `Mutex` and consumers build
    // their `Config` from a single thread at startup, so the racy view is
    // not observable in practice.
    let alive: LivenessCheck = Box::new(move || weak.strong_count() > 0);
    MATCHER_REGISTRY
        .lock()
        .expect("MATCHER_REGISTRY mutex poisoned")
        .insert(key, (alive, spec));
}

/// Look up the [`MatcherSpec`] registered for `arc`, if any.
///
/// Returns `None` for matchers that were not produced by the built-in
/// factories (e.g., hand-written closures) **and** for stale entries whose
/// underlying `Arc` has been dropped — the latter case can occur when the
/// allocator reuses a freed slot for an unrelated `Arc`, causing the
/// pointer keys to collide. Stale entries detected this way are evicted in
/// place so the registry shrinks as dead pointers are touched.
pub(crate) fn lookup<T: ?Sized>(arc: &Arc<T>) -> Option<MatcherSpec> {
    let key = Arc::as_ptr(arc).cast::<()>() as usize;
    let mut registry = MATCHER_REGISTRY
        .lock()
        .expect("MATCHER_REGISTRY mutex poisoned");
    if let Some((alive, spec)) = registry.get(&key) {
        if alive() {
            return Some(spec.clone());
        }
        // Stale entry — the original `Arc` was dropped; this key is from a
        // coincidentally-reused slot. Evict and report a miss.
        registry.remove(&key);
    }
    None
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

    /// Exercises the pointer-reuse path that previously caused
    /// [`hand_written_closure_returns_none`] to flake.
    ///
    /// Creates and immediately drops many factory-produced `Arc`s, then
    /// constructs a hand-written `Arc<dyn Fn>` and verifies `lookup`
    /// returns `None`. The allocator is free to (and frequently does)
    /// place the hand-written closure on a slot that was occupied by a
    /// just-dropped factory closure; the liveness check must catch that
    /// case and evict the stale entry rather than misreport the
    /// hand-written closure as carrying a factory spec.
    ///
    /// The test cannot *force* pointer reuse to happen on any given run
    /// (the allocator is opaque), but it heavily encourages it by
    /// allocating and freeing in a tight loop. Combined with running the
    /// surrounding test suite many times under thread interleaving, this
    /// pins the regression.
    #[test]
    fn dropped_factory_arc_does_not_alias_hand_written_closure() {
        for _ in 0..1024 {
            // Each call allocates a fresh Arc, registers it, and drops it
            // at the end of the statement — its slot is now free for
            // reuse.
            let _ = allow_cmds(["transient"]);
        }
        let hand_written: CmdMatcher = Arc::new(|_path: &str| true);
        assert!(
            lookup(&hand_written).is_none(),
            "hand-written closure must not inherit a stale factory spec \
             via pointer reuse",
        );
    }

    /// Verifies a live `Arc`'s registry entry survives unrelated
    /// allocator churn — eviction must only fire on stale keys, not on
    /// collateral collisions.
    #[test]
    fn live_arc_survives_intermediate_factory_churn() {
        let m = allow_cmds(["sticky"]);
        // 256 churn iterations — enough to reuse the slot multiple times
        // in practice without bloating CI time.
        for _ in 0..256 {
            let _ = exclude_flags(["churn"]);
        }
        let spec = lookup(&m).expect("live Arc must still resolve");
        assert_eq!(spec.kind, MatcherKind::AllowCmds);
        assert_eq!(spec.args, vec!["sticky"]);
    }
}
