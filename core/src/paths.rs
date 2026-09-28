/**
 * Canonical on-disk layout: EVERYTHING Comrade owns lives under one roof.
 * - Linux / macOS: `$COMRADE_HOME`, else `$XDG_CONFIG_HOME/comrade-agent`,
 *   else `~/.config/comrade-agent`
 * - Windows: `%APPDATA%/comrade-agent`
 *
 * Layout:
 * ```
 * comrade-agent/
 *   comrade-memory.db      SQLite vector memory
 *   comrade-memory.json    legacy memory (one-time migration source)
 *   brave-profile/         persistent browser profile
 *   sysroot/               vendored webkit libs (rootless Linux dev only)
 *   shim/webkit-shim.so    webkit path shim (rootless Linux dev only)
 * ```
 */
use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "comrade-agent";
/// Pre-consolidation Tauri bundle identifier (old DB location).
const LEGACY_IDENTIFIER: &str = "com.comrade.desktop";

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

pub fn comrade_home() -> PathBuf {
    if let Ok(custom) = std::env::var("COMRADE_HOME") {
        if !custom.trim().is_empty() {
            return PathBuf::from(custom);
        }
    }
    if std::env::consts::OS == "windows" {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                return PathBuf::from(appdata).join(APP_DIR_NAME);
            }
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            if !profile.trim().is_empty() {
                return PathBuf::from(profile).join(APP_DIR_NAME);
            }
        }
        return PathBuf::from(APP_DIR_NAME);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| format!("{}/.config", home_dir()));
    PathBuf::from(base).join(APP_DIR_NAME)
}

pub fn ensure_home() -> std::io::Result<PathBuf> {
    let home = comrade_home();
    std::fs::create_dir_all(&home)?;
    Ok(home)
}

pub fn memory_db_path() -> PathBuf {
    comrade_home().join("comrade-memory.db")
}

pub fn profile_dir_default() -> PathBuf {
    // One shared dir for the selected browser — never per-browser duplicates.
    comrade_home().join("browser-profile")
}

/// Previous DB location (Tauri app_data_dir, pre-consolidation) for one-time move.
pub fn legacy_db_path() -> PathBuf {
    if std::env::consts::OS == "windows" {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base).join(LEGACY_IDENTIFIER).join("comrade-memory.db")
    } else if std::env::consts::OS == "macos" {
        PathBuf::from(home_dir())
            .join("Library/Application Support")
            .join(LEGACY_IDENTIFIER)
            .join("comrade-memory.db")
    } else {
        let data_home = std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| format!("{}/.local/share", home_dir()));
        PathBuf::from(data_home).join(LEGACY_IDENTIFIER).join("comrade-memory.db")
    }
}

pub fn legacy_json_path() -> PathBuf {
    legacy_db_path().with_file_name("comrade-memory.json")
}

/// Vendored webkit sysroot (rootless Linux dev): new home first,
/// legacy `~/comrade-sysroot` fallback so existing installs keep working.
pub fn sysroot_dir() -> PathBuf {
    let preferred = comrade_home().join("sysroot");
    if preferred.join("usr/lib/pkgconfig/webkit2gtk-4.1.pc").exists() {
        return preferred;
    }
    if std::env::consts::OS != "windows" {
        let legacy = PathBuf::from(home_dir()).join("comrade-sysroot");
        if legacy.join("usr/lib/pkgconfig/webkit2gtk-4.1.pc").exists() {
            return legacy;
        }
    }
    preferred
}

pub fn webkit_helper_dir() -> PathBuf {
    sysroot_dir().join("usr/lib/webkit2gtk-4.1")
}

/// Path shim .so: new home first, legacy `~/.local/lib/comrade` fallback.
pub fn webkit_shim_path() -> PathBuf {
    let preferred = comrade_home().join("shim/webkit-shim.so");
    if preferred.exists() {
        return preferred;
    }
    PathBuf::from(home_dir()).join(".local/lib/comrade/webkit-shim.so")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_resolution_is_consolidated() {
        // Sequential in one test: env mutation is process-global.
        std::env::set_var("COMRADE_HOME", "/tmp/x-comrade-test-home");
        assert_eq!(comrade_home(), PathBuf::from("/tmp/x-comrade-test-home"));
        assert_eq!(
            memory_db_path(),
            PathBuf::from("/tmp/x-comrade-test-home/comrade-memory.db")
        );
        std::env::remove_var("COMRADE_HOME");
        assert!(comrade_home().ends_with(APP_DIR_NAME));
        assert!(memory_db_path().ends_with("comrade-agent/comrade-memory.db"));
        assert!(profile_dir_default().ends_with("comrade-agent/browser-profile"));
    }
}
