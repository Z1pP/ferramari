//! Cross-platform user-data locations and helper executable discovery.
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
    config_base().join("ferramari")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Favourites, history and collections. Kept separate from `config.json`.
pub fn library_path() -> PathBuf {
    config_dir().join("library.json")
}

/// Remembered display identities: which physical screen owns which config key.
/// Separate from `config.json` because it is internal bookkeeping that must
/// survive a config reset.
pub fn display_ids_path() -> PathBuf {
    config_dir().join("display-ids.json")
}

#[cfg(target_os = "windows")]
pub fn autostart_dir() -> PathBuf {
    // Windows uses the per-user Run key, not a desktop-autostart folder. The
    // Startup folder is still the human-facing location, so diagnostics and
    // documentation point there rather than at a directory we never write.
    config_base()
        .join("Microsoft/Windows/Start Menu/Programs/Startup")
        .join("Ferramari")
}

#[cfg(not(target_os = "windows"))]
pub fn autostart_dir() -> PathBuf {
    config_base().join("autostart")
}

#[cfg(target_os = "windows")]
pub fn autostart_path() -> PathBuf {
    autostart_dir().join("Ferramari.lnk")
}

#[cfg(not(target_os = "windows"))]
pub fn autostart_path() -> PathBuf {
    autostart_dir().join(format!("{APP_ID}.desktop"))
}

pub fn thumbnail_cache_dir() -> PathBuf {
    cache_base().join("ferramari").join("thumbnails")
}

/// Cached blurred frames used for the GNOME static wallpaper.
pub fn static_cache_dir() -> PathBuf {
    cache_base().join("ferramari").join("static")
}

/// Installed `.ferramari` packages.
pub fn packages_dir() -> PathBuf {
    data_base().join("ferramari").join("packages")
}

/// Declarative plugin manifests.
pub fn plugins_dir() -> PathBuf {
    data_base().join("ferramari").join("plugins")
}

/// Cache for the online wallpaper catalogue (pages, previews, downloads).
pub fn catalogue_cache_dir() -> PathBuf {
    cache_base().join("ferramari").join("catalogue")
}

/// mpv IPC sockets. Kept in the cache rather than `/tmp` so a host mpv
/// started through `flatpak-spawn` can reach the same path.
pub fn mpv_socket_dir() -> PathBuf {
    cache_base().join("ferramari").join("mpv")
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

/// Directory containing `<language>/LC_MESSAGES/ferramari.mo`.
///
/// `FERRAMARI_LOCALEDIR` wins; otherwise look next to the executable so a
/// non-default `PREFIX` (`/usr/local`, a staged install, Guix, Nix, the
/// Windows installer) finds its own catalogs. A system install keeps them one
/// level up in `share/locale`; Unix falls back to `/usr/share/locale`.
pub fn locale_dir() -> PathBuf {
    if let Some(dir) = env::var_os("FERRAMARI_LOCALEDIR") {
        return dir.into();
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(bin) = exe.parent() {
            let local = bin.join("locale");
            if local.is_dir() {
                return local;
            }
            if let Some(prefix) = bin.parent() {
                let candidate = prefix.join("share/locale");
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }
    }
    #[cfg(unix)]
    {
        PathBuf::from("/usr/share/locale")
    }
    #[cfg(not(unix))]
    {
        PathBuf::new()
    }
}

/// Locate a helper program (mpv, yt-dlp). A copy next to the `ferramari`
/// binary wins, so a per-user install works even when the desktop session's
/// PATH does not contain `~/.local/bin`. On Windows the standard installer
/// locations are checked next: mpv's Inno installer puts the binary under
/// `Program Files\MPV Player` without touching PATH, and winget keeps its
/// shims under `LOCALAPPDATA\Microsoft\WinGet\Links`.
pub fn tool(name: &str) -> std::path::PathBuf {
    if let Some(sibling) = sibling_tool(name) {
        return sibling;
    }
    #[cfg(target_os = "windows")]
    {
        let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let program_files = env::var_os("ProgramFiles").map(PathBuf::from);
        for candidate in
            windows_tool_candidates(name, local_app_data.as_deref(), program_files.as_deref())
        {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from(name)
}

fn sibling_tool(name: &str) -> Option<PathBuf> {
    let directory = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))?;
    let candidate = tool_in(&directory, name);
    (candidate != Path::new(name)).then_some(candidate)
}

/// Standard per-user and machine-wide Windows install locations.
#[cfg(target_os = "windows")]
fn windows_tool_candidates(
    name: &str,
    local_app_data: Option<&Path>,
    program_files: Option<&Path>,
) -> Vec<PathBuf> {
    let exe = format!("{name}.exe");
    let mut candidates = Vec::new();
    if let Some(local) = local_app_data {
        candidates.push(
            local
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join(&exe),
        );
        candidates.push(local.join("Programs").join(name).join(&exe));
    }
    if let Some(program_files) = program_files {
        candidates.push(program_files.join(name).join(&exe));
        if name == "mpv" {
            // shinchiro's Inno installer (the winget `shinchiro.mpv` package).
            candidates.push(program_files.join("MPV Player").join(&exe));
        }
    }
    candidates
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
        #[cfg(target_os = "windows")]
        let sibling = dir.join("mpv.exe");
        #[cfg(not(target_os = "windows"))]
        let sibling = dir.join("mpv");
        std::fs::write(&sibling, b"tool").unwrap();
        assert_eq!(tool_in(&dir, "mpv"), sibling);
        assert_eq!(tool_in(&dir, "yt-dlp"), std::path::PathBuf::from("yt-dlp"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn video_dir_ends_with_ferramari() {
        assert!(video_wallpaper_dir().ends_with("Ferramari"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_tool_candidates_cover_installer_locations() {
        let local = Path::new(r"C:\Users\Test\AppData\Local");
        let program_files = Path::new(r"C:\Program Files");

        let mpv = windows_tool_candidates("mpv", Some(local), Some(program_files));
        assert!(mpv.contains(
            &local
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("mpv.exe")
        ));
        assert!(mpv.contains(&program_files.join("MPV Player").join("mpv.exe")));
        assert!(mpv.contains(&program_files.join("mpv").join("mpv.exe")));

        let ffmpeg = windows_tool_candidates("ffmpeg", Some(local), Some(program_files));
        assert!(ffmpeg.contains(
            &local
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("ffmpeg.exe")
        ));
        assert!(!ffmpeg
            .iter()
            .any(|path| path.ends_with("MPV Player/ffmpeg.exe")));
    }
}
