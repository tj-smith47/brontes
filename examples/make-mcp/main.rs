//! `make-mcp` — example brontes consumer that exposes GNU `make` over MCP.
//!
//! Wraps GNU `make` as a tiny single-leaf CLI: a `build` subcommand with a
//! required `--directory` flag (the makefile root) plus optional `--target`,
//! `--jobs`, and `--dry-run` knobs. Demonstrates how a real CLI surfaces as
//! MCP tools, and exercises the required-flag schema path so generated tool
//! schemas carry a non-empty `inputSchema.required` array.
//!
//! Run as an MCP server over stdio:
//!
//! ```bash
//! cargo run --example make-mcp -- mcp start
//! ```
//!
//! Export the generated tool list to `./mcp-tools.json`:
//!
//! ```bash
//! cargo run --example make-mcp -- mcp tools
//! ```
//!
//! Invoke the wrapped tool directly (bypassing MCP):
//!
//! ```bash
//! cargo run --example make-mcp -- build --directory ./my-project --target test
//! ```

use clap::{Arg, ArgAction, Command};

#[tokio::main]
async fn main() -> brontes::Result<()> {
    // The clap root is named `make` (not `make-mcp`) so the brontes substring
    // filter — which excludes any path containing the `mcp` group name — does
    // not accidentally drop every tool in this CLI. The produced binary is
    // still `target/debug/examples/make-mcp` (the Cargo `[[example]]` stanza
    // controls the binary name independently from the clap `Command::new`).
    let cli = Command::new("make")
        .version("0.1.0")
        .about("Run GNU make targets via MCP")
        .subcommand(
            Command::new("build")
                .about("Run a make target")
                .arg(
                    Arg::new("directory")
                        .long("directory")
                        .short('C')
                        .value_name("DIR")
                        .required(true)
                        .help("Directory containing the Makefile to run"),
                )
                .arg(
                    Arg::new("target")
                        .long("target")
                        .short('t')
                        .value_name("NAME")
                        .help("Make target to invoke (default: makefile's default goal)"),
                )
                .arg(
                    Arg::new("jobs")
                        .long("jobs")
                        .short('j')
                        .value_name("N")
                        .help("Number of parallel jobs (passed to make -j)"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .short('n')
                        .action(ArgAction::SetTrue)
                        .help("Print what make would do but don't execute it"),
                ),
        )
        .subcommand(brontes::command(None));

    let matches = cli.clone().get_matches();
    match matches.subcommand() {
        Some(("mcp", sub)) => brontes::handle(sub, &cli, None).await,
        Some(("build", sub)) => run_make(sub),
        _ => {
            let mut help = cli.clone();
            help.print_help().map_err(|e| brontes::Error::Io {
                context: "print help".to_string(),
                source: e,
            })?;
            println!();
            std::process::exit(2);
        }
    }
}

/// Shell out to `make` with the parsed args. Used when the example is
/// invoked directly (`build` subcommand) — the same flag surface that MCP
/// exposes as a tool.
fn run_make(sub: &clap::ArgMatches) -> brontes::Result<()> {
    let dir: &String = sub
        .get_one("directory")
        .expect("clap enforces --directory required");

    let mut cmd = std::process::Command::new("make");
    cmd.arg("-C").arg(dir);
    if let Some(target) = sub.get_one::<String>("target") {
        cmd.arg(target);
    }
    if let Some(jobs) = sub.get_one::<String>("jobs") {
        cmd.arg(format!("-j{jobs}"));
    }
    if sub.get_flag("dry-run") {
        cmd.arg("-n");
    }

    let status = cmd.status().map_err(brontes::Error::Spawn)?;
    std::process::exit(status.code().unwrap_or(1));
}
