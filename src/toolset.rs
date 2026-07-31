//! Groups and the tool filter: which of a CLI's commands become tools.
//!
//! A CLI with ninety subcommands makes a poor MCP server. Every tool
//! descriptor is tokens in the model's context before a single call is made,
//! and a client that must choose among ninety tools chooses worse than one
//! choosing among five. The two halves of the answer live here:
//!
//! - the **developer** names bundles of related commands with [`Group`], so
//!   the useful subsets have names instead of having to be spelled out;
//! - the **end user** picks what a given server exposes with [`ToolFilter`],
//!   driven by `--group` / `--command` / `--tool` (and their `--hide-`
//!   counterparts) on `mcp start`, `mcp stream`, and `mcp tools`.
//!
//! ```rust
//! use brontes::Config;
//!
//! // Developer, in the CLI's own source:
//! let cfg = Config::default()
//!     .group("release", ["release", "publish"])
//!     .group_description("release", "Cut, sign, and publish a release");
//! ```
//!
//! ```text
//! # End user, at launch:
//! $ anodizer mcp start --group release
//! $ anodizer mcp start --command release --hide-tool anodizer_release_notes
//! $ anodizer mcp tools --groups
//! ```
//!
//! # Why this is a launch-time decision
//!
//! MCP `2026-07-28` removed protocol sessions, and with them the notion of a
//! listing that varies per connection: `tools/list` answers the same thing for
//! every caller of a given server. Trimming therefore belongs to the process,
//! not to the request — one server, one tool list. Two different subsets means
//! two servers, which is exactly what an editor's MCP config expresses anyway
//! (one entry per launch command).

use std::collections::{BTreeMap, BTreeSet};

/// A named bundle of commands, defined by the CLI's author.
///
/// Membership covers a command and everything under it: naming `"release"`
/// also takes `"release notes"`. That way the group tracks the command tree
/// instead of drifting from it whenever a subcommand is added.
///
/// Paths are written relative to the CLI's own name, which brontes supplies —
/// `"release"` and `"anodizer release"` name the same command.
///
/// Construct through [`crate::Config::group`] and
/// [`crate::Config::group_description`] rather than by struct literal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Group {
    /// Command paths in the group, each covering its own subtree.
    pub commands: Vec<String>,
    /// One-line summary shown by `mcp tools --groups`.
    pub description: Option<String>,
}

/// Which of the walked commands a server exposes as tools.
///
/// Empty — the default — means every command that survives the safety filters
/// and the [`Selector`](crate::Selector) pass. Otherwise:
///
/// 1. if any of `groups` / `commands` / `tools` is non-empty, the tool list
///    starts as the **union** of what they name;
/// 2. everything named by `hidden_groups` / `hidden_commands` / `hidden_tools`
///    is then removed.
///
/// Hiding always wins over exposing, so `--group release --hide-command
/// "release notes"` reads the way it looks.
///
/// Every name, exposed or hidden, has to be one the CLI actually has; a typo
/// is a startup error rather than a silent no-op. Exposing entries are held to
/// more: one that resolves to a real command the server would not serve anyway
/// — deprecated, hidden, or dropped by a [`Selector`](crate::Selector) — also
/// fails, because a server quietly missing a tool the caller asked for is
/// indistinguishable from a CLI that never had it. Hiding such a command is
/// allowed, since the result is what was asked for either way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolFilter {
    /// Group names to expose.
    pub groups: BTreeSet<String>,
    /// Command paths to expose, each covering its own subtree.
    pub commands: BTreeSet<String>,
    /// MCP tool names to expose, matched exactly.
    pub tools: BTreeSet<String>,
    /// Group names to remove.
    pub hidden_groups: BTreeSet<String>,
    /// Command paths to remove, each covering its own subtree.
    pub hidden_commands: BTreeSet<String>,
    /// MCP tool names to remove, matched exactly.
    pub hidden_tools: BTreeSet<String>,
}

impl ToolFilter {
    /// Whether this filter would change anything.
    ///
    /// A no-op filter skips the "selected nothing" check entirely: a CLI with
    /// no commands is its own (already-reported) problem, not a trim failure.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
            && self.commands.is_empty()
            && self.tools.is_empty()
            && self.hidden_groups.is_empty()
            && self.hidden_commands.is_empty()
            && self.hidden_tools.is_empty()
    }

    /// Whether any of the three exposing sets is populated.
    ///
    /// When none is, the filter subtracts from the full tool list instead of
    /// building up from an empty one.
    #[must_use]
    pub fn selects(&self) -> bool {
        !(self.groups.is_empty() && self.commands.is_empty() && self.tools.is_empty())
    }

    /// Merge `other`'s entries into this filter.
    ///
    /// Used to fold the CLI flags parsed at launch onto whatever the CLI's
    /// author already pinned in [`Config`](crate::Config), so a developer
    /// default and an end-user choice compose rather than one silently
    /// replacing the other.
    #[must_use]
    pub fn merged_with(mut self, other: &Self) -> Self {
        self.groups.extend(other.groups.iter().cloned());
        self.commands.extend(other.commands.iter().cloned());
        self.tools.extend(other.tools.iter().cloned());
        self.hidden_groups
            .extend(other.hidden_groups.iter().cloned());
        self.hidden_commands
            .extend(other.hidden_commands.iter().cloned());
        self.hidden_tools.extend(other.hidden_tools.iter().cloned());
        self
    }

    /// Expand every group reference into the command paths behind it.
    ///
    /// Returns `(exposed_commands, hidden_commands)` — the group sets folded
    /// into the path sets, so the matcher only ever deals in paths and tool
    /// names.
    fn resolve_groups(
        &self,
        groups: &BTreeMap<String, Group>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let expand = |names: &BTreeSet<String>, base: &BTreeSet<String>| {
            let mut out = base.clone();
            for name in names {
                if let Some(group) = groups.get(name) {
                    out.extend(group.commands.iter().cloned());
                }
            }
            out
        };
        (
            expand(&self.groups, &self.commands),
            expand(&self.hidden_groups, &self.hidden_commands),
        )
    }

    /// Whether the tool at `command_path`, named `tool_name`, survives.
    pub(crate) fn admits(
        &self,
        groups: &BTreeMap<String, Group>,
        command_path: &str,
        tool_name: &str,
    ) -> bool {
        let (exposed_paths, hidden_paths) = self.resolve_groups(groups);

        if covers_any(&hidden_paths, command_path) || self.hidden_tools.contains(tool_name) {
            return false;
        }
        if !self.selects() {
            return true;
        }
        covers_any(&exposed_paths, command_path) || self.tools.contains(tool_name)
    }

    /// Rewrite every command path through [`absolute`], so the matcher only
    /// ever sees paths that start at the root.
    pub(crate) fn normalized(&self, root: &str) -> Self {
        let resolve = |paths: &BTreeSet<String>| paths.iter().map(|p| absolute(p, root)).collect();
        Self {
            groups: self.groups.clone(),
            commands: resolve(&self.commands),
            tools: self.tools.clone(),
            hidden_groups: self.hidden_groups.clone(),
            hidden_commands: resolve(&self.hidden_commands),
            hidden_tools: self.hidden_tools.clone(),
        }
    }

    /// Every command path this filter names, exposed or hidden, with the
    /// groups already expanded. Used to report the ones that match nothing.
    pub(crate) fn named_paths(&self, groups: &BTreeMap<String, Group>) -> BTreeSet<String> {
        let (exposed, hidden) = self.resolve_groups(groups);
        exposed.into_iter().chain(hidden).collect()
    }

    /// Every group name this filter references, exposed or hidden.
    pub(crate) fn named_groups(&self) -> impl Iterator<Item = &String> {
        self.groups.iter().chain(self.hidden_groups.iter())
    }

    /// Every tool name this filter references, exposed or hidden.
    pub(crate) fn named_tools(&self) -> impl Iterator<Item = &String> {
        self.tools.iter().chain(self.hidden_tools.iter())
    }
}

/// Whether `entry` names `path` or an ancestor of it.
///
/// Matching is on whole path segments, so `"anodizer rel"` does not cover
/// `"anodizer release"` — a near-miss in a launch flag has to fail loudly
/// rather than quietly select a neighbour.
pub fn covers(entry: &str, path: &str) -> bool {
    path == entry
        || path
            .strip_prefix(entry)
            .is_some_and(|rest| rest.starts_with(' '))
}

/// Resolve a selection path against the root command's name.
///
/// The root name is the one segment brontes can always derive — it is the
/// binary the user just invoked — so requiring it in `--command "anodizer
/// release"` is asking them to retype it. A path whose first segment is
/// already the root is absolute and returned unchanged; anything else is
/// taken as relative to the root. `"release notes"` and `"anodizer release
/// notes"` therefore name the same command.
///
/// A CLI with a subcommand named after itself addresses it with the root
/// spelled twice (`"anodizer anodizer"`), since the leading segment is read
/// as the root first.
/// Only a well-formed path is resolved. One carrying stray whitespace is
/// returned untouched, so the error that follows names what the caller wrote
/// rather than a resolved form they never typed.
pub fn absolute(entry: &str, root: &str) -> String {
    let malformed = entry.is_empty() || entry != entry.trim() || entry.contains("  ");
    if malformed || covers(root, entry) {
        entry.to_owned()
    } else {
        format!("{root} {entry}")
    }
}

/// Whether any entry in `entries` covers `path`.
fn covers_any(entries: &BTreeSet<String>, path: &str) -> bool {
    entries.iter().any(|e| covers(e, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups() -> BTreeMap<String, Group> {
        let mut map = BTreeMap::new();
        map.insert(
            "release".to_owned(),
            Group {
                commands: vec!["cli release".to_owned(), "cli publish".to_owned()],
                description: Some("Ship it".to_owned()),
            },
        );
        map
    }

    #[test]
    fn covers_matches_whole_segments_only() {
        assert!(covers("cli release", "cli release"));
        assert!(covers("cli release", "cli release notes"));
        assert!(!covers("cli rel", "cli release"));
        assert!(!covers("cli release", "cli"));
        assert!(!covers("cli release", "cli releases"));
    }

    #[test]
    fn absolute_resolves_a_relative_path_and_leaves_the_rest_alone() {
        assert_eq!(absolute("release", "cli"), "cli release");
        assert_eq!(absolute("release notes", "cli"), "cli release notes");
        assert_eq!(absolute("cli release", "cli"), "cli release");
        assert_eq!(absolute("cli", "cli"), "cli");
        // A malformed path stays verbatim so the error names what was written.
        assert_eq!(absolute("", "cli"), "");
        assert_eq!(absolute(" cli release ", "cli"), " cli release ");
        assert_eq!(absolute("cli  release", "cli"), "cli  release");
    }

    #[test]
    fn an_empty_filter_admits_everything() {
        let filter = ToolFilter::default();
        assert!(filter.is_empty());
        assert!(filter.admits(&groups(), "cli anything", "cli_anything"));
    }

    #[test]
    fn a_group_admits_its_members_and_their_subtrees() {
        let mut filter = ToolFilter::default();
        filter.groups.insert("release".to_owned());

        assert!(filter.admits(&groups(), "cli release", "cli_release"));
        assert!(filter.admits(&groups(), "cli release notes", "cli_release_notes"));
        assert!(filter.admits(&groups(), "cli publish", "cli_publish"));
        assert!(!filter.admits(&groups(), "cli secrets", "cli_secrets"));
    }

    #[test]
    fn hiding_beats_exposing() {
        let mut filter = ToolFilter::default();
        filter.groups.insert("release".to_owned());
        filter
            .hidden_commands
            .insert("cli release notes".to_owned());

        assert!(filter.admits(&groups(), "cli release", "cli_release"));
        assert!(!filter.admits(&groups(), "cli release notes", "cli_release_notes"));
    }

    #[test]
    fn hiding_alone_subtracts_from_the_full_list() {
        let mut filter = ToolFilter::default();
        filter.hidden_tools.insert("cli_secrets".to_owned());

        assert!(!filter.selects());
        assert!(filter.admits(&groups(), "cli release", "cli_release"));
        assert!(!filter.admits(&groups(), "cli secrets", "cli_secrets"));
    }

    #[test]
    fn a_tool_name_selects_exactly_one_tool() {
        let mut filter = ToolFilter::default();
        filter.tools.insert("cli_release_notes".to_owned());

        assert!(filter.admits(&groups(), "cli release notes", "cli_release_notes"));
        assert!(
            !filter.admits(&groups(), "cli release", "cli_release"),
            "a tool name must not drag in the parent command"
        );
    }

    #[test]
    fn merging_unions_both_sides() {
        let mut developer = ToolFilter::default();
        developer.hidden_commands.insert("cli secrets".to_owned());
        let mut end_user = ToolFilter::default();
        end_user.groups.insert("release".to_owned());

        let merged = developer.merged_with(&end_user);
        assert!(merged.admits(&groups(), "cli release", "cli_release"));
        assert!(
            !merged.admits(&groups(), "cli secrets", "cli_secrets"),
            "an end-user selection must not undo a developer's hide"
        );
    }
}
