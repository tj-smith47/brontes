# Contributing to brontes

Thanks for your interest. Please read this before opening a PR.

## Development setup

```bash
git clone https://github.com/tj-smith47/brontes
cd brontes
cargo build
cargo test
```

A Taskfile is provided for common workflows.

```bash
task           # list available tasks
task ci        # full local CI sweep (fmt, clippy, test, doc, audit)
task fmt       # cargo fmt
task clippy    # cargo clippy with -D warnings
task test      # cargo test
```

## Pull requests

1. Fork and branch from `main`.
2. Run `task ci` before pushing. CI mirrors this exactly and will reject anything that fails locally.
3. Write tests for any new behavior. We follow a no-stubs rule: a feature either works end-to-end with tests or it does not land.
4. Match the existing code style. `cargo fmt` is authoritative.
5. Update the `CHANGELOG.md` Unreleased section with a one-line summary.

## Commit messages

Conventional Commits encouraged: `feat:`, `fix:`, `perf:`, `refactor:`, `docs:`, `test:`, `chore:`. Use `!` for breaking changes (`feat!: drop FooBar`). The release tooling parses these to compute version bumps.

## MSRV

The minimum supported Rust version is pinned in `rust-toolchain.toml` and `Cargo.toml`. Bumping it is a minor-version release event.

## License

Contributions are licensed under MIT, matching the rest of the project. By submitting a PR you agree to license your changes under MIT.
