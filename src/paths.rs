//! File-system locations, environment detection and process-wide environment
//! preparation.
//!
//! All knowledge about *where things live* is kept here so the rest of the
//! program never builds paths by hand.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const APP_ID: &str = "io.github.ferramari.Ferramari";
pub const SERVER_NAME: &str = "io.github.ferramari.Ferramari.server";
pub const TRANSLATION_DOMAIN: &str = "ferramari";

/// Force the X11 backend (through XWayland on Wayland sessions).
///
/// Toplevel placement, the DESKTOP window type and `--wid` embedding of mpv
/// all require X11. This must run before GTK is initialised.
pub fn prepare_env() {
    // Always force X11 (XWayland on Wayland): desktop-type windows, toplevel
    // placement and mpv's `--wid` embedding are X11-only.
    env::set_var("GDK_BACKEND", "x11");
    prepare_webkit_env();
}

/// WebKitGTK's DMA-BUF renderer cannot allocate GBM buffers on the NVIDIA
/// driver. If an NVIDIA GPU is present, steer WebKit to another render node,
/// or fall back to shared-memory buffers on NVIDIA-only systems.
fn prepare_webkit_env() {
    const OVERRIDES: [&str; 3] = [
        "WEBKIT_WEB_RENDER_DEVICE_FILE",
        "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
        "WEBKIT_DISABLE_DMABUF_RENDERER",
    ];
    if OVERRIDES.iter().any(|var| env::var_os(var).is_some()) {
        return;
    }
    let mut nodes = Vec::new();
    let Ok(entries) = fs::read_dir("/dev/dri") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("renderD") {
            continue;
        }
        let driver = fs::canonicalize(format!("/sys/class/drm/{name}/device/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        nodes.push((entry.path(), driver));
    }
    if !nodes.iter().any(|(_, driver)| driver == "nvidia") {
        return;
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    for (node, driver) in &nodes {
        if driver != "nvidia" {
            log::info!(
                "[WebPlayer] Using render device {} ({driver})",
                node.display()
            );
            env::set_var("WEBKIT_WEB_RENDER_DEVICE_FILE", node);
            return;
        }
    }
    log::info!("[WebPlayer] NVIDIA-only system, using SHM buffers for WebKit");
    env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1");
}

pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg_config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

fn xdg_cache_home() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
}

pub fn config_dir() -> PathBuf {
    xdg_config_home().join("ferramari")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn autostart_dir() -> PathBuf {
    xdg_config_home().join("autostart")
}

pub fn autostart_path() -> PathBuf {
    autostart_dir().join(format!("{APP_ID}.desktop"))
}

pub fn thumbnail_cache_dir() -> PathBuf {
    xdg_cache_home().join("ferramari").join("thumbnails")
}

/// `$XDG_VIDEOS_DIR/Ferramari`, falling back to `$HOME/Ferramari` when
/// `xdg-user-dir` is unavailable.
pub fn video_wallpaper_dir() -> PathBuf {
    let xdg = Command::new("xdg-user-dir")
        .arg("VIDEOS")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|dir| !dir.is_empty());
    match xdg {
        Some(dir) => Path::new(&dir).join("Ferramari"),
        None => home().join("Ferramari"),
    }
}

pub fn is_flatpak() -> bool {
    Path::new("/.flatpak-info").is_file()
}

pub fn is_wayland() -> bool {
    env::var("XDG_SESSION_TYPE")
        .map(|v| v == "wayland")
        .unwrap_or(false)
}

/// GNOME and Ubuntu report different values (`GNOME`, `ubuntu:GNOME`), so the
/// check is a case-insensitive substring match, like upstream.
pub fn is_gnome() -> bool {
    env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_lowercase().contains("gnome"))
        .unwrap_or(false)
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
