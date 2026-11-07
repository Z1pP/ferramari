//! File-system locations, environment detection and process-wide environment
//! preparation.
//!
//! All knowledge about *where things live* is kept here so the rest of the
//! program never builds paths by hand.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(target_os = "linux")]
use std::time::Duration;

pub const APP_ID: &str = "io.github.ferramari.Ferramari";
pub const SERVER_NAME: &str = "io.github.ferramari.Ferramari.server";
pub const TRANSLATION_DOMAIN: &str = "ferramari";

/// User home directory, independent of the host platform.
pub fn home() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "windows")]
fn config_base() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData/Roaming"))
}

#[cfg(target_os = "macos")]
fn config_base() -> PathBuf {
    home().join("Library/Application Support")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn config_base() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

#[cfg(target_os = "windows")]
fn cache_base() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData/Local"))
}

#[cfg(target_os = "macos")]
fn cache_base() -> PathBuf {
    home().join("Library/Caches")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn cache_base() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
}

#[cfg(target_os = "windows")]
fn data_base() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData/Local"))
}

#[cfg(target_os = "macos")]
fn data_base() -> PathBuf {
    home().join("Library/Application Support")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn data_base() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
}

pub fn config_dir() -> PathBuf {
    xdg_config_home().join("ferramari")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Favourites, history and collections. Kept separate from `config.json`.
pub fn library_path() -> PathBuf {
    config_dir().join("library.json")
}

pub fn autostart_dir() -> PathBuf {
    config_base().join("autostart")
}

pub fn autostart_path() -> PathBuf {
    autostart_dir().join(format!("{APP_ID}.desktop"))
}

pub fn thumbnail_cache_dir() -> PathBuf {
    cache_base().join("ferramari").join("thumbnails")
}

/// Cached blurred frames used for the GNOME static wallpaper.
pub fn static_cache_dir() -> PathBuf {
    xdg_cache_home().join("ferramari").join("static")
}

/// Installed `.ferramari` packages.
pub fn packages_dir() -> PathBuf {
    data_base().join("ferramari").join("packages")
}

/// Declarative plugin manifests.
pub fn plugins_dir() -> PathBuf {
    xdg_data_home().join("ferramari").join("plugins")
}

/// Cache for the online wallpaper catalogue (pages, previews, downloads).
pub fn catalogue_cache_dir() -> PathBuf {
    xdg_cache_home().join("ferramari").join("catalogue")
}

/// mpv IPC sockets. Kept in the cache rather than `/tmp` so a host mpv
/// started through `flatpak-spawn` can reach the same path.
pub fn mpv_socket_dir() -> PathBuf {
    xdg_cache_home().join("ferramari").join("mpv")
}

/// User folder for local wallpaper videos.
pub fn video_wallpaper_dir() -> PathBuf {
    static VIDEO_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    VIDEO_DIR.get_or_init(resolve_video_wallpaper_dir).clone()
}

#[cfg(target_os = "linux")]
fn resolve_video_wallpaper_dir() -> PathBuf {
    let mut command = Command::new("xdg-user-dir");
    command.arg("VIDEOS");
    let xdg = crate::util::output_with_timeout(&mut command, Duration::from_secs(2))
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|dir| !dir.is_empty());
    match xdg {
        Some(dir) => Path::new(&dir).join("Ferramari"),
        None => home().join("Videos").join("Ferramari"),
    }
}

#[cfg(target_os = "macos")]
fn resolve_video_wallpaper_dir() -> PathBuf {
    home().join("Movies").join("Ferramari")
}

#[cfg(target_os = "windows")]
fn resolve_video_wallpaper_dir() -> PathBuf {
    home().join("Videos").join("Ferramari")
}

pub fn is_flatpak() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/.flatpak-info").is_file()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn is_wayland() -> bool {
    #[cfg(target_os = "linux")]
    {
        env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// GNOME and Ubuntu report different values (`GNOME`, `ubuntu:GNOME`), so the
/// check is a case-insensitive substring match.
pub fn is_gnome() -> bool {
    #[cfg(target_os = "linux")]
    {
        env::var("XDG_CURRENT_DESKTOP")
            .map(|v| v.to_lowercase().contains("gnome"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Locate a helper program (mpv, yt-dlp). A copy next to the `ferramari`
/// binary wins, so a per-user install works even when the desktop session's
/// PATH does not contain `~/.local/bin`.
pub fn tool(name: &str) -> std::path::PathBuf {
    match std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        Some(dir) => tool_in(&dir, name),
        None => std::path::PathBuf::from(name),
    }
}

fn tool_in(dir: &Path, name: &str) -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    let candidate = dir.join(format!("{name}.exe"));
    #[cfg(not(target_os = "windows"))]
    let candidate = dir.join(name);
    if candidate.is_file() {
        candidate
    } else {
        std::path::PathBuf::from(name)
    }
}

/// Create directories the program writes to. Idempotent.
pub fn ensure_dirs() {
    for dir in [
        config_dir(),
        video_wallpaper_dir(),
        thumbnail_cache_dir(),
        static_cache_dir(),
        catalogue_cache_dir(),
        packages_dir(),
        plugins_dir(),
        mpv_socket_dir(),
        autostart_dir(),
    ] {
        if let Err(err) = fs::create_dir_all(&dir) {
            log::warn!("[Paths] Could not create {}: {err}", dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_paths_are_absolute_and_nested() {
        let path = config_path();
        assert!(path.is_absolute());
        assert!(path.ends_with("ferramari/config.json"));
    }

    #[test]
    fn tool_prefers_a_sibling_binary() {
        let dir = std::env::temp_dir().join(format!("ferramari-tool-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mpv"), b"#!/bin/sh\n").unwrap();
        assert_eq!(tool_in(&dir, "mpv"), dir.join("mpv"));
        assert_eq!(tool_in(&dir, "yt-dlp"), std::path::PathBuf::from("yt-dlp"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn video_dir_ends_with_ferramari() {
        assert!(video_wallpaper_dir().ends_with("Ferramari"));
    }
}
