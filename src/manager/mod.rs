//! Editor-config manager — the shared load / save / backup machinery
//! consumed by the per-editor subcommand handlers (Claude in Task #4,
//! Cursor and `VSCode` in Tasks #5 and #6).
//!
//! The shape mirrors ophis `internal/cfgmgr/manager/manager.go`:
//!
//! - [`EditorConfig`] is the trait every editor's on-disk config implements.
//!   It exposes a typed view of the server map keyed by name; each editor
//!   chooses its own server-struct shape (Claude is `command/args/env`,
//!   VSCode/Cursor add `type/url/headers`).
//! - [`Manager<C>`] holds the resolved config path, the loaded (or
//!   defaulted) config, and the `load` / `save` / `enable_server` /
//!   `disable_server` / `print` surface.
//!
//! # Save semantics — backup before write
//!
//! When the target path already exists on save, the existing file is copied
//! to `<base>.backup.json` (replacing the final extension) **before** the
//! new bytes are written. Backup failure returns
//! [`crate::Error::EditorConfigBackup`] and the primary file is NOT touched —
//! the caller may safely retry or abort. The missing-file case skips the
//! backup step entirely; first writes do not litter `.backup.json` files
//! next to fresh configs.
//!
//! # Determinism
//!
//! JSON is written with 2-space indent (`serde_json::to_vec_pretty`). The
//! `EditorConfig` impl is expected to back its server map with a
//! [`std::collections::BTreeMap`] (or a structure that walks keys in
//! sorted order) so the on-disk bytes are stable across runs — important
//! for the golden round-trip parity tests against ophis.

pub(crate) mod claude;
pub(crate) mod paths;

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The on-disk shape of an editor's MCP configuration file.
///
/// Implementors expose a typed window onto the editor's server map. The
/// associated `Server` type is opaque to the manager — each editor chooses
/// its own server struct (e.g. [`claude::ClaudeServer`]) — but every editor
/// has the same load / mutate / save lifecycle, so the trait factors out
/// the polymorphic part and lets [`Manager`] be generic over `C: EditorConfig`.
///
/// Implementors must serialize and deserialize symmetrically: the
/// `Default::default()` value must round-trip through `serde_json` without
/// shape drift, so [`Manager::load`] on a missing file produces an empty
/// config that subsequent `enable_server` calls can extend without
/// surprising the editor at read-back time.
pub(crate) trait EditorConfig:
    serde::Serialize + serde::de::DeserializeOwned + Default + Clone
{
    /// The per-editor server-entry type written under the server map.
    ///
    /// Claude's [`claude::ClaudeServer`] has `command`, optional `args`,
    /// optional `env`. `VSCode` and Cursor (Tasks #5/#6) add `type`, `url`,
    /// `headers`; the trait is intentionally opaque to those differences.
    type Server: serde::Serialize + serde::de::DeserializeOwned + Clone;

    /// Whether the named server is currently present in the config.
    fn has_server(&self, name: &str) -> bool;

    /// Insert or replace the named server entry.
    fn add_server(&mut self, name: String, server: Self::Server);

    /// Remove the named server entry if present. No-op on missing names.
    fn remove_server(&mut self, name: &str);

    /// Iterate configured server names in insertion (`BTreeMap` key) order.
    ///
    /// Used by `mcp <editor> list` to print one name per line. Returning a
    /// boxed iterator keeps the trait object-safe-ish and avoids leaking
    /// the concrete map type through the trait.
    fn server_names(&self) -> Box<dyn Iterator<Item = &str> + '_>;
}

/// Generic manager that load / mutates / saves an editor's MCP config.
///
/// Construct via [`Manager::load`]; missing files yield a default config
/// rather than an error so the first `mcp <editor> enable` invocation on
/// a fresh machine writes the initial config without ceremony.
pub(crate) struct Manager<C: EditorConfig> {
    path: PathBuf,
    config: C,
}

impl<C: EditorConfig> Manager<C> {
    /// Read the editor config from `path`. A missing file yields
    /// [`C::default()`](Default::default); other I/O or JSON-parse failures
    /// surface as [`Error::EditorConfigRead`] or [`Error::EditorConfigParse`].
    ///
    /// Emits `Using config path "<path>"` to stdout on every call (ophis
    /// `manager.go:114` parity).
    pub(crate) fn load(path: PathBuf) -> Result<Self> {
        println!("Using config path \"{}\"", path.display());
        let exists = path.exists();
        if !exists {
            return Ok(Self {
                path,
                config: C::default(),
            });
        }
        let data = std::fs::read(&path).map_err(|e| Error::EditorConfigRead {
            path: path.clone(),
            source: e,
        })?;
        let config: C = serde_json::from_slice(&data).map_err(|e| Error::EditorConfigParse {
            path: path.clone(),
            source: e,
        })?;
        Ok(Self { path, config })
    }

    /// Persist the current config to disk with `2`-space-indented JSON.
    ///
    /// Backup-before-write: when the path already exists, the existing
    /// bytes are first copied to `<base>.backup.json`; failure to write the
    /// backup aborts the entire save and the primary file is unchanged.
    /// When the path does not yet exist, the parent directory is created
    /// (matching ophis `manager.go:135-138` `os.MkdirAll`).
    pub(crate) fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| Error::EditorConfigWrite {
                path: self.path.clone(),
                source: e,
            })?;
        }
        let bytes =
            serde_json::to_vec_pretty(&self.config).map_err(|e| Error::EditorConfigParse {
                path: self.path.clone(),
                source: e,
            })?;
        // Backup BEFORE write so a write that fails mid-stream still leaves
        // the previous content recoverable.
        self.backup()?;
        std::fs::write(&self.path, &bytes).map_err(|e| Error::EditorConfigWrite {
            path: self.path.clone(),
            source: e,
        })?;
        Ok(())
    }

    /// Copy the current on-disk config to `<base>.backup.json`.
    ///
    /// No-op when the primary file does not yet exist (first write on a
    /// fresh machine). Failure returns [`Error::EditorConfigBackup`] and
    /// the caller's `save()` aborts before writing the new bytes.
    ///
    /// Emits `Backing up config file at "<path>"` to stdout before copying,
    /// only when the source file actually exists (ophis `manager.go:166`
    /// parity). No notice is printed when the file is absent (no-op path).
    fn backup(&self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        println!("Backing up config file at \"{}\"", self.path.display());
        let dest = backup_path(&self.path);
        std::fs::copy(&self.path, &dest).map_err(|e| Error::EditorConfigBackup {
            path: dest,
            source: e,
        })?;
        Ok(())
    }

    /// Insert or replace `name` in the server map, then persist.
    ///
    /// Prints the existing-server warning (`MCP server "NAME" already
    /// exists and will be overwritten`) to stdout when the name already
    /// exists. Per PLAN §11 #4, no emoji prefix.
    pub(crate) fn enable_server(&mut self, name: &str, server: C::Server) -> Result<()> {
        if self.config.has_server(name) {
            println!("MCP server \"{name}\" already exists and will be overwritten");
        }
        self.config.add_server(name.to_string(), server);
        self.save()?;
        println!("Successfully enabled MCP server: \"{name}\"");
        Ok(())
    }

    /// Remove `name` from the server map and persist.
    ///
    /// Per PLAN §11 #5, when the name is not configured, prints
    /// `MCP server "NAME" does not exist` to stdout and returns `Ok(())`
    /// — no error, no exit-code change. No emoji prefix.
    pub(crate) fn disable_server(&mut self, name: &str) -> Result<()> {
        if !self.config.has_server(name) {
            println!("MCP server \"{name}\" does not exist");
            return Ok(());
        }
        self.config.remove_server(name);
        self.save()
    }

    /// Print configured server names to stdout, one per line.
    ///
    /// When the underlying file does not exist, surface the resolved path
    /// so the operator can see where brontes looked. When the config has
    /// zero servers, print a friendly "no servers configured" note rather
    /// than blank output.
    pub(crate) fn print(&self) {
        if !self.path.exists() {
            println!("(no servers configured at {})", self.path.display());
            return;
        }
        let names: Vec<&str> = self.config.server_names().collect();
        if names.is_empty() {
            println!("(no servers configured at {})", self.path.display());
            return;
        }
        for name in names {
            println!("{name}");
        }
    }
}

/// Compute the backup destination for a config path.
///
/// Strips the final extension and appends `.backup.json`. Mirrors ophis
/// `manager.go:178-180` `strings.TrimSuffix(m.configPath, ext) + ".backup.json"`.
fn backup_path(primary: &Path) -> PathBuf {
    let parent = primary.parent().unwrap_or(Path::new(""));
    let stem = primary
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    parent.join(format!("{stem}.backup.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_path_strips_extension() {
        assert_eq!(
            backup_path(Path::new("/tmp/claude_desktop_config.json")),
            PathBuf::from("/tmp/claude_desktop_config.backup.json")
        );
    }

    #[test]
    fn backup_path_handles_no_extension() {
        assert_eq!(
            backup_path(Path::new("/tmp/config")),
            PathBuf::from("/tmp/config.backup.json")
        );
    }
}
