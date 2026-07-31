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

use brontes::{Config, Selector, generate_tools, selectors};

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
        .subcommand(Command::new("status").about("Show status"))
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
            "demo_publish",
            "demo_status"
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
        err.to_string().contains("demo secrets"),
        "the command caught between the two must be named, got {err}"
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
        msg.contains("demo publish"),
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

// ─────────────────────────────────────────────────────────────────────────────
// Selecting something the CLI will not serve
// ─────────────────────────────────────────────────────────────────────────────
//
// A path can name a real command and still yield no tool: the command may be
// deprecated, `hide`-ed in clap, a navigation node, or dropped by a selector.
// Validating against the walked tree alone would let those selections pass and
// then quietly vanish, which is the one outcome a client cannot diagnose.

#[test]
fn selecting_a_deprecated_command_is_reported_not_dropped() {
    let cfg = Config::default()
        .deprecate("demo publish")
        .expose_command("demo publish")
        .expose_command("demo status");
    let msg = err(&cfg);
    assert!(
        msg.contains("demo publish") && msg.contains("deprecated"),
        "asking for a deprecated command must say so rather than serving the \
         rest of the selection: {msg}"
    );
}

#[test]
fn selecting_a_selector_excluded_command_is_reported() {
    let cfg = Config::default()
        .selector(Selector {
            cmd: Some(selectors::allow_cmds(["demo status"])),
            ..Default::default()
        })
        .expose_command("demo publish")
        .expose_command("demo status");
    let msg = err(&cfg);
    assert!(
        msg.contains("demo publish"),
        "a selector already excluded it, so the selection cannot be honoured: {msg}"
    );
}

#[test]
fn a_group_whose_commands_are_all_filtered_out_is_reported() {
    let cfg = grouped()
        .deprecate("demo release")
        .deprecate("demo release notes")
        .deprecate("demo publish")
        .expose_group("release");
    let msg = err(&cfg);
    assert!(
        msg.contains("release") && msg.contains("exposes no tools"),
        "an empty group must name itself: {msg}"
    );
}

#[test]
fn a_group_that_still_has_one_live_command_is_served() {
    // The other side of the line: a group is not required to be intact, only
    // non-empty. A developer who deprecates one member has not broken it.
    let got = names(&grouped().deprecate("demo publish").expose_group("release"));
    assert_eq!(got, vec!["demo_release", "demo_release_notes"]);
}

#[test]
fn selecting_then_hiding_the_same_tool_is_reported() {
    let cfg = Config::default()
        .expose_tool("demo_release")
        .expose_tool("demo_status")
        .hide_tool("demo_release");
    let msg = err(&cfg);
    assert!(
        msg.contains("demo_release"),
        "contradicting flags must name the tool caught between them: {msg}"
    );
}

#[test]
fn listing_groups_refuses_to_look_like_a_filtered_listing() {
    // `--groups` describes the CLI, not one server's subset, so combining it
    // with a selection is a mistake rather than a filter.
    let err = brontes::__test_internal::tools_subcommand()
        .try_get_matches_from(["tools", "--groups", "--group", "release"])
        .expect_err("--groups and --group must not combine");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

// ─────────────────────────────────────────────────────────────────────────────
// Name collisions the tool list cannot express
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn two_commands_that_generate_one_tool_name_are_rejected() {
    // A path separator becomes an underscore, so a flat `by_cell` and a nested
    // `by cell` land on the same MCP name. Serving both would advertise one
    // name twice, dispatch every call to whichever came first, and make
    // `--tool demo_by_cell` silently mean two different things.
    let colliding = Command::new("demo")
        .subcommand(Command::new("by_cell").about("Flat"))
        .subcommand(
            Command::new("by")
                .about("Nested")
                .subcommand(Command::new("cell").about("Leaf")),
        );

    let msg = generate_tools(&colliding, &Config::default())
        .expect_err("a duplicate tool name must be rejected")
        .to_string();

    assert!(
        msg.contains("demo_by_cell")
            && msg.contains("demo by_cell")
            && msg.contains("demo by cell"),
        "the error must name the tool and both commands that produce it: {msg}"
    );
}

#[test]
fn a_command_name_containing_a_space_is_rejected() {
    // Command paths are joined with spaces everywhere brontes addresses a
    // command, so a name with one in it cannot be named by `--command`, by a
    // group member, or by a `Config` key.
    let spaced = Command::new("demo").subcommand(Command::new("by cell").about("Two words"));

    let msg = generate_tools(&spaced, &Config::default())
        .expect_err("a command name with a space must be rejected")
        .to_string();

    assert!(
        msg.contains("by cell") && msg.contains("contains a space"),
        "the error must name the offending command: {msg}"
    );
}

#[test]
fn a_path_that_is_only_a_prefix_of_a_command_is_rejected() {
    // `"my"` is not a command on a CLI rooted at `my-tool`. Matching selection
    // paths by prefix would let it through and silently select the whole tree.
    let cli = Command::new("my-tool").subcommand(Command::new("release").about("Cut a release"));

    let msg = generate_tools(&cli, &Config::default().expose_command("my"))
        .expect_err("a partial path must not select a subtree")
        .to_string();

    assert!(
        msg.contains("no such command") && msg.contains("my-tool my"),
        "the error must name the path that does not exist: {msg}"
    );
}

#[test]
fn a_group_with_no_commands_is_rejected() {
    // `group_description` creates the entry, so a description set against a
    // typo'd name leaves behind a group that can never select anything.
    let msg = err(&grouped().group_description("ghost", "Nothing here"));

    assert!(
        msg.contains("ghost") && msg.contains("no commands"),
        "an empty group must be reported where it is defined: {msg}"
    );
}

#[test]
fn hiding_a_command_the_server_would_not_serve_anyway_is_allowed() {
    // Hiding is held to a weaker standard than exposing on purpose: the
    // outcome of `--hide-command` on an already-unserved command is exactly
    // what was asked for, so failing would be pedantry rather than safety.
    let cfg = grouped()
        .deprecate("demo publish")
        .hide_command("demo publish");

    let got = names(&cfg);
    assert!(
        !got.iter().any(|n| n == "demo_publish"),
        "the command must be absent either way: {got:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Installing into an editor
// ─────────────────────────────────────────────────────────────────────────────

/// The demo CLI with the `mcp` subtree mounted, for dispatching through
/// `brontes::handle` the way a consumer's `main` does.
fn mounted() -> Command {
    cli().subcommand(brontes::command(None))
}

/// Run one `mcp …` invocation against the demo CLI.
fn dispatch(argv: &[&str], cfg: Option<&Config>) -> brontes::Result<()> {
    let cli = mounted();
    let matches = cli
        .clone()
        .try_get_matches_from(argv)
        .expect("argv must parse");
    let sub = matches
        .subcommand_matches("mcp")
        .expect("argv must select the mcp subtree");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(brontes::handle(sub, &cli, cfg))
}

#[test]
fn every_editor_enable_carries_the_same_selection_flags() {
    // An editor config is a launch command, so `enable` has to be able to
    // express every selection `mcp start` accepts. A flag missing from one
    // editor is a selection that silently cannot be installed there.
    let mcp = brontes::command(None);
    let expected = [
        "group",
        "command",
        "tool",
        "hide-group",
        "hide-command",
        "hide-tool",
    ];

    for editor in ["claude", "cursor", "vscode", "zed"] {
        let enable = mcp
            .find_subcommand(editor)
            .unwrap_or_else(|| panic!("mcp {editor} must exist"))
            .find_subcommand("enable")
            .unwrap_or_else(|| panic!("mcp {editor} enable must exist"));
        let present: Vec<&str> = enable
            .get_arguments()
            .map(|a| a.get_id().as_str())
            .collect();
        for flag in expected {
            assert!(
                present.contains(&flag),
                "mcp {editor} enable is missing --{flag}: {present:?}"
            );
        }
    }
}

#[test]
fn an_editor_install_refuses_a_selection_that_would_not_start() {
    // The editor spawns the server where nobody reads stderr, so a typo that
    // only fails at launch reaches the user as an editor showing no tools.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("claude_desktop_config.json");

    let msg = dispatch(
        &[
            "demo",
            "mcp",
            "claude",
            "enable",
            "--config-path",
            path.to_str().expect("utf8 path"),
            "--server-name",
            "demo",
            "--group",
            "relase",
        ],
        Some(&grouped()),
    )
    .expect_err("a typo'd group must be caught before anything is written")
    .to_string();

    assert!(
        msg.contains("no such group") && msg.contains("relase"),
        "the error must name the typo: {msg}"
    );
    assert!(
        !path.exists(),
        "a rejected selection must leave no config file behind"
    );
}

#[test]
fn an_editor_install_writes_a_selection_it_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("claude_desktop_config.json");

    dispatch(
        &[
            "demo",
            "mcp",
            "claude",
            "enable",
            "--config-path",
            path.to_str().expect("utf8 path"),
            "--server-name",
            "demo",
            "--group",
            "release",
            "--hide-tool",
            "demo_release_notes",
        ],
        Some(&grouped()),
    )
    .expect("a valid selection installs");

    let doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read config")).expect("parse config");
    let args: Vec<&str> = doc["mcpServers"]["demo"]["args"]
        .as_array()
        .expect("args array")
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();

    assert_eq!(
        args,
        vec![
            "mcp",
            "start",
            "--group",
            "release",
            "--hide-tool",
            "demo_release_notes",
        ]
    );
}

#[test]
fn listing_groups_still_works_under_a_pinned_selection() {
    // `--groups` answers "what does this CLI define", which does not change
    // because the server was pinned to a subset — and must not fail when the
    // pinned subset is narrower than the groups being listed.
    dispatch(
        &["demo", "mcp", "tools", "--groups"],
        Some(&grouped().expose_group("release")),
    )
    .expect("--groups describes the CLI, not the selection");
}

// ─────────────────────────────────────────────────────────────────────────────
// Writing a path without the CLI's own name
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_path_may_omit_the_root_command_name() {
    // The root is the binary the user just invoked; making them retype it is
    // ceremony, so a path is read as relative to the root unless it already
    // starts there.
    assert_eq!(
        names(&Config::default().expose_command("release")),
        names(&Config::default().expose_command("demo release"))
    );
    assert_eq!(
        names(&Config::default().expose_command("secrets get")),
        names(&Config::default().expose_command("demo secrets get"))
    );
}

#[test]
fn group_members_may_omit_the_root_command_name() {
    let relative = Config::default()
        .group("release", ["release", "publish"])
        .expose_group("release");
    let absolute = grouped().expose_group("release");

    assert_eq!(names(&relative), names(&absolute));
}

#[test]
fn hiding_takes_a_relative_path_too() {
    // The two halves of the matrix have to read the same way, or a user who
    // learns `--command release` gets a silent no-op from `--hide-command`.
    assert_eq!(
        names(&Config::default().hide_command("secrets")),
        names(&Config::default().hide_command("demo secrets"))
    );
}

#[test]
fn the_launch_flags_take_a_relative_path() {
    let from_flags = brontes::__test_internal::apply_selection_flags(
        Config::default(),
        &brontes::__test_internal::start_subcommand()
            .try_get_matches_from([
                "start",
                "--command",
                "release",
                "--hide-tool",
                "demo_release_notes",
            ])
            .expect("parses"),
    );

    assert_eq!(names(&from_flags), vec!["demo_release"]);
}

#[test]
fn a_relative_path_that_names_nothing_is_still_rejected() {
    // Resolving against the root must not turn a typo into a silent miss; the
    // error names the resolved path so it is obvious what was looked for.
    let msg = err(&Config::default().expose_command("relase"));

    assert!(
        msg.contains("no such command") && msg.contains("demo relase"),
        "the resolved path must be named: {msg}"
    );
}

#[test]
fn the_root_command_itself_is_still_addressable() {
    // `--command demo` names the root and therefore the whole tree; the
    // leading segment is read as the root before anything else.
    assert_eq!(
        names(&Config::default().expose_command("demo")),
        names(&Config::default())
    );
}

#[test]
fn every_config_path_key_may_omit_the_root_command_name() {
    // The whole `Config` surface has to read one way. A user who learns
    // `--command release` and then writes `deprecate("anodizer secrets")`
    // because the builders disagree is being taxed for our inconsistency.
    use brontes::{DescriptionMode, TaskMode};

    let relative = Config::default()
        .description("release", "Cut a release, carefully")
        .description_mode_for("release", DescriptionMode::Short)
        .task_mode_for("release", TaskMode::Detached)
        .deprecate("releases");
    let absolute = Config::default()
        .description("demo release", "Cut a release, carefully")
        .description_mode_for("demo release", DescriptionMode::Short)
        .task_mode_for("demo release", TaskMode::Detached)
        .deprecate("demo releases");

    let from_relative = generate_tools(&cli(), &relative).expect("relative paths resolve");
    let from_absolute = generate_tools(&cli(), &absolute).expect("absolute paths resolve");

    assert_eq!(
        from_relative
            .iter()
            .map(|t| t.name.to_string())
            .collect::<Vec<_>>(),
        from_absolute
            .iter()
            .map(|t| t.name.to_string())
            .collect::<Vec<_>>(),
        "deprecation must drop the same command either way"
    );
    let described = from_relative
        .iter()
        .find(|t| t.name.as_ref() == "demo_release")
        .expect("demo_release is served");
    assert_eq!(
        described.description.as_deref(),
        Some("Cut a release, carefully"),
        "a relative path must reach the description override"
    );
}

#[test]
fn a_promoted_flag_takes_a_relative_path() {
    let cli = Command::new("demo").subcommand(
        Command::new("deploy").arg(clap::Arg::new("region").long("region").value_name("R")),
    );

    let relative = generate_tools(&cli, &Config::default().promote_flag("deploy", "region"))
        .expect("relative promotion resolves");
    let absolute = generate_tools(
        &cli,
        &Config::default().promote_flag("demo deploy", "region"),
    )
    .expect("absolute promotion resolves");

    let promoted = |tools: &[rmcp::model::Tool]| {
        tools
            .iter()
            .find(|t| t.name.as_ref() == "demo_deploy")
            .and_then(|t| t.input_schema.get("properties").cloned())
            .expect("deploy tool with properties")
    };
    assert_eq!(promoted(&relative), promoted(&absolute));
    assert!(
        promoted(&relative).get("region").is_some(),
        "the promoted flag must be hoisted to the top level: {:?}",
        promoted(&relative)
    );
}
