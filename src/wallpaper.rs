//! GNOME static wallpaper support: extract a frame from the video, blur it,
//! install it as the desktop background, and put the user's original
//! background back on exit.
//!
//! Non-GNOME desktops are left untouched.

use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use gtk::gio;
use gtk::prelude::*;

use crate::paths;
use crate::util;

const BACKGROUND_SCHEMA: &str = "org.gnome.desktop.background";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_TIMEOUT: Duration = Duration::from_secs(60);

const STATIC_CACHE_LIMIT: usize = 12;

/// Extract a frame at the golden-ratio point of the video, blur it and return
/// the generated file. This is CPU/GPU work and must run off the GTK thread.
/// Results are cached by `(path, mtime, blur)` so restarts and source switches
/// do not decode the video again.
pub fn render_static_frame(video: &Path, blur_radius: u32) -> Option<PathBuf> {
    let metadata = fs::metadata(video).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_secs())
        .unwrap_or(0);
    let cache_dir = paths::static_cache_dir();
    fs::create_dir_all(&cache_dir).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    video.hash(&mut hasher);
    modified.hash(&mut hasher);
    blur_radius.hash(&mut hasher);
    "static-v1".hash(&mut hasher);
    let file = cache_dir.join(format!("{:016x}.png", hasher.finish()));
    if file.is_file() {
        return Some(file);
    }

    let duration = probe_duration(video).unwrap_or(0.0);
    let seek = util::format_hms(duration / std::f64::consts::PI);
    let partial = cache_dir.join(format!(
        "{:016x}-{:06}.part.png",
        hasher.finish(),
        rand::random::<u32>() % 1_000_000
    ));

    let mut command = Command::new(paths::tool("ffmpeg"));
    command
        .arg("-y")
        .arg("-v")
        .arg("error")
        .arg("-ss")
        .arg(&seek)
        .arg("-i")
        .arg(video)
        .arg("-frames:v")
        .arg("1");
    if blur_radius > 0 {
        command.arg("-vf").arg(format!("gblur=sigma={blur_radius}"));
    }
    let Ok(mut child) = command
        .arg(&partial)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return None;
    };
    let rendered =
        util::wait_with_timeout(&mut child, FRAME_TIMEOUT).is_some_and(|status| status.success());
    if !rendered || !partial.is_file() {
        log::warn!(
            "[StaticWallpaper] Could not extract a frame from {}",
            video.display()
        );
        let _ = fs::remove_file(&partial);
        return None;
    }
    if fs::rename(&partial, &file).is_err() {
        let _ = fs::remove_file(&partial);
        return None;
    }
    prune_static_cache(&cache_dir, &file);
    Some(file)
}

/// Keep the cache bounded; one PNG per video can be several megabytes.
fn prune_static_cache(dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "png")
                || path == keep
                || path.to_string_lossy().ends_with(".part.png")
            {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    if files.len() < STATIC_CACHE_LIMIT {
        return;
    }
    files.sort_by_key(|(modified, _)| *modified);
    let remove = files.len() + 1 - STATIC_CACHE_LIMIT;
    for (_, path) in files.into_iter().take(remove) {
        let _ = fs::remove_file(path);
    }
}

pub struct GnomeBackground {
    settings: Option<gio::Settings>,
    original: Option<(String, String)>,
    active_file: Option<PathBuf>,
}

impl GnomeBackground {
    pub fn capture() -> Self {
        if !paths::is_gnome() {
            return Self {
                settings: None,
                original: None,
                active_file: None,
            };
        }
        if paths::is_flatpak() {
            let light = host_gsettings_get("picture-uri");
            let dark = host_gsettings_get("picture-uri-dark");
            return Self {
                settings: None,
                original: light.zip(dark),
                active_file: None,
            };
        }
        let settings = schema_available().then(|| gio::Settings::new(BACKGROUND_SCHEMA));
        let original = settings.as_ref().map(|settings| {
            (
                settings.string("picture-uri").to_string(),
                settings.string("picture-uri-dark").to_string(),
            )
        });
        Self {
            settings,
            original,
            active_file: None,
        }
    }

    pub fn is_supported(&self) -> bool {
        self.settings.is_some() || (paths::is_flatpak() && self.original.is_some())
    }

    /// Install an already rendered frame as the background. Rendering itself
    /// happens on a worker thread so ffmpeg never stalls the GTK loop.
    pub fn apply_static_frame(&mut self, file: &Path) -> Option<PathBuf> {
        if !self.is_supported() {
            return None;
        }
        let uri = util::path_to_uri(file);
        self.set_uri("picture-uri", &uri);
        self.set_uri("picture-uri-dark", &uri);
        self.cleanup_files_except(Some(file));
        self.active_file = Some(file.to_path_buf());
        log::info!("[StaticWallpaper] Applied {uri}");
        Some(file.to_path_buf())
    }

    /// Restore the background captured at startup and drop generated frames.
    pub fn restore(&mut self) {
        // Deliberately keep `original`: the same process may apply a static
        // wallpaper again later (mode switch, blur change) and must still know
        // what to put back.
        if let Some((light, dark)) = &self.original {
            self.set_uri("picture-uri", light);
            self.set_uri("picture-uri-dark", dark);
        }
        self.active_file = None;
        self.cleanup_files_except(None);
    }

    fn set_uri(&self, key: &str, value: &str) {
        if let Some(settings) = &self.settings {
            if settings.string(key).as_str() != value {
                let _ = settings.set_string(key, value);
            }
        } else if paths::is_flatpak() {
            let mut command = Command::new("flatpak-spawn");
            command.args(["--host", "gsettings", "set", BACKGROUND_SCHEMA, key, value]);
            let _ = util::run_with_timeout(&mut command, Duration::from_secs(5));
        }
    }

    fn cleanup_files_except(&self, keep: Option<&Path>) {
        let dir = paths::config_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("static-") && name.ends_with(".png") && keep != Some(path.as_path())
            {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn schema_available() -> bool {
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(BACKGROUND_SCHEMA, true))
        .is_some()
}

fn host_gsettings_get(key: &str) -> Option<String> {
    let mut command = Command::new("flatpak-spawn");
    command.args(["--host", "gsettings", "get", BACKGROUND_SCHEMA, key]);
    let output = util::output_with_timeout(&mut command, Duration::from_secs(5))?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn probe_duration(video: &Path) -> Option<f64> {
    let mut child = Command::new(paths::tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(video)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = util::wait_with_timeout(&mut child, PROBE_TIMEOUT)?;
    if !status.success() {
        return None;
    }
    let mut stdout = Vec::new();
    child.stdout.take()?.read_to_end(&mut stdout).ok()?;
    String::from_utf8_lossy(&stdout).trim().parse().ok()
}
