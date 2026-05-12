//! Per-OS path resolution for editor configuration files.
//!
//! Each helper mirrors the ophis `internal/cfgmgr/manager/<editor>/<editor>_{darwin,linux,windows}.go`
//! family verbatim, lifted into a single Rust module that is `cfg`-gated by
//! `target_os`. The Rust analog of Go's `os.UserHomeDir()` is
//! [`dirs::home_dir`]; on `None` we take the per-row fallback chain
//! documented in `PLAN.md` §3.1.
//!
//! Tasks #5 (Cursor) and #6 (`VSCode`) layer their own resolvers next to
//! [`claude_config_path`] following the same `cfg`-gated shape; the trait of
//! "primary path + fallback when home resolution fails" is the surface area
//! every editor shares.

use std::path::PathBuf;

/// Default Claude Desktop config path for the current platform.
///
/// macOS: `$HOME_DIR/Library/Application Support/Claude/claude_desktop_config.json`,
/// falling back to `/Users/$USER/...` when `dirs::home_dir()` returns `None`.
///
/// Linux: respects `$XDG_CONFIG_HOME` (this is the ONLY editor that consults
/// XDG); otherwise `$HOME_DIR/.config/Claude/...`, falling back to
/// `/home/$USER/.config/Claude/...`.
///
/// Windows: reads `$APPDATA`, then `$USERPROFILE\AppData\Roaming\...`, then
/// the literal `C:\Users\Default\AppData\Roaming\...` — no separate home-
/// unresolved branch (the chain is entirely env-driven per
/// `claude_windows.go:9-19`).
#[must_use]
pub(crate) fn claude_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        claude_config_path_macos_from(home.as_deref(), &user)
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir();
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let user = std::env::var("USER").unwrap_or_default();
        claude_config_path_linux_from(home.as_deref(), xdg.as_deref(), &user)
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok();
        let userprofile = std::env::var("USERPROFILE").ok();
        claude_config_path_windows_from(appdata.as_deref(), userprofile.as_deref())
    }
    // Fallback for non-tier-1 targets (BSD, illumos, etc.): treat as Linux.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let home = dirs::home_dir();
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let user = std::env::var("USER").unwrap_or_default();
        claude_config_path_linux_from(home.as_deref(), xdg.as_deref(), &user)
    }
}

// ── pure path builders ───────────────────────────────────────────────────
// Each per-OS resolver takes every input as a parameter (testable without
// process-env mutation). The public [`claude_config_path`] wrapper pulls
// those inputs from `dirs::home_dir()` and `std::env::var(...)`. Reading
// env vars is safe; only mutating them is `unsafe` in Rust 2024 — and the
// crate forbids `unsafe_code` at the root.

/// Pure macOS resolver. `home` is `Some` when `dirs::home_dir()` resolves;
/// `user` is the value of `$USER` (used only in the home-unresolved path).
///
/// `cfg`-gated on macOS or `test` so the function compiles (and runs its
/// tests) on every host while the linux/windows lib builds don't see it as
/// dead code.
#[cfg(any(target_os = "macos", test))]
fn claude_config_path_macos_from(home: Option<&std::path::Path>, user: &str) -> PathBuf {
    if let Some(home) = home {
        return home
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json");
    }
    PathBuf::from("/Users")
        .join(user)
        .join("Library")
        .join("Application Support")
        .join("Claude")
        .join("claude_desktop_config.json")
}

/// Pure Linux resolver. `home` is `Some` when `dirs::home_dir()` resolves;
/// `xdg_config_home` is the value of `$XDG_CONFIG_HOME` (empty/`None`
/// triggers the `$HOME/.config` fallback); `user` is `$USER` for the
/// home-unresolved branch.
///
/// `cfg`-gated on Linux (or any non-tier-1 target, which also routes here)
/// or `test` so the function compiles (and runs its tests) on every host
/// while macos/windows lib builds don't see it as dead code.
#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows")),
    test
))]
fn claude_config_path_linux_from(
    home: Option<&std::path::Path>,
    xdg_config_home: Option<&str>,
    user: &str,
) -> PathBuf {
    if let Some(home) = home {
        // Claude on Linux is the ONLY editor that consults XDG_CONFIG_HOME.
        let cfg_root = match xdg_config_home {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => home.join(".config"),
        };
        return cfg_root.join("Claude").join("claude_desktop_config.json");
    }
    PathBuf::from("/home")
        .join(user)
        .join(".config")
        .join("Claude")
        .join("claude_desktop_config.json")
}

/// Pure Windows resolver. Mirrors `claude_windows.go:9-19` — APPDATA wins,
/// then USERPROFILE + `AppData\Roaming`, then the literal
/// `C:\Users\Default\AppData\Roaming`. Each input is an `Option<&str>` so
/// the resolver is testable without process-env mutation.
///
/// `cfg`-gated on Windows or `test` so the function compiles (and runs its
/// tests) on every host while the linux/macos lib builds don't see it as
/// dead code.
#[cfg(any(target_os = "windows", test))]
fn claude_config_path_windows_from(appdata: Option<&str>, userprofile: Option<&str>) -> PathBuf {
    if let Some(v) = appdata.filter(|s| !s.is_empty()) {
        return PathBuf::from(v)
            .join("Claude")
            .join("claude_desktop_config.json");
    }
    if let Some(v) = userprofile.filter(|s| !s.is_empty()) {
        return PathBuf::from(v)
            .join("AppData")
            .join("Roaming")
            .join("Claude")
            .join("claude_desktop_config.json");
    }
    PathBuf::from(r"C:\Users\Default\AppData\Roaming")
        .join("Claude")
        .join("claude_desktop_config.json")
}

/// Strip exactly one trailing extension from the file-stem portion of a
/// path, matching ophis `manager.DeriveServerName` (`utils.go:13-20`).
///
/// `foo` -> `foo`. `foo.exe` -> `foo`. `foo.tar.exe` -> `foo.tar`.
/// `/usr/local/bin/myapp.exe` -> `myapp`. Used by `mcp claude {enable,disable}`
/// to derive the server name from the current executable when the user did
/// not pass `--server-name`.
#[must_use]
pub(crate) fn derive_server_name(executable_path: &std::path::Path) -> String {
    let base = executable_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // ophis Go: filepath.Ext on the basename returns the FINAL extension
    // including the dot, or empty when none. We trim one trailing extension.
    if let Some(idx) = base.rfind('.') {
        // Guard against a leading dot (e.g. ".bashrc") which Go's filepath.Ext
        // treats as no extension; ophis's behavior on such a name is "return
        // basename verbatim". `rfind('.') == Some(0)` indicates a dotfile.
        if idx == 0 {
            return base;
        }
        return base[..idx].to_string();
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn derive_server_name_plain_basename() {
        assert_eq!(
            derive_server_name(Path::new("/usr/local/bin/myapp")),
            "myapp"
        );
    }

    #[test]
    fn derive_server_name_strips_exe() {
        assert_eq!(
            derive_server_name(Path::new("/usr/local/bin/myapp.exe")),
            "myapp"
        );
    }

    #[test]
    fn derive_server_name_strips_one_extension_only() {
        assert_eq!(derive_server_name(Path::new("/tmp/foo.tar.exe")), "foo.tar");
    }

    #[test]
    fn derive_server_name_relative_path() {
        assert_eq!(derive_server_name(Path::new("./myapp")), "myapp");
    }

    #[test]
    fn derive_server_name_no_directory() {
        assert_eq!(derive_server_name(Path::new("kubectl")), "kubectl");
    }

    #[test]
    fn derive_server_name_dotfile_returned_verbatim() {
        // ophis filepath.Ext on ".bashrc" returns "" so the basename is
        // returned verbatim; we mirror that intent (no leading-dot strip).
        assert_eq!(derive_server_name(Path::new(".bashrc")), ".bashrc");
    }

    // ── per-OS pure-resolver tests. Each test drives the `*_from` helper
    // with synthetic args so no process-env mutation is needed — the crate
    // forbids `unsafe_code`, and Rust 2024 marks `std::env::set_var` as
    // `unsafe`. Driving the pure functions directly sidesteps that
    // conflict and runs on every host regardless of `target_os`. ──────

    // ── macOS pure resolver ───────────────────────────────────────────

    #[test]
    fn macos_uses_application_support_when_home_resolves() {
        let path = claude_config_path_macos_from(Some(Path::new("/Users/synthetic")), "synthetic");
        assert_eq!(
            path,
            PathBuf::from(
                "/Users/synthetic/Library/Application Support/Claude/claude_desktop_config.json"
            )
        );
    }

    #[test]
    fn macos_falls_back_to_users_user_when_home_unresolved() {
        let path = claude_config_path_macos_from(None, "fallback");
        assert_eq!(
            path,
            PathBuf::from(
                "/Users/fallback/Library/Application Support/Claude/claude_desktop_config.json"
            )
        );
    }

    // ── Linux pure resolver ───────────────────────────────────────────

    #[test]
    fn linux_uses_dollar_home_dot_config_when_xdg_unset() {
        let path =
            claude_config_path_linux_from(Some(Path::new("/home/synthetic")), None, "synthetic");
        assert_eq!(
            path,
            PathBuf::from("/home/synthetic/.config/Claude/claude_desktop_config.json")
        );
    }

    #[test]
    fn linux_uses_dollar_home_dot_config_when_xdg_empty() {
        // Empty XDG_CONFIG_HOME must fall through to the $HOME/.config path
        // (matches ophis behavior on `os.Getenv` returning the empty string).
        let path = claude_config_path_linux_from(
            Some(Path::new("/home/synthetic")),
            Some(""),
            "synthetic",
        );
        assert_eq!(
            path,
            PathBuf::from("/home/synthetic/.config/Claude/claude_desktop_config.json")
        );
    }

    #[test]
    fn linux_honors_xdg_config_home() {
        let path = claude_config_path_linux_from(
            Some(Path::new("/home/synthetic")),
            Some("/custom/xdg"),
            "synthetic",
        );
        assert_eq!(
            path,
            PathBuf::from("/custom/xdg/Claude/claude_desktop_config.json")
        );
    }

    #[test]
    fn linux_falls_back_to_home_user_when_home_unresolved() {
        let path = claude_config_path_linux_from(None, None, "fallback");
        assert_eq!(
            path,
            PathBuf::from("/home/fallback/.config/Claude/claude_desktop_config.json")
        );
    }

    // ── Windows pure resolver ─────────────────────────────────────────

    #[test]
    fn windows_prefers_appdata() {
        let path = claude_config_path_windows_from(
            Some(r"C:\Users\synth\AppData\Roaming"),
            Some(r"C:\Users\synth"),
        );
        let components: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert!(components.contains(&"Claude".to_string()));
        assert!(components.contains(&"claude_desktop_config.json".to_string()));
        assert!(
            components.iter().any(|c| c.contains("Roaming")),
            "must include the Roaming segment from APPDATA, got {components:?}"
        );
    }

    #[test]
    fn windows_falls_back_to_userprofile_when_appdata_empty() {
        let path = claude_config_path_windows_from(Some(""), Some(r"C:\Users\synth"));
        let s = path.to_string_lossy();
        assert!(s.contains("AppData"), "got {s}");
        assert!(s.contains("Roaming"), "got {s}");
        assert!(s.contains("Claude"), "got {s}");
    }

    #[test]
    fn windows_falls_back_to_userprofile_when_appdata_none() {
        let path = claude_config_path_windows_from(None, Some(r"C:\Users\synth"));
        let s = path.to_string_lossy();
        assert!(s.contains("AppData"), "got {s}");
        assert!(s.contains("Roaming"), "got {s}");
        assert!(s.contains("Claude"), "got {s}");
    }

    #[test]
    fn windows_default_users_fallback() {
        // Both env vars missing → literal C:\Users\Default\AppData\Roaming.
        let path = claude_config_path_windows_from(None, None);
        let s = path.to_string_lossy().into_owned();
        assert!(s.contains("Default"), "got {s}");
        assert!(s.contains("Claude"), "got {s}");
    }
}
