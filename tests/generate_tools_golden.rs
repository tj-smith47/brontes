//! Golden test pinning `brontes::generate_tools` wire shape.
//!
//! The fixture in `tests/fixtures/generate_tools_golden.json` is the
//! canonical v0.1.0 output for the tree built in this file. Any change to
//! the wire shape — input schema layout, tool description, annotation
//! handling, naming — diffs against the fixture and fails the test.
//!
//! # Regenerating the fixture
//!
//! After an intentional wire-shape change:
//!
//! ```bash
//! UPDATE_GOLDEN=1 cargo test --test generate_tools_golden
//! ```
//!
//! Then inspect `git diff tests/fixtures/generate_tools_golden.json` before
//! committing — every change to the fixture is a wire-shape change visible
//! to MCP clients.
//!
//! # Feature coverage
//!
//! The fixture tree exercises every flag shape brontes lowers to JSON
//! Schema today: required + optional positional, bool / integer / float /
//! string / `PathBuf` flags, hidden flags (filtered), enum flags via
//! `PossibleValuesParser`, `ArgAction::Append` (array) and `ArgAction::Count`
//! (integer) flags, a leaf with `.after_help(...)` (Examples block in the
//! description), a global flag inherited by every leaf, a deprecated leaf
//! (filtered out via `Config::deprecate`), and a leaf named `helpful`
//! filtered by the `"help"` substring rule.
//!
//! Leaves expected in the golden output: `act foo`, `act bar`, `act baz`.
//! Leaves expected to be absent: `act legacy` (deprecated) and
//! `edge helpful` (filtered by the substring-on-path rule — `"help"` is a
//! substring of `"helpful"`, this is the documented quirk in
//! `walk::should_filter`).
//!
//! # Group commands do NOT surface as tools
//!
//! `golden-cli` (root), `golden-cli act`, and `golden-cli edge` declare
//! `.subcommand_required(true)` with no LOCAL user-facing args, so they are
//! filtered out by `walk::is_group_only`. The root's `.global(true) --verbose`
//! flag propagates to every descendant but is excluded from the local-arg
//! count, so it does not promote intermediate group nodes into tools.

use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser};

const FIXTURE_PATH: &str = "tests/fixtures/generate_tools_golden.json";

/// Build the fixture command tree.
///
/// Shape:
///
/// ```text
/// golden-cli  (subcommand_required; global --verbose; .about)
///   ├── act   (group-only; subcommand_required)
///   │     ├── foo     — required positional + bool + int + after_help
///   │     ├── bar     — float + string + path + Append
///   │     ├── baz     — enum + Count + hidden + optional positional
///   │     └── legacy  — marked deprecated → filtered
///   └── edge  (group-only; subcommand_required)
///         └── helpful — filtered by the "help" substring rule
/// ```
fn fixture_tree() -> Command {
    Command::new("golden-cli")
        .about("Golden CLI used to pin the brontes wire shape")
        .subcommand_required(true)
        .arg(
            // A `.global(true)` flag on the root propagates to every walked
            // leaf, so each leaf's flags.properties must contain `verbose`.
            Arg::new("verbose")
                .long("verbose")
                .global(true)
                .help("Increase output verbosity")
                .action(ArgAction::SetTrue),
        )
        .subcommand(
            Command::new("act")
                .about("Action commands")
                .subcommand_required(true)
                .subcommand(
                    // Required positional + bool flag + integer flag + after_help.
                    Command::new("foo")
                        .about("Run the foo action")
                        .arg(Arg::new("path").help("Target path").required(true))
                        .arg(
                            Arg::new("debug")
                                .long("debug")
                                .help("Enable debug output")
                                .action(ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("count")
                                .long("count")
                                .help("How many times to run")
                                .value_parser(clap::value_parser!(i64)),
                        )
                        .after_help("golden-cli act foo /tmp --count 3"),
                )
                .subcommand(
                    // Float + string + path-typed + Append flag.
                    Command::new("bar")
                        .about("Run the bar action")
                        .arg(
                            Arg::new("ratio")
                                .long("ratio")
                                .help("Ratio to apply")
                                .value_parser(clap::value_parser!(f64)),
                        )
                        .arg(Arg::new("label").long("label").help("Human-readable label"))
                        .arg(
                            Arg::new("config")
                                .long("config")
                                .help("Path to a config file")
                                .value_parser(clap::value_parser!(std::path::PathBuf)),
                        )
                        .arg(
                            Arg::new("tag")
                                .long("tag")
                                .help("Tag to attach (repeatable)")
                                .action(ArgAction::Append),
                        ),
                )
                .subcommand(
                    // Enum + Count + hidden + optional positional.
                    Command::new("baz")
                        .about("Run the baz action")
                        .arg(Arg::new("target").help("Optional target").required(false))
                        .arg(
                            Arg::new("format")
                                .long("format")
                                .help("Output format")
                                .value_parser(PossibleValuesParser::new(["json", "yaml", "toml"])),
                        )
                        .arg(
                            Arg::new("v")
                                .short('v')
                                .help("Verbosity (repeat for more)")
                                .action(ArgAction::Count),
                        )
                        .arg(
                            // Hidden — must be filtered out of flags.properties.
                            Arg::new("secret")
                                .long("secret")
                                .help("Internal-only flag")
                                .hide(true),
                        ),
                )
                .subcommand(
                    Command::new("legacy")
                        .about("Legacy command, marked deprecated in Config")
                        .arg(Arg::new("name").long("name").help("Legacy flag")),
                ),
        )
        .subcommand(
            Command::new("edge")
                .about("Edge-case leaves")
                .subcommand_required(true)
                .subcommand(
                    // Substring-collision: "helpful" contains "help",
                    // so walk::should_filter excludes it. Documented in the
                    // module doc above.
                    Command::new("helpful")
                        .about("Filtered by the help-substring rule")
                        .arg(Arg::new("note").long("note").help("A note")),
                ),
        )
}

/// Build the fixture config.
///
/// The single configured behaviour is `deprecate("golden-cli act legacy")`,
/// which removes the `legacy` leaf from the generated tool list.
fn fixture_config() -> brontes::Config {
    brontes::Config::default().deprecate("golden-cli act legacy")
}

#[test]
fn generate_tools_golden() {
    let root = fixture_tree();
    let cfg = fixture_config();
    let tools = brontes::generate_tools(&root, &cfg).expect("generation must succeed");
    let actual = serde_json::to_string_pretty(&tools).expect("serialization must succeed");

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(FIXTURE_PATH, format!("{actual}\n")).expect("write golden fixture");
        return;
    }

    let expected = std::fs::read_to_string(FIXTURE_PATH).expect("read golden fixture");
    // Fixture is written with exactly one trailing newline; trim both sides
    // so an editor that strips the newline on save does not flip the test.
    pretty_assertions::assert_eq!(actual.trim(), expected.trim());
}
