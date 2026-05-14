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

    /// JSON serialization or deserialization of an editor config file failed.
    /// Covers both read-side parse failures and write-side serialize failures.
    #[error("editor config: JSON error at {path}: {source}")]
    EditorConfigJson {
        /// Path that was being read or written.
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
    ///
    /// Uses `#[source]` (not `#[from]`) so the auto-generated `From` would
    /// only fire on `Box<...>`; an explicit `From<ServerInitializeError>`
    /// impl below boxes inside so bare-error `?` propagation also compiles.
    #[error("mcp initialize error: {0}")]
    McpInitialize(#[source] Box<rmcp::service::ServerInitializeError>),

    /// An MCP protocol-level error occurred after the server was running
    /// (transport closed unexpectedly, response wrong shape, cancellation, etc.).
    ///
    /// Same `#[source] Box<...>` + hand-rolled `From` pattern as
    /// [`Error::McpInitialize`].
    #[error("mcp protocol error: {0}")]
    Mcp(#[source] Box<rmcp::ServiceError>),
}

impl From<rmcp::service::ServerInitializeError> for Error {
    fn from(err: rmcp::service::ServerInitializeError) -> Self {
        Self::McpInitialize(Box::new(err))
    }
}

impl From<Box<rmcp::service::ServerInitializeError>> for Error {
    fn from(err: Box<rmcp::service::ServerInitializeError>) -> Self {
        Self::McpInitialize(err)
    }
}

impl From<rmcp::ServiceError> for Error {
    fn from(err: rmcp::ServiceError) -> Self {
        Self::Mcp(Box::new(err))
    }
}

impl From<Box<rmcp::ServiceError>> for Error {
    fn from(err: Box<rmcp::ServiceError>) -> Self {
        Self::Mcp(err)
    }
}

/// Result alias for fallible brontes operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    // The Display strings on these variants are part of brontes's public
    // user-facing surface — they appear in error messages a downstream
    // consumer's `main` propagates to stderr or stdout. The assertions
    // below pin each variant's exact `Display` output so a future change
    // to the `#[error("…")]` attribute (rename, punctuation drift,
    // dropped `{path}` field) breaks the test loudly instead of silently
    // shipping a different consumer-visible message.

    #[test]
    fn config_error_display_pin() {
        let e = Error::Config("bad path".into());
        assert_eq!(e.to_string(), "config error: bad path");
    }

    #[test]
    fn spawn_error_display_pin() {
        let e = Error::Spawn(std::io::Error::other("nope"));
        assert_eq!(e.to_string(), "could not spawn subprocess: nope");
    }

    #[test]
    fn io_error_display_includes_context_and_source() {
        let e = Error::Io {
            context: "open ./mcp-tools.json".into(),
            source: std::io::Error::other("disk gone"),
        };
        assert_eq!(
            e.to_string(),
            "io error at open ./mcp-tools.json: disk gone"
        );
    }

    #[test]
    fn schema_error_display_pin() {
        let e = Error::Schema("missing required field".into());
        assert_eq!(e.to_string(), "schema error: missing required field");
    }

    #[test]
    fn editor_config_read_display_pin() {
        let e = Error::EditorConfigRead {
            path: PathBuf::from("/etc/x.json"),
            source: std::io::Error::other("permission denied"),
        };
        assert_eq!(
            e.to_string(),
            "editor config: read failed at /etc/x.json: permission denied"
        );
    }

    #[test]
    fn editor_config_json_display_pin() {
        // Build a real serde_json::Error so the `{source}` interpolation
        // gets exercised against a representative payload, not a stub.
        let json_err = serde_json::from_str::<serde_json::Value>("{not-json")
            .expect_err("malformed JSON must error");
        let e = Error::EditorConfigJson {
            path: PathBuf::from("/tmp/y.json"),
            source: json_err,
        };
        let s = e.to_string();
        assert!(
            s.starts_with("editor config: JSON error at /tmp/y.json: "),
            "got {s}"
        );
    }

    #[test]
    fn editor_config_backup_display_pin() {
        let e = Error::EditorConfigBackup {
            path: PathBuf::from("/tmp/z.backup.json"),
            source: std::io::Error::other("readonly"),
        };
        assert_eq!(
            e.to_string(),
            "editor config: backup failed for /tmp/z.backup.json: readonly"
        );
    }

    #[test]
    fn editor_config_write_display_pin() {
        let e = Error::EditorConfigWrite {
            path: PathBuf::from("/tmp/w.json"),
            source: std::io::Error::other("no space"),
        };
        assert_eq!(
            e.to_string(),
            "editor config: write failed at /tmp/w.json: no space"
        );
    }

    #[test]
    fn panic_error_display_pin() {
        let e = Error::Panic("middleware blew up".into());
        assert_eq!(e.to_string(), "panic: middleware blew up");
    }
}
