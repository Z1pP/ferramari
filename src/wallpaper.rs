//! GNOME static wallpaper support: extract a frame from the video, blur it,
//! install it as the desktop background, and put the user's original
//! background back on exit.
//!
//! Non-GNOME desktops are left untouched, matching upstream.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use gtk::gio;
use gtk::prelude::*;

use crate::paths;
use crate::util;

const BACKGROUND_SCHEMA: &str = "org.gnome.desktop.background";

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

    /// Extract a frame at the golden-ratio point of the video, blur it and set
    /// it as the background. Returns the generated file on success.
    pub fn apply_static(&mut self, video: &Path, blur_radius: u32) -> Option<PathBuf> {
        if !self.is_supported() {
            return None;
        }
        let duration = probe_duration(video).unwrap_or(0.0);
        let seek = util::format_hms(duration / std::f64::consts::PI);
        let file = paths::config_dir().join(format!(
            "static-{:06}.png",
            rand::random::<u32>() % 1_000_000
        ));

        let mut command = Command::new("ffmpeg");
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
        let status = command.arg(&file).status();
        if !matches!(status, Ok(status) if status.success()) || !file.is_file() {
            log::warn!(
                "[StaticWallpaper] Could not extract a frame from {}",
                video.display()
            );
            let _ = fs::remove_file(&file);
            return None;
        }

        let uri = util::path_to_uri(&file);
        self.set_uri("picture-uri", &uri);
        self.set_uri("picture-uri-dark", &uri);
        self.cleanup_files_except(Some(&file));
        self.active_file = Some(file.clone());
        log::info!("[StaticWallpaper] Applied {uri}");
        Some(file)
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
            let _ = Command::new("flatpak-spawn")
                .args(["--host", "gsettings", "set", BACKGROUND_SCHEMA, key, value])
                .status();
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
    let output = Command::new("flatpak-spawn")
        .args(["--host", "gsettings", "get", BACKGROUND_SCHEMA, key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn probe_duration(video: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(video)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}
