//! Small utilities that do not own state: video discovery, GNOME Shell
//! extension workarounds and a few string helpers.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk::gio;
use gtk::prelude::*;

use crate::paths;

/// Translate a user-visible string through the `ferramari` gettext domain.
pub fn tr(text: &str) -> String {
    gettextrs::gettext(text)
}

/// All regular files in the wallpaper folder whose content type is `video/*`.
/// Sorted for a stable GUI order, like upstream.
pub fn video_paths() -> Vec<PathBuf> {
    video_paths_in(&paths::video_wallpaper_dir())
}

pub fn video_paths_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut videos: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_video(path))
        .collect();
    videos.sort();
    videos
}

pub fn is_video(path: &Path) -> bool {
    let file = gio::File::for_path(path);
    let info = file.query_info(
        "standard::content-type",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    );
    match info {
        Ok(info) => info
            .content_type()
            .map(|content_type| content_type.starts_with("video/"))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Upload a path to a `file://` URI using GIO (handles escaping for us).
pub fn path_to_uri(path: &Path) -> String {
    gio::File::for_path(path).uri().to_string()
}

/// GNOME Shell reports extension state as an integer; 1 means enabled.
pub fn gnome_extension_state(name: &str) -> Option<i32> {
    use zbus::blocking::{Connection, Proxy};
    let connection = Connection::session().ok()?;
    let proxy = Proxy::new(
        &connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    )
    .ok()?;
    let info: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
        proxy.call("GetExtensionInfo", &(name,)).ok()?;
    let state = info.get("state")?;
    owned_value_to_i32(state)
}

/// `org.gnome.Shell.Extensions` reports `state` as an integer in older
/// releases but as a double (`1.0`) in GNOME 50. Accept both, plus the other
/// integer flavours, instead of silently failing the workaround.
fn owned_value_to_i32(value: &zbus::zvariant::OwnedValue) -> Option<i32> {
    if let Ok(value) = i32::try_from(value) {
        return Some(value);
    }
    if let Ok(value) = u32::try_from(value) {
        return Some(value as i32);
    }
    if let Ok(value) = i64::try_from(value) {
        return Some(value as i32);
    }
    if let Ok(value) = u64::try_from(value) {
        return Some(value as i32);
    }
    if let Ok(value) = f64::try_from(value) {
        return Some(value as i32);
    }
    None
}

pub fn gnome_extension_installed(name: &str) -> bool {
    use zbus::blocking::{Connection, Proxy};
    let Ok(connection) = Connection::session() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(
        &connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    ) else {
        return false;
    };
    let installed: std::collections::HashMap<
        String,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    > = match proxy.call("ListExtensions", &()) {
        Ok(value) => value,
        Err(_) => return false,
    };
    installed.contains_key(name)
}

fn gnome_extension_call(method: &str, name: &str) {
    use zbus::blocking::{Connection, Proxy};
    let Ok(connection) = Connection::session() else {
        return;
    };
    let Ok(proxy) = Proxy::new(
        &connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    ) else {
        return;
    };
    if let Err(err) = proxy.call::<_, _, bool>(method, &(name,)) {
        log::debug!("[Utils] {method}({name}) failed: {err}");
    }
}

/// Desktop-icon extensions keep their icons above the wallpaper window.
/// Restarting them after the wallpaper is shown puts the icons back on top.
///
/// Runs on a worker thread: disabling and re-enabling an extension takes a
/// moment, and GNOME 50 leaves the extension `INACTIVE` if `EnableExtension`
/// arrives immediately after `DisableExtension`.
pub fn gnome_desktop_icon_workaround() {
    if !paths::is_gnome() {
        return;
    }
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        for extension in [
            "ding@rastersoft.com",
            "desktopicons-neo@darkdemon",
            "gtk4-ding@smedius.gitlab.com",
            "zorin-desktop-icons@zorinos.com",
        ] {
            let installed = gnome_extension_installed(extension);
            let state = gnome_extension_state(extension);
            log::debug!("[Utils] {extension}: installed={installed} state={state:?}");
            if installed && state == Some(1) {
                log::info!("[Utils] Apply workaround for {extension}");
                gnome_extension_call("DisableExtension", extension);
                std::thread::sleep(Duration::from_millis(700));
                gnome_extension_call("EnableExtension", extension);
                std::thread::sleep(Duration::from_millis(700));
                log::debug!(
                    "[Utils] {extension} state after workaround: {:?}",
                    gnome_extension_state(extension)
                );
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Format seconds as `HH:MM:SS`, used for the static-wallpaper frame seek.
pub fn format_hms(total_seconds: f64) -> String {
    let total = total_seconds.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

/// Keep a random choice from picking the video that is already playing.
pub fn random_other_video(videos: &[PathBuf], current: &str) -> Option<PathBuf> {
    use rand::seq::SliceRandom;
    let candidates: Vec<&PathBuf> = videos
        .iter()
        .filter(|path| path.to_string_lossy() != current)
        .collect();
    candidates
        .choose(&mut rand::thread_rng())
        .map(|p| (*p).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_formats_and_truncates() {
        assert_eq!(format_hms(0.0), "00:00:00");
        assert_eq!(format_hms(59.9), "00:00:59");
        assert_eq!(format_hms(61.0), "00:01:01");
        assert_eq!(format_hms(3661.0), "01:01:01");
        assert_eq!(format_hms(-5.0), "00:00:00");
    }

    #[test]
    fn random_other_video_skips_current() {
        let videos = vec![PathBuf::from("/a.mp4"), PathBuf::from("/b.mp4")];
        for _ in 0..20 {
            let choice = random_other_video(&videos, "/a.mp4").unwrap();
            assert_eq!(choice, PathBuf::from("/b.mp4"));
        }
        assert!(random_other_video(&videos, "/a.mp4").is_some());
    }

    #[test]
    fn random_other_video_returns_none_when_only_current_exists() {
        let videos = vec![PathBuf::from("/a.mp4")];
        assert!(random_other_video(&videos, "/a.mp4").is_none());
    }
}
