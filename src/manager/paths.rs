//! Per-OS path resolution for editor configuration files.
//!
//! Each helper mirrors the ophis `internal/cfgmgr/manager/<editor>/<editor>_{darwin,linux,windows}.go`
//! family verbatim, lifted into a single Rust module that is `cfg`-gated by
//! `target_os`. The Rust analog of Go's `os.UserHomeDir()` is
//! [`dirs::home_dir`]; on `None` we take the per-row fallback chain.
//!
//! Cursor and `VSCode` layer their own resolvers next to
//! [`claude_config_path`] following the same `cfg`-gated shape; the trait of
//! "primary path + fallback when home resolution fails" is the surface area
//! every editor shares. `VSCode` and Cursor on Linux do NOT consult
//! `XDG_CONFIG_HOME` — that is Claude-only behavior.

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
pub fn claude_config_path() -> PathBuf {
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

/// Default Cursor user-mode `mcp.json` path for the current platform.
///
/// macOS: `$HOME_DIR/.cursor/mcp.json`, falling back to
/// `/Users/$USER/.cursor/mcp.json` when `dirs::home_dir()` returns `None`.
///
/// Linux: `$HOME_DIR/.cursor/mcp.json`, falling back to
/// `/home/$USER/.cursor/mcp.json` when home is unresolved. Cursor on Linux
/// does NOT consult `XDG_CONFIG_HOME` — that's Claude-only behavior.
///
/// Windows: `$HOME_DIR/.cursor/mcp.json` (where `$HOME_DIR` is `dirs::home_dir`,
/// which on Windows resolves `%USERPROFILE%`), falling back to
/// `$USERPROFILE\.cursor\mcp.json` from a direct env read when home is
/// unresolved.
#[must_use]
pub fn cursor_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        cursor_config_path_macos_from(home.as_deref(), &user)
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        cursor_config_path_linux_from(home.as_deref(), &user)
    }
    #[cfg(target_os = "windows")]
    {
        let home = dirs::home_dir();
        let userprofile = std::env::var("USERPROFILE").ok();
        cursor_config_path_windows_from(home.as_deref(), userprofile.as_deref())
    }
    // Fallback for non-tier-1 targets (BSD, illumos, etc.): treat as Linux.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        cursor_config_path_linux_from(home.as_deref(), &user)
    }
}

/// Cursor workspace-mode `mcp.json` path: `$CWD/.cursor/mcp.json`, falling
/// back to the relative `.cursor/mcp.json` when `std::env::current_dir()`
/// fails (matches ophis behavior).
#[must_use]
pub fn cursor_workspace_path() -> PathBuf {
    cursor_workspace_path_from(std::env::current_dir().ok().as_deref())
}

/// Pure macOS resolver for Cursor user-mode. `home` is `Some` when
/// `dirs::home_dir()` resolves; `user` is `$USER` for the home-unresolved
/// branch.
#[cfg(any(target_os = "macos", test))]
fn cursor_config_path_macos_from(home: Option<&std::path::Path>, user: &str) -> PathBuf {
    if let Some(home) = home {
        return home.join(".cursor").join("mcp.json");
    }
    PathBuf::from("/Users")
        .join(user)
        .join(".cursor")
        .join("mcp.json")
}

/// Pure Linux resolver for Cursor user-mode. `home` is `Some` when
/// `dirs::home_dir()` resolves; `user` is `$USER` for the home-unresolved
/// branch. Cursor on Linux does NOT consult `XDG_CONFIG_HOME`.
#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows")),
    test
))]
fn cursor_config_path_linux_from(home: Option<&std::path::Path>, user: &str) -> PathBuf {
    if let Some(home) = home {
        return home.join(".cursor").join("mcp.json");
    }
    PathBuf::from("/home")
        .join(user)
        .join(".cursor")
        .join("mcp.json")
}

/// Pure Windows resolver for Cursor user-mode. `home` is `Some` when
/// `dirs::home_dir()` resolves; `userprofile` is `$USERPROFILE` for the
/// home-unresolved branch.
#[cfg(any(target_os = "windows", test))]
fn cursor_config_path_windows_from(
    home: Option<&std::path::Path>,
    userprofile: Option<&str>,
) -> PathBuf {
    if let Some(home) = home {
        return home.join(".cursor").join("mcp.json");
    }
    if let Some(v) = userprofile.filter(|s| !s.is_empty()) {
        return PathBuf::from(v).join(".cursor").join("mcp.json");
    }
    // No home, no USERPROFILE — match the relative fallback so we still
    // produce *some* path the caller can present to the user (the error
    // surfaces at file open).
    PathBuf::from(".cursor").join("mcp.json")
}

/// Pure workspace resolver. `cwd` is `Some` when `std::env::current_dir()`
/// resolves; `None` (e.g. cwd deleted out from under us) falls back to the
/// relative `.cursor/mcp.json`.
fn cursor_workspace_path_from(cwd: Option<&std::path::Path>) -> PathBuf {
    if let Some(cwd) = cwd {
        return cwd.join(".cursor").join("mcp.json");
    }
    PathBuf::from(".cursor").join("mcp.json")
}

/// Default `VSCode` user-mode `mcp.json` path for the current platform.
///
/// macOS: `$HOME_DIR/Library/Application Support/Code/User/mcp.json`,
/// falling back to `/Users/$USER/Library/Application Support/Code/User/mcp.json`
/// when `dirs::home_dir()` returns `None`.
///
/// Linux: `$HOME_DIR/.config/Code/User/mcp.json`, falling back to
/// `/home/$USER/.config/Code/User/mcp.json` when home is unresolved.
/// `VSCode` on Linux does NOT consult `XDG_CONFIG_HOME` — that's Claude-only
/// behavior.
///
/// Windows: `$HOME_DIR/AppData/Roaming/Code/User/mcp.json` (where
/// `$HOME_DIR` is `dirs::home_dir`, which on Windows resolves
/// `%USERPROFILE%`), falling back to
/// `$USERPROFILE/AppData/Roaming/Code/User/mcp.json` from a direct env read
/// when home is unresolved.
#[must_use]
pub fn vscode_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        vscode_config_path_macos_from(home.as_deref(), &user)
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        vscode_config_path_linux_from(home.as_deref(), &user)
    }
    #[cfg(target_os = "windows")]
    {
        let home = dirs::home_dir();
        let userprofile = std::env::var("USERPROFILE").ok();
        vscode_config_path_windows_from(home.as_deref(), userprofile.as_deref())
    }
    // Fallback for non-tier-1 targets (BSD, illumos, etc.): treat as Linux.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        vscode_config_path_linux_from(home.as_deref(), &user)
    }
}

/// `VSCode` workspace-mode `mcp.json` path: `$CWD/.vscode/mcp.json`, falling
/// back to the relative `.vscode/mcp.json` when `std::env::current_dir()`
/// fails (matches ophis behavior).
#[must_use]
pub fn vscode_workspace_path() -> PathBuf {
    vscode_workspace_path_from(std::env::current_dir().ok().as_deref())
}

/// Pure macOS resolver for `VSCode` user-mode. `home` is `Some` when
/// `dirs::home_dir()` resolves; `user` is `$USER` for the home-unresolved
/// branch.
#[cfg(any(target_os = "macos", test))]
fn vscode_config_path_macos_from(home: Option<&std::path::Path>, user: &str) -> PathBuf {
    if let Some(home) = home {
        return home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("mcp.json");
    }
    PathBuf::from("/Users")
        .join(user)
        .join("Library")
        .join("Application Support")
        .join("Code")
        .join("User")
        .join("mcp.json")
}

/// Pure Linux resolver for `VSCode` user-mode. `home` is `Some` when
/// `dirs::home_dir()` resolves; `user` is `$USER` for the home-unresolved
/// branch. `VSCode` on Linux does NOT consult `XDG_CONFIG_HOME`.
#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows")),
    test
))]
fn vscode_config_path_linux_from(home: Option<&std::path::Path>, user: &str) -> PathBuf {
    if let Some(home) = home {
        return home
            .join(".config")
            .join("Code")
            .join("User")
            .join("mcp.json");
    }
    PathBuf::from("/home")
        .join(user)
        .join(".config")
        .join("Code")
        .join("User")
        .join("mcp.json")
}

/// Pure Windows resolver for `VSCode` user-mode. `home` is `Some` when
/// `dirs::home_dir()` resolves; `userprofile` is `$USERPROFILE` for the
/// home-unresolved branch.
#[cfg(any(target_os = "windows", test))]
fn vscode_config_path_windows_from(
    home: Option<&std::path::Path>,
    userprofile: Option<&str>,
) -> PathBuf {
    if let Some(home) = home {
        return home
            .join("AppData")
            .join("Roaming")
            .join("Code")
            .join("User")
            .join("mcp.json");
    }
    if let Some(v) = userprofile.filter(|s| !s.is_empty()) {
        return PathBuf::from(v)
            .join("AppData")
            .join("Roaming")
            .join("Code")
            .join("User")
            .join("mcp.json");
    }
    // No home, no USERPROFILE — match Cursor's relative fallback shape
    // (the home-prefix dropped) so we still produce *some* path the caller
    // can present to the user. The error surfaces at file open. ophis's
    // `vscode_windows.go` has no `C:\Users\Default` literal (that pattern
    // is Claude-only), so a relative fallback is more
    // faithful to the parity goal than inventing one.
    PathBuf::from("AppData")
        .join("Roaming")
        .join("Code")
        .join("User")
        .join("mcp.json")
}

/// Pure workspace resolver for `VSCode`. `cwd` is `Some` when
/// `std::env::current_dir()` resolves; `None` (e.g. cwd deleted out from
/// under us) falls back to the relative `.vscode/mcp.json`.
fn vscode_workspace_path_from(cwd: Option<&std::path::Path>) -> PathBuf {
    if let Some(cwd) = cwd {
        return cwd.join(".vscode").join("mcp.json");
    }
    PathBuf::from(".vscode").join("mcp.json")
}

/// Default Zed user-mode `settings.json` path for the current platform.
///
/// macOS: `$HOME_DIR/.config/zed/settings.json` (Zed on macOS follows the
/// XDG-style `~/.config/zed/` layout rather than `Library/Application
/// Support`), falling back to `/Users/$USER/.config/zed/settings.json`
/// when `dirs::home_dir()` returns `None`.
///
/// Linux: respects `$XDG_CONFIG_HOME`; otherwise
/// `$HOME_DIR/.config/zed/settings.json`, falling back to
/// `/home/$USER/.config/zed/settings.json` when home is unresolved.
///
/// Windows: `$APPDATA\Zed\settings.json`, falling back to
/// `$USERPROFILE\AppData\Roaming\Zed\settings.json` when `APPDATA` is
/// missing.
///
/// Mirrors ophis `njayp/ophis#46`'s `zed_{darwin,linux,windows}.go`.
#[must_use]
pub fn zed_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir();
        let user = std::env::var("USER").unwrap_or_default();
        zed_config_path_macos_from(home.as_deref(), &user)
    }
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir();
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let user = std::env::var("USER").unwrap_or_default();
        zed_config_path_linux_from(home.as_deref(), xdg.as_deref(), &user)
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok();
        let userprofile = std::env::var("USERPROFILE").ok();
        zed_config_path_windows_from(appdata.as_deref(), userprofile.as_deref())
    }
    // Fallback for non-tier-1 targets (BSD, illumos, etc.): treat as Linux.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let home = dirs::home_dir();
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let user = std::env::var("USER").unwrap_or_default();
        zed_config_path_linux_from(home.as_deref(), xdg.as_deref(), &user)
    }
}

/// Zed workspace-mode `settings.json` path: `$CWD/.zed/settings.json`,
/// falling back to the relative `.zed/settings.json` when
/// `std::env::current_dir()` fails.
#[must_use]
pub fn zed_workspace_path() -> PathBuf {
    zed_workspace_path_from(std::env::current_dir().ok().as_deref())
}

/// Pure macOS resolver for Zed user-mode.
#[cfg(any(target_os = "macos", test))]
fn zed_config_path_macos_from(home: Option<&std::path::Path>, user: &str) -> PathBuf {
    if let Some(home) = home {
        return home.join(".config").join("zed").join("settings.json");
    }
    PathBuf::from("/Users")
        .join(user)
        .join(".config")
        .join("zed")
        .join("settings.json")
}

/// Pure Linux resolver for Zed user-mode.
///
/// Zed on Linux DOES consult `$XDG_CONFIG_HOME` (unlike Cursor and `VSCode`
/// which do not). The fallback chain matches ophis: XDG → `$HOME/.config`
/// → `/home/$USER/.config`.
#[cfg(any(
    target_os = "linux",
    not(any(target_os = "macos", target_os = "windows")),
    test
))]
fn zed_config_path_linux_from(
    home: Option<&std::path::Path>,
    xdg_config_home: Option<&str>,
    user: &str,
) -> PathBuf {
    if let Some(home) = home {
        let cfg_root = match xdg_config_home {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => home.join(".config"),
        };
        return cfg_root.join("zed").join("settings.json");
    }
    PathBuf::from("/home")
        .join(user)
        .join(".config")
        .join("zed")
        .join("settings.json")
}

/// Pure Windows resolver for Zed user-mode.
///
/// Prefers `%APPDATA%\Zed\settings.json`, falls back to
/// `%USERPROFILE%\AppData\Roaming\Zed\settings.json`. No `C:\Users\Default`
/// literal — Zed's Windows layout does not document that fallback, so we
/// surface a relative path (matching the Cursor/`VSCode` pattern) rather
/// than invent one.
#[cfg(any(target_os = "windows", test))]
fn zed_config_path_windows_from(appdata: Option<&str>, userprofile: Option<&str>) -> PathBuf {
    if let Some(v) = appdata.filter(|s| !s.is_empty()) {
        return PathBuf::from(v).join("Zed").join("settings.json");
    }
    if let Some(v) = userprofile.filter(|s| !s.is_empty()) {
        return PathBuf::from(v)
            .join("AppData")
            .join("Roaming")
            .join("Zed")
            .join("settings.json");
    }
    PathBuf::from("AppData")
        .join("Roaming")
        .join("Zed")
        .join("settings.json")
}

/// Pure workspace resolver for Zed. `cwd` is `Some` when
/// `std::env::current_dir()` resolves; `None` falls back to the relative
/// `.zed/settings.json` (matches the Cursor/`VSCode` shape).
fn zed_workspace_path_from(cwd: Option<&std::path::Path>) -> PathBuf {
    if let Some(cwd) = cwd {
        return cwd.join(".zed").join("settings.json");
    }
    PathBuf::from(".zed").join("settings.json")
}

/// Strip exactly one trailing extension from the file-stem portion of a
/// path, matching ophis `manager.DeriveServerName` (`utils.go:13-20`).
///
/// `foo` -> `foo`. `foo.exe` -> `foo`. `foo.tar.exe` -> `foo.tar`.
/// `/usr/local/bin/myapp.exe` -> `myapp`. Used by `mcp claude {enable,disable}`
/// to derive the server name from the current executable when the user did
/// not pass `--server-name`.
#[must_use]
pub fn derive_server_name(executable_path: &std::path::Path) -> String {
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

    // ── Cursor (user) macOS resolver ──────────────────────────────────

    #[test]
    fn cursor_macos_uses_home_dot_cursor_when_home_resolves() {
        let path = cursor_config_path_macos_from(Some(Path::new("/Users/synthetic")), "synthetic");
        assert_eq!(path, PathBuf::from("/Users/synthetic/.cursor/mcp.json"));
    }

    #[test]
    fn cursor_macos_falls_back_to_users_user_when_home_unresolved() {
        let path = cursor_config_path_macos_from(None, "fallback");
        assert_eq!(path, PathBuf::from("/Users/fallback/.cursor/mcp.json"));
    }

    // ── Cursor (user) Linux resolver ──────────────────────────────────

    #[test]
    fn cursor_linux_uses_home_dot_cursor_when_home_resolves() {
        let path = cursor_config_path_linux_from(Some(Path::new("/home/synthetic")), "synthetic");
        assert_eq!(path, PathBuf::from("/home/synthetic/.cursor/mcp.json"));
    }

    #[test]
    fn cursor_linux_falls_back_to_home_user_when_home_unresolved() {
        let path = cursor_config_path_linux_from(None, "fallback");
        assert_eq!(path, PathBuf::from("/home/fallback/.cursor/mcp.json"));
    }

    #[test]
    fn cursor_linux_does_not_consult_xdg() {
        // Cursor on Linux must NOT consult XDG_CONFIG_HOME.
        // The resolver signature doesn't even accept an XDG argument — this
        // test pins the surface so a future "let's consolidate the linux
        // resolvers" refactor doesn't accidentally route Cursor through XDG.
        let path = cursor_config_path_linux_from(Some(Path::new("/home/synthetic")), "synthetic");
        let s = path.to_string_lossy();
        assert!(
            !s.contains(".config"),
            "must not route through .config: {s}"
        );
    }

    // ── Cursor (user) Windows resolver ────────────────────────────────

    #[test]
    fn cursor_windows_uses_home_when_resolves() {
        let path = cursor_config_path_windows_from(
            Some(Path::new(r"C:\Users\synth")),
            Some(r"C:\Users\synth"),
        );
        let s = path.to_string_lossy();
        assert!(s.contains(".cursor"), "got {s}");
        assert!(s.contains("mcp.json"), "got {s}");
    }

    #[test]
    fn cursor_windows_falls_back_to_userprofile_when_home_unresolved() {
        let path = cursor_config_path_windows_from(None, Some(r"C:\Users\synth"));
        let s = path.to_string_lossy();
        assert!(s.contains(r"C:\Users\synth"), "got {s}");
        assert!(s.contains(".cursor"), "got {s}");
    }

    #[test]
    fn cursor_windows_relative_fallback_when_all_unresolved() {
        // No home, no USERPROFILE — fall back to relative `.cursor/mcp.json`
        // so the caller still gets a usable PathBuf; error surfaces at open.
        let path = cursor_config_path_windows_from(None, None);
        assert_eq!(path, PathBuf::from(r".cursor").join("mcp.json"));
    }

    // ── Cursor workspace-mode resolver ────────────────────────────────

    #[test]
    fn cursor_workspace_uses_cwd_when_resolves() {
        let path = cursor_workspace_path_from(Some(Path::new("/tmp/myproj")));
        assert_eq!(path, PathBuf::from("/tmp/myproj/.cursor/mcp.json"));
    }

    #[test]
    fn cursor_workspace_falls_back_to_relative_when_cwd_unresolved() {
        let path = cursor_workspace_path_from(None);
        assert_eq!(path, PathBuf::from(".cursor").join("mcp.json"));
    }

    // ── VSCode (user) macOS resolver ──────────────────────────────────

    #[test]
    fn vscode_macos_uses_application_support_code_when_home_resolves() {
        let path = vscode_config_path_macos_from(Some(Path::new("/Users/synthetic")), "synthetic");
        assert_eq!(
            path,
            PathBuf::from("/Users/synthetic/Library/Application Support/Code/User/mcp.json")
        );
    }

    #[test]
    fn vscode_macos_falls_back_to_users_user_when_home_unresolved() {
        let path = vscode_config_path_macos_from(None, "fallback");
        assert_eq!(
            path,
            PathBuf::from("/Users/fallback/Library/Application Support/Code/User/mcp.json")
        );
    }

    // ── VSCode (user) Linux resolver ──────────────────────────────────

    #[test]
    fn vscode_linux_uses_home_dot_config_code_when_home_resolves() {
        let path = vscode_config_path_linux_from(Some(Path::new("/home/synthetic")), "synthetic");
        assert_eq!(
            path,
            PathBuf::from("/home/synthetic/.config/Code/User/mcp.json")
        );
    }

    #[test]
    fn vscode_linux_falls_back_to_home_user_when_home_unresolved() {
        let path = vscode_config_path_linux_from(None, "fallback");
        assert_eq!(
            path,
            PathBuf::from("/home/fallback/.config/Code/User/mcp.json")
        );
    }

    #[test]
    fn vscode_linux_does_not_consult_xdg() {
        // VSCode on Linux must NOT consult XDG_CONFIG_HOME.
        // The resolver signature doesn't even accept an XDG argument — this
        // test pins the surface so a future "let's consolidate the linux
        // resolvers" refactor doesn't accidentally route VSCode through XDG.
        let path = vscode_config_path_linux_from(Some(Path::new("/home/synthetic")), "synthetic");
        // The resolver MUST produce `$HOME/.config/Code/User/mcp.json` — the
        // `.config` segment here is part of the VSCode-specific path layout,
        // NOT an XDG redirect. Compare component-wise (via PathBuf equality)
        // so the assertion remains portable across host OSes.
        assert_eq!(
            path,
            PathBuf::from("/home/synthetic/.config/Code/User/mcp.json")
        );
    }

    // ── VSCode (user) Windows resolver ────────────────────────────────

    #[test]
    fn vscode_windows_uses_home_when_resolves() {
        let path = vscode_config_path_windows_from(
            Some(Path::new(r"C:\Users\synth")),
            Some(r"C:\Users\synth"),
        );
        let s = path.to_string_lossy();
        assert!(s.contains("AppData"), "got {s}");
        assert!(s.contains("Roaming"), "got {s}");
        assert!(s.contains("Code"), "got {s}");
        assert!(s.contains("User"), "got {s}");
        assert!(s.contains("mcp.json"), "got {s}");
    }

    #[test]
    fn vscode_windows_falls_back_to_userprofile_when_home_unresolved() {
        let path = vscode_config_path_windows_from(None, Some(r"C:\Users\synth"));
        let s = path.to_string_lossy();
        assert!(s.contains(r"C:\Users\synth"), "got {s}");
        assert!(s.contains("AppData"), "got {s}");
        assert!(s.contains("Roaming"), "got {s}");
        assert!(s.contains("Code"), "got {s}");
        assert!(s.contains("User"), "got {s}");
        assert!(s.contains("mcp.json"), "got {s}");
    }

    #[test]
    fn vscode_windows_relative_fallback_when_all_unresolved() {
        // No home, no USERPROFILE — fall back to a relative path with the
        // home-prefix dropped, mirroring Cursor's tertiary shape. ophis's
        // `vscode_windows.go` has no `C:\Users\Default` literal (that
        // pattern is Claude-only), so we do not invent
        // an absolute fallback here. The error surfaces at file open.
        let path = vscode_config_path_windows_from(None, None);
        assert_eq!(
            path,
            PathBuf::from("AppData")
                .join("Roaming")
                .join("Code")
                .join("User")
                .join("mcp.json")
        );
        assert!(path.is_relative(), "tertiary fallback must be relative");
    }

    // ── VSCode workspace-mode resolver ────────────────────────────────

    #[test]
    fn vscode_workspace_uses_cwd_when_resolves() {
        let path = vscode_workspace_path_from(Some(Path::new("/tmp/myproj")));
        assert_eq!(path, PathBuf::from("/tmp/myproj/.vscode/mcp.json"));
    }

    #[test]
    fn vscode_workspace_falls_back_to_relative_when_cwd_unresolved() {
        let path = vscode_workspace_path_from(None);
        assert_eq!(path, PathBuf::from(".vscode").join("mcp.json"));
    }

    // ── Zed (user) macOS resolver ─────────────────────────────────────

    #[test]
    fn zed_macos_uses_dot_config_zed_when_home_resolves() {
        // Zed on macOS does NOT use `Library/Application Support` (that is
        // Claude's macOS layout). It uses `~/.config/zed/settings.json`
        // matching ophis `zed_darwin.go`.
        let path = zed_config_path_macos_from(Some(Path::new("/Users/synthetic")), "synthetic");
        assert_eq!(
            path,
            PathBuf::from("/Users/synthetic/.config/zed/settings.json")
        );
    }

    #[test]
    fn zed_macos_falls_back_to_users_user_when_home_unresolved() {
        let path = zed_config_path_macos_from(None, "fallback");
        assert_eq!(
            path,
            PathBuf::from("/Users/fallback/.config/zed/settings.json")
        );
    }

    // ── Zed (user) Linux resolver ─────────────────────────────────────

    #[test]
    fn zed_linux_uses_dollar_home_dot_config_when_xdg_unset() {
        let path =
            zed_config_path_linux_from(Some(Path::new("/home/synthetic")), None, "synthetic");
        assert_eq!(
            path,
            PathBuf::from("/home/synthetic/.config/zed/settings.json")
        );
    }

    #[test]
    fn zed_linux_uses_dollar_home_dot_config_when_xdg_empty() {
        // Empty XDG_CONFIG_HOME must fall through to the $HOME/.config path
        // (matches ophis behavior on `os.Getenv` returning the empty string).
        let path =
            zed_config_path_linux_from(Some(Path::new("/home/synthetic")), Some(""), "synthetic");
        assert_eq!(
            path,
            PathBuf::from("/home/synthetic/.config/zed/settings.json")
        );
    }

    #[test]
    fn zed_linux_honors_xdg_config_home() {
        // Zed on Linux respects XDG_CONFIG_HOME (Claude also does; Cursor
        // and VSCode do NOT). Verify the XDG-pointed root is honored.
        let path = zed_config_path_linux_from(
            Some(Path::new("/home/synthetic")),
            Some("/custom/xdg"),
            "synthetic",
        );
        assert_eq!(path, PathBuf::from("/custom/xdg/zed/settings.json"));
    }

    #[test]
    fn zed_linux_falls_back_to_home_user_when_home_unresolved() {
        let path = zed_config_path_linux_from(None, None, "fallback");
        assert_eq!(
            path,
            PathBuf::from("/home/fallback/.config/zed/settings.json")
        );
    }

    // ── Zed (user) Windows resolver ───────────────────────────────────

    #[test]
    fn zed_windows_prefers_appdata() {
        let path = zed_config_path_windows_from(
            Some(r"C:\Users\synth\AppData\Roaming"),
            Some(r"C:\Users\synth"),
        );
        let s = path.to_string_lossy();
        assert!(s.contains("Zed"), "got {s}");
        assert!(s.contains("settings.json"), "got {s}");
        assert!(
            s.contains("Roaming"),
            "must include the Roaming segment from APPDATA, got {s}"
        );
    }

    #[test]
    fn zed_windows_falls_back_to_userprofile_when_appdata_empty() {
        let path = zed_config_path_windows_from(Some(""), Some(r"C:\Users\synth"));
        let s = path.to_string_lossy();
        assert!(s.contains("AppData"), "got {s}");
        assert!(s.contains("Roaming"), "got {s}");
        assert!(s.contains("Zed"), "got {s}");
    }

    #[test]
    fn zed_windows_falls_back_to_userprofile_when_appdata_none() {
        let path = zed_config_path_windows_from(None, Some(r"C:\Users\synth"));
        let s = path.to_string_lossy();
        assert!(s.contains("AppData"), "got {s}");
        assert!(s.contains("Roaming"), "got {s}");
        assert!(s.contains("Zed"), "got {s}");
    }

    #[test]
    fn zed_windows_relative_fallback_when_all_unresolved() {
        // Both env vars missing — fall back to a relative path so the caller
        // still gets a usable PathBuf; the error surfaces at file open.
        let path = zed_config_path_windows_from(None, None);
        assert_eq!(
            path,
            PathBuf::from("AppData")
                .join("Roaming")
                .join("Zed")
                .join("settings.json")
        );
    }

    // ── Zed workspace-mode resolver ───────────────────────────────────

    #[test]
    fn zed_workspace_uses_cwd_when_resolves() {
        let path = zed_workspace_path_from(Some(Path::new("/tmp/myproj")));
        assert_eq!(path, PathBuf::from("/tmp/myproj/.zed/settings.json"));
    }

    #[test]
    fn zed_workspace_falls_back_to_relative_when_cwd_unresolved() {
        let path = zed_workspace_path_from(None);
        assert_eq!(path, PathBuf::from(".zed").join("settings.json"));
    }
}
