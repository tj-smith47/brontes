## Summary

<!-- Brief description of what this PR does -->

## Type of Change

- [ ] Bug fix
- [ ] New feature
- [ ] Enhancement to existing feature
- [ ] Refactoring (no functional change)
- [ ] Documentation
- [ ] CI/CD

## Changes Made

-

## Checklist

### Code Quality
- [ ] No `unwrap()` or `expect()` in library code (tests excepted)
- [ ] `thiserror` for library errors; `anyhow` only at binary boundaries
- [ ] Public API changes documented with rustdoc + example
- [ ] Import grouping: std, external, internal (blank line separated)

### Testing
- [ ] `cargo test` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] New code has unit tests
- [ ] If touching `mcp stream` / editor subcommands, integration coverage added or updated

### Documentation
- [ ] README updated (if user-facing API change)
- [ ] CHANGELOG.md updated (if user-facing change)
- [ ] Rustdoc examples build (`cargo test --doc`)

## Testing Done

<!-- How did you test this? Include the consumer CLI / MCP client used if relevant. -->

## Related Issues

<!-- Link to related issues: Fixes #123, Relates to #456 -->
