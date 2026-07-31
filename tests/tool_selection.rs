//! Groups and the tool filter: which commands a given server exposes.
//!
//! An MCP server's tool list is a context cost paid before any work happens,
//! so a CLI with a large command tree needs a way to serve a subset. These
//! tests cover both halves of that:
//!
//! - the developer names bundles with `Config::group`;
//! - the end user picks with `--group` / `--command` / `--tool` and the
//!   `--hide-` counterparts on `mcp start`, `mcp stream`, and `mcp tools`.
//!
//! The property worth defending hardest is that a selection which matches
//! nothing is a startup error. A trimmed server that quietly exposes the wrong
//! set — or nothing at all — is indistinguishable from a CLI that never had
//! those commands, and the client has no way to tell.

use clap::Command;

use brontes::{Config, generate_tools};

/// A CLI with a nested subtree, two unrelated commands, and a command whose
/// name is a prefix of another so segment-boundary matching is exercised.
fn cli() -> Command {
    Command::new("demo")
        .version("0.1.0")
        .subcommand(
            Command::new("release")
                .about("Cut a release")
                .subcommand(Command::new("notes").about("Render release notes")),
        )
        .subcommand(Command::new("releases").about("List past releases"))
        .subcommand(Command::new("publish").about("Publish artifacts"))
        .subcommand(
            Command::new("secrets")
                .about("Manage secrets")
                .subcommand(Command::new("get").about("Read a secret")),
        )
}

fn names(cfg: &Config) -> Vec<String> {
    generate_tools(&cli(), cfg)
        .expect("the tool list must build")
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

fn err(cfg: &Config) -> String {
    generate_tools(&cli(), cfg)
        .expect_err("this configuration must be rejected")
        .to_string()
}

fn grouped() -> Config {
    Config::default()
        .group("release", ["demo release", "demo publish"])
        .group_description("release", "Cut and publish a release")
        .group("secrets", ["demo secrets"])
}

// ─────────────────────────────────────────────────────────────────────────────
// Baseline
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn defining_groups_does_not_change_the_tool_list() {
    // Grouping is a vocabulary for selection, not a selection. A developer who
    // adds groups to a shipped CLI must not silently change what it serves.
    assert_eq!(names(&Config::default()), names(&grouped()));
}

// ─────────────────────────────────────────────────────────────────────────────
// Selecting
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_group_selects_its_members_and_their_subtrees() {
    let got = names(&grouped().expose_group("release"));
    assert_eq!(
        got,
        vec!["demo_release", "demo_release_notes", "demo_publish"],
        "a group naming a parent command must carry the parent's subcommands \
         with it, so adding a subcommand does not silently fall out of the group"
    );
}

#[test]
fn a_command_selects_its_own_subtree_only() {
    let got = names(&Config::default().expose_command("demo release"));
    assert_eq!(got, vec!["demo_release", "demo_release_notes"]);
}

#[test]
fn command_selection_respects_segment_boundaries() {
    // "demo release" must not drag in "demo releases": a near-miss in a launch
    // flag has to be visible, and silently serving the neighbouring command is
    // the worst possible way for it not to be.
    let got = names(&Config::default().expose_command("demo release"));
    assert!(
        !got.contains(&"demo_releases".to_string()),
        "prefix matching must stop at a path segment, got {got:?}"
    );
}

#[test]
fn a_tool_name_selects_exactly_one_tool() {
    let got = names(&Config::default().expose_tool("demo_release"));
    assert_eq!(
        got,
        vec!["demo_release"],
        "selection by tool name is the surgical form and must not take the subtree"
    );
}

#[test]
fn selections_union() {
    let got = names(
        &grouped()
            .expose_group("secrets")
            .expose_command("demo publish")
            .expose_tool("demo_release"),
    );
    assert_eq!(
        got,
        vec![
            "demo_release",
            "demo_publish",
            "demo_secrets",
            "demo_secrets_get"
        ]
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Hiding
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hiding_alone_subtracts_from_the_full_tool_list() {
    let got = names(&grouped().hide_group("secrets"));
    assert_eq!(
        got,
        vec![
            "demo",
            "demo_release",
            "demo_release_notes",
            "demo_releases",
            "demo_publish"
        ],
        "with nothing selected, a hide must trim the full list rather than \
         start a new one"
    );
}

#[test]
fn hiding_beats_selecting_for_the_same_command() {
    let got = names(
        &grouped()
            .expose_group("release")
            .hide_tool("demo_release_notes"),
    );
    assert_eq!(got, vec!["demo_release", "demo_publish"]);
}

#[test]
fn a_developer_hide_survives_an_end_user_selection() {
    // The composition rule that matters for a CLI author: pinning a hide in
    // Config must not be undoable from the launch line.
    let cfg = brontes::__test_internal::apply_selection_flags(
        grouped().hide_command("demo secrets"),
        &brontes::__test_internal::start_subcommand()
            .try_get_matches_from(["start", "--command", "demo secrets"])
            .expect("parses"),
    );
    let err = generate_tools(&cli(), &cfg).expect_err("nothing survives the hide");
    assert!(
        err.to_string().contains("matches no commands"),
        "the trim collapsing to nothing must be reported, got {err}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Loud failure
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn an_unknown_group_names_the_groups_that_exist() {
    let msg = err(&grouped().expose_group("relase"));
    assert!(
        msg.contains("relase"),
        "the typo must be quoted back: {msg}"
    );
    assert!(
        msg.contains("release") && msg.contains("secrets"),
        "the report must list the names that would have worked: {msg}"
    );
}

#[test]
fn an_unknown_group_says_so_when_the_cli_defines_none() {
    let msg = err(&Config::default().expose_group("release"));
    assert!(
        msg.contains("defines no groups"),
        "a CLI with no groups must say that rather than list an empty set: {msg}"
    );
}

#[test]
fn an_unknown_command_is_rejected_rather_than_ignored() {
    let msg = err(&Config::default().expose_command("demo deploy"));
    assert!(
        msg.contains("demo deploy"),
        "the unmatched path must be named: {msg}"
    );
}

#[test]
fn an_unknown_tool_name_lists_the_names_that_exist() {
    let msg = err(&Config::default().expose_tool("demo_relase"));
    assert!(
        msg.contains("demo_relase"),
        "the typo must be quoted: {msg}"
    );
    assert!(
        msg.contains("demo_release"),
        "the report must list real tool names: {msg}"
    );
}

#[test]
fn a_group_whose_member_no_longer_exists_fails_the_build() {
    // The drift case: a subcommand is renamed and the group still names the old
    // path. Failing here is what keeps a group from silently shrinking.
    let msg = err(&Config::default().group("release", ["demo relase"]));
    assert!(
        msg.contains("group \"release\"") && msg.contains("demo relase"),
        "the report must name both the group and the dead path: {msg}"
    );
}

#[test]
fn a_selection_that_matches_nothing_is_not_an_empty_server() {
    // `demo secrets` exists and `demo publish` exists, but hiding the one that
    // was selected leaves nothing. An empty tool list is a working server that
    // can do nothing, which is the failure a client cannot diagnose.
    let msg = err(&Config::default()
        .expose_command("demo publish")
        .hide_command("demo publish"));
    assert!(
        msg.contains("matches no commands"),
        "an empty selection must be reported: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The launch flags
// ─────────────────────────────────────────────────────────────────────────────

/// Every `mcp` leaf that decides a tool list must carry the same six flags —
/// `mcp tools` is how a user previews what `mcp start` would serve, so a flag
/// present on one and missing from another makes the preview a lie.
#[test]
fn all_three_leaves_carry_the_same_selection_flags() {
    let expected = [
        "group",
        "command",
        "tool",
        "hide-group",
        "hide-command",
        "hide-tool",
    ];
    for (label, cmd) in [
        ("start", brontes::__test_internal::start_subcommand()),
        ("stream", brontes::__test_internal::stream_subcommand()),
        ("tools", brontes::__test_internal::tools_subcommand()),
    ] {
        let present: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        for flag in expected {
            assert!(
                present.contains(&flag),
                "mcp {label} is missing --{flag}: {present:?}"
            );
        }
    }
}

#[test]
fn the_launch_flags_produce_the_same_list_as_the_builders() {
    // The flags and the `Config` builders are two doors into one mechanism;
    // this is the check that they stay one mechanism.
    let from_flags = brontes::__test_internal::apply_selection_flags(
        grouped(),
        &brontes::__test_internal::start_subcommand()
            .try_get_matches_from([
                "start",
                "--group",
                "release",
                "--hide-tool",
                "demo_release_notes",
            ])
            .expect("parses"),
    );
    let from_builders = grouped()
        .expose_group("release")
        .hide_tool("demo_release_notes");

    assert_eq!(names(&from_flags), names(&from_builders));
    assert_eq!(names(&from_flags), vec!["demo_release", "demo_publish"]);
}

#[test]
fn an_editor_install_writes_the_selection_into_the_launch_argv() {
    // Trimming is a launch-time decision, so a selection made at install time
    // has to reach the argv the editor spawns — otherwise the tool list the
    // user chose lasts exactly as long as the install command.
    let matches = brontes::__test_internal::start_subcommand()
        .try_get_matches_from([
            "start",
            "--group",
            "release",
            "--group",
            "secrets",
            "--hide-command",
            "demo secrets",
        ])
        .expect("parses");

    let mut argv = vec!["mcp".to_string(), "start".to_string()];
    brontes::__test_internal::push_selection_flags(&mut argv, &matches);

    assert_eq!(
        argv,
        vec![
            "mcp",
            "start",
            "--group",
            "release",
            "--group",
            "secrets",
            "--hide-command",
            "demo secrets",
        ],
        "flags must round-trip in a stable order so re-running enable rewrites \
         the same config file"
    );
}
