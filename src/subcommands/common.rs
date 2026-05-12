//! Shared helpers reused by multiple `mcp` leaves.
//!
//! Currently hosts [`parse_log_level`], which both `mcp start` and
//! `mcp stream` need with byte-identical semantics — including the
//! `tracing::warn!` on unrecognized values that PLAN §11 #9 pins as
//! a deliberate divergence from ophis (which silently maps unknown
//! levels to `Info`).

use clap::ArgMatches;
use tracing::Level;

/// Parse the `--log-level` flag into a [`Level`] when present.
///
/// Invalid values return `None` (i.e., fall through to `Config::log_level`
/// or `RUST_LOG`); a `tracing::warn!` records the offending value so users
/// notice the typo at startup rather than wondering why their level had
/// no effect. PLAN §11 #9 documents this divergence from ophis's silent
/// fallback to `Info`.
///
/// Shared between `mcp start` (stdio transport) and `mcp stream`
/// (streamable-HTTP transport) so the two surfaces cannot drift in their
/// `--log-level` parsing semantics. Each surface's `parse_log_level`
/// wrapper exists to keep the call sites in their own module but
/// delegates here without modification.
pub(crate) fn parse_log_level(matches: &ArgMatches) -> Option<Level> {
    let raw = matches.get_one::<String>("log-level")?;
    match raw.to_ascii_lowercase().as_str() {
        "trace" => Some(Level::TRACE),
        "debug" => Some(Level::DEBUG),
        "info" => Some(Level::INFO),
        "warn" | "warning" => Some(Level::WARN),
        "error" => Some(Level::ERROR),
        other => {
            tracing::warn!(value = %other, "unrecognized --log-level; falling back to default");
            None
        }
    }
}
