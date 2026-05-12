//! Error and Result types for brontes.

use std::path::PathBuf;

/// Errors produced by brontes.
///
/// `Error` implements [`std::process::Termination`] so a `Result<(), brontes::Error>`
/// returned from `main` yields a non-zero exit code automatically.
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
}

/// Result alias for fallible brontes operations.
pub type Result<T> = std::result::Result<T, Error>;

impl std::process::Termination for Error {
    fn report(self) -> std::process::ExitCode {
        eprintln!("{self}");
        std::process::ExitCode::from(1)
    }
}

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
