//! Performance budget for [`brontes::generate_tools`].
//!
//! Synthetic-tree benchmark exercising 100 leaf commands with mixed
//! flag types and positionals. Marked `#[ignore]` so CI doesn't run it on
//! every push; invoke with:
//!
//! ```text
//! cargo test --release -- --ignored generate_tools_perf
//! ```
//!
//! Budget: 100 ms cold (first call after a warm-up), 50 ms warm
//! (5-run average). Cold includes the static-schema cache fill and the
//! per-tool descriptor build; warm should hit the cache directly.

use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser};

fn build_leaf(name: String) -> Command {
    Command::new(name)
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("count")
                .long("count")
                .value_parser(clap::value_parser!(i64)),
        )
        .arg(Arg::new("label").long("label"))
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(PossibleValuesParser::new(["json", "yaml", "toml"])),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_parser(clap::value_parser!(std::path::PathBuf)),
        )
        .arg(Arg::new("source").required(true))
        .arg(Arg::new("target").required(false))
}

fn build_synthetic_tree() -> Command {
    let mut root = Command::new("perf-cli").subcommand_required(true);
    for g in 0..10 {
        let mut group = Command::new(format!("grp-{g}")).subcommand_required(true);
        for l in 0..10 {
            group = group.subcommand(build_leaf(format!("leaf-{g}-{l}")));
        }
        root = root.subcommand(group);
    }
    root
}

#[test]
#[ignore = "wall-clock-time-sensitive; would flake on busy CI runners — run with --ignored"]
fn generate_tools_perf_100_commands() {
    let root = build_synthetic_tree();
    let cfg = brontes::Config::default();

    // Warm up the static schema cache (the first call pays the LazyLock
    // init cost). Discard the result.
    let _warmup = brontes::generate_tools(&root, &cfg).expect("warmup call should succeed");

    // Cold = one call immediately after warm-up. (The static caches are
    // warm; the schema-per-tool build is "cold" relative to repeated
    // calls.)
    let start = std::time::Instant::now();
    let cold = brontes::generate_tools(&root, &cfg).expect("cold call should succeed");
    let cold_ms = start.elapsed().as_millis();
    assert_eq!(
        cold.len(),
        100,
        "expected 100 leaf tools, got {}",
        cold.len()
    );

    // Warm = 5-call average.
    let mut total = std::time::Duration::ZERO;
    for _ in 0..5 {
        let s = std::time::Instant::now();
        let _ = brontes::generate_tools(&root, &cfg).expect("warm call should succeed");
        total += s.elapsed();
    }
    let warm_ms = (total / 5).as_millis();

    eprintln!("generate_tools_perf: cold = {cold_ms} ms, warm = {warm_ms} ms");
    assert!(
        cold_ms < 100,
        "cold {cold_ms} ms exceeds 100 ms budget — investigate hot path"
    );
    assert!(
        warm_ms < 50,
        "warm {warm_ms} ms exceeds 50 ms budget — investigate per-tool schema build"
    );
}
