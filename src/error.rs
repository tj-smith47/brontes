//! Error and Result types for brontes.

use std::path::PathBuf;

/// Errors produced by brontes.
///
/// `Error` implements [`std::fmt::Display`] (via `thiserror`) so the
/// recommended `main` shape prints the human-friendly message and
/// returns a non-zero exit code explicitly:
///
/// ```no_run
/// fn main() -> std::process::ExitCode {
///     if let Err(e) = run() {
///         eprintln!("{e}");
///         return std::process::ExitCode::from(1);
///     }
///     std::process::ExitCode::SUCCESS
/// }
/// # fn run() -> brontes::Result<()> { Ok(()) }
/// ```
///
/// Using `fn main() -> brontes::Result<()>` also works but emits the
/// `Debug` form of the error, which is less readable.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// User configuration was rejected at construction time. The caller supplied
    /// a command path, annotation key, or selector that does not resolve in the
    /// clap tree, or the configured `command_name` collides with an existing
    /// subcommand.
    #[error("config error: {0}")]
    Config(String),

    /// An I/O operation failed outside of the editor-config or spawn paths.
    #[error("io error at {context}: {source}")]
    Io {
        /// Human-readable context describing the operation.
        context: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The CLI subprocess could not be spawned (binary missing, fork failed,
    /// permissions denied). Distinct from a subprocess that ran and exited
    /// non-zero, which is reported through `ToolOutput.exit_code`.
    #[error("could not spawn subprocess: {0}")]
    Spawn(#[source] std::io::Error),

    /// JSON Schema generation failed for a command's input or output schema.
    #[error("schema error: {0}")]
    Schema(String),

    /// Reading an editor config file failed.
    #[error("editor config: read failed at {path}: {source}")]
    EditorConfigRead {
        /// Path that was being read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Parsing an editor config file as JSON failed.
    #[error("editor config: parse failed at {path}: {source}")]
    EditorConfigParse {
        /// Path that was being parsed.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// Backing up an existing editor config before write failed.
    #[error("editor config: backup failed for {path}: {source}")]
    EditorConfigBackup {
        /// Path that was being backed up.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Writing the updated editor config failed.
    #[error("editor config: write failed at {path}: {source}")]
    EditorConfigWrite {
        /// Path that was being written.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A middleware closure or executor task panicked. The payload, if it was a
    /// `String` or `&'static str`, is preserved verbatim.
    #[error("panic: {0}")]
    Panic(String),

    /// The MCP server failed during initialization (transport setup, protocol
    /// negotiation, or cancellation before the first request).
    ///
    /// Boxed because `ServerInitializeError` is several hundred bytes — keeping
    /// the variant body small keeps the overall [`Error`] enum compact for the
    /// common (non-error) `Result<T, Error>` path.
    #[error("mcp initialize error: {0}")]
    McpInitialize(#[from] Box<rmcp::service::ServerInitializeError>),

    /// An MCP protocol-level error occurred after the server was running
    /// (transport closed unexpectedly, response wrong shape, cancellation, etc.).
    #[error("mcp protocol error: {0}")]
    Mcp(#[from] Box<rmcp::ServiceError>),
}

/// Result alias for fallible brontes operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_renders() {
        let e = Error::Config("bad path".into());
        assert_eq!(e.to_string(), "config error: bad path");
    }

    #[test]
    fn spawn_error_wraps_io() {
        let e = Error::Spawn(std::io::Error::other("nope"));
        assert!(e.to_string().contains("could not spawn subprocess"));
    }
}
