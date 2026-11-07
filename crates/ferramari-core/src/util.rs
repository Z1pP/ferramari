//! Small utilities that do not own state: video discovery, GNOME Shell
//! extension workarounds and a few string helpers.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use url::Url;

use crate::config::{Config, Mode};
use crate::monitor::MonitorInfo;
use crate::paths;

/// Poll a child until it exits or the deadline passes. On timeout the child is
/// killed and reaped so a wedged decoder cannot leak a process or block the
/// caller forever.
pub fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Run a command with captured output and a hard deadline.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> Option<Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let status = wait_with_timeout(&mut child, timeout)?;
    let mut stdout = Vec::new();
    child.stdout.take()?.read_to_end(&mut stdout).ok()?;
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr);
    }
    Some(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run a command with discarded output and a hard deadline.
pub fn run_with_timeout(command: &mut Command, timeout: Duration) -> bool {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    wait_with_timeout(&mut child, timeout).is_some_and(|status| status.success())
}

/// Translate a user-visible string through the `ferramari` gettext domain.
pub fn tr(text: &str) -> String {
    gettextrs::gettext(text)
}

/// Translate a counted noun. The catalog supplies the plural forms.
pub fn tr_n(singular: &str, plural: &str, n: u32) -> String {
    gettextrs::ngettext(singular, plural, n)
}

/// The source shown as "now playing": the primary monitor's own video in video
/// mode, otherwise the shared stream/web URL.
pub fn primary_source<'a>(config: &'a Config, monitors: &[MonitorInfo]) -> &'a str {
    if config.mode == Mode::Video {
        monitors
            .iter()
            .find(|monitor| monitor.is_primary)
            .or_else(|| monitors.first())
            .map(|monitor| config.source_for(&monitor.name))
            .unwrap_or("")
    } else {
        config.default_source()
    }
}

/// All regular files in the wallpaper folder whose content type is `video/*`.
/// Sorted for a stable GUI order.
pub fn video_paths() -> Vec<PathBuf> {
    let mut videos = video_paths_in(&paths::video_wallpaper_dir());
    videos.extend(crate::package::installed_videos());
    videos.sort();
    videos.dedup();
    videos
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

/// Whether the file at `path` looks like a video. The previous implementation
/// asked GIO for `standard::content-type`, which sniffs magic bytes; the
/// toolkit-free replacement does the same for the containers the library
/// actually encounters (ISO-BMFF, EBML, Ogg) and falls back to the file
/// extension, which is what users expect from a video file anyway.
pub fn is_video(path: &Path) -> bool {
    let magic = head_bytes(path, 12);
    if let Some(magic) = magic.as_deref() {
        if matches_container(magic) {
            return true;
        }
    }
    matches_extension(path)
}

fn head_bytes(path: &Path, count: usize) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0u8; count];
    let read = std::io::Read::read(&mut file, &mut buffer).ok()?;
    buffer.truncate(read);
    Some(buffer)
}

fn matches_container(magic: &[u8]) -> bool {
    // ISO-BMFF (mp4/mov/m4v/webm-vp9 container): 4cc at offset 4 is `ftyp`.
    magic.len() >= 12 && &magic[4..8] == b"ftyp"
    // EBML (matroska/webm): 0x1A45DFA3.
    || magic.len() >= 4 && magic[0..4] == [0x1a, 0x45, 0xdf, 0xa3]
    // Ogg.
    || magic.len() >= 4 && &magic[0..4] == b"OggS"
}

fn matches_extension(path: &Path) -> bool {
    const VIDEO_EXTENSIONS: [&str; 14] = [
        "mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "m4a", "ogv", "ts", "m2ts",
        "flv", "wmv",
    ];
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Convert a path to a `file://` URI with percent-escaping.
pub fn path_to_uri(path: &Path) -> String {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

/// The path a `file://` URI refers to, or `None` for anything that is not a
/// local file. Percent escapes are decoded; a URI naming a remote host is
/// refused rather than silently read as a local path. Plain paths pass through
/// unchanged so D-Bus callers may send either form.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    if let Some(rest) = uri.strip_prefix("file://") {
        let host = rest.split('/').next().unwrap_or_default();
        if !host.is_empty() && host != "localhost" {
            return None;
        }
        return Url::parse(uri)
            .ok()
            .and_then(|url| url.to_file_path().ok());
    }
    // Anything else with a scheme (`https://`, `data:`) is not a local file.
    let local = !uri.is_empty() && !uri.contains("://") && !uri.starts_with("data:");
    local.then(|| PathBuf::from(uri))
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

/// True for URIs that do not leave this machine. `data:`, `about:` and
/// `blob:` are generated by WebKit itself; a `file:` URI is local only when
/// its authority is empty or `localhost`.
pub fn is_local_uri(uri: &str) -> bool {
    if let Some(rest) = uri.strip_prefix("file://") {
        let host = rest.split('/').next().unwrap_or_default();
        return host.is_empty() || host == "localhost";
    }
    uri.starts_with("data:") || uri.starts_with("about:") || uri.starts_with("blob:")
}

/// Remove abandoned ffmpeg output files (`*.part.jpg`, `*.part.png`) left by
/// a crashed process. Fresh partials are kept: another worker may still be
/// writing them.
pub fn cleanup_partial_files(dir: &Path, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().contains(".part.") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= older_than);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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
    fn uri_to_path_decodes_escapes_and_refuses_remote_hosts() {
        assert_eq!(
            uri_to_path("/tmp/a.html"),
            Some(PathBuf::from("/tmp/a.html"))
        );
        assert_eq!(
            uri_to_path("file:///tmp/a%20b.html"),
            Some(PathBuf::from("/tmp/a b.html"))
        );
        assert_eq!(uri_to_path("file://evil.example/a.html"), None);
        assert_eq!(uri_to_path("https://example.test"), None);
        assert_eq!(uri_to_path("data:text/html,x"), None);
        assert_eq!(uri_to_path(""), None);
        // Whatever `path_to_uri` writes, `uri_to_path` reads back.
        let round = uri_to_path(&path_to_uri(Path::new("/tmp/one two.mkv")));
        assert_eq!(round, Some(PathBuf::from("/tmp/one two.mkv")));
    }

    #[test]
    fn hms_formats_and_truncates() {
        assert_eq!(format_hms(0.0), "00:00:00");
        assert_eq!(format_hms(59.9), "00:00:59");
        assert_eq!(format_hms(61.0), "00:01:01");
        assert_eq!(format_hms(3661.0), "01:01:01");
        assert_eq!(format_hms(-5.0), "00:00:00");
    }

    #[test]
    fn primary_source_uses_the_primary_monitor_only_in_video_mode() {
        let names = vec!["eDP-1".to_string(), "HDMI-1".to_string()];
        let mut config = Config::template(&names);
        config.mode = Mode::Video;
        config.data_source.insert("eDP-1".into(), "/a.mp4".into());
        config.data_source.insert("HDMI-1".into(), "/b.mp4".into());
        let monitors = vec![
            MonitorInfo {
                index: 0,
                name: "eDP-1".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale_factor: 1,
                is_primary: true,
            },
            MonitorInfo {
                index: 1,
                name: "HDMI-1".into(),
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080,
                scale_factor: 1,
                is_primary: false,
            },
        ];
        assert_eq!(primary_source(&config, &monitors), "/a.mp4");

        config.mode = Mode::Webpage;
        config
            .data_source
            .insert("Default".into(), "https://example.com/".into());
        assert_eq!(primary_source(&config, &monitors), "https://example.com/");
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
    fn abandoned_partial_files_are_removed_but_fresh_ones_are_kept() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-partials-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.part.jpg");
        let fresh = dir.join("fresh.part.png");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&fresh, b"x").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        let old_time = std::time::SystemTime::now() - Duration::from_secs(7200);
        file.set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        cleanup_partial_files(&dir, Duration::from_secs(3600));
        assert!(!old.exists());
        assert!(fresh.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn local_uris_are_recognised_and_remote_file_hosts_are_not() {
        assert!(is_local_uri("file:///tmp/a.html"));
        assert!(is_local_uri("file://localhost/tmp/a.html"));
        assert!(is_local_uri("data:text/html,hi"));
        assert!(is_local_uri("about:blank"));
        assert!(!is_local_uri("file://example.com/tmp/a.html"));
        assert!(!is_local_uri("https://example.com/"));
        assert!(!is_local_uri("http://127.0.0.1:8080/"));
    }

    #[test]
    fn random_other_video_returns_none_when_only_current_exists() {
        let videos = vec![PathBuf::from("/a.mp4")];
        assert!(random_other_video(&videos, "/a.mp4").is_none());
    }
}
