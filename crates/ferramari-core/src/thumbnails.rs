//! Video thumbnails for the control window.
//!
//! Rendering happens on a worker thread; the resulting file is loaded into an
//! `iced::image::Handle`. Frames are cached per (path, mtime).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use crate::paths;
use crate::util;

/// A cached JPEG preview of `video`. `size` picks the frame geometry so the
/// library grid and the Home hero can share one cache directory while keeping
/// their own resolutions. Returns `None` when no thumbnail can be produced.
pub fn generate(video: &Path) -> Option<PathBuf> {
    generate_at(video, "card-400x225")
}

/// The wide frame used by the Home hero card. 960×540 stays sharp when the
/// card is stretched across a 1180 px window on a HiDPI display.
pub fn generate_hero(video: &Path) -> Option<PathBuf> {
    generate_at(video, "hero-960x540")
}

fn generate_at(video: &Path, size: &str) -> Option<PathBuf> {
    let (width, height) = match size {
        "hero-960x540" => (960, 540),
        _ => (400, 225),
    };
    let metadata = std::fs::metadata(video).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    video.hash(&mut hasher);
    modified.hash(&mut hasher);
    size.hash(&mut hasher);
    let cache_dir = paths::thumbnail_cache_dir();
    std::fs::create_dir_all(&cache_dir).ok()?;
    let target = cache_dir.join(format!("{:016x}.jpg", hasher.finish()));
    if target.is_file() {
        return Some(target);
    }

    let partial = cache_dir.join(format!(
        "{}.part.jpg",
        target.file_stem()?.to_string_lossy()
    ));
    let ok = run_ffmpeg(video, &partial, true, width, height)
        || run_ffmpeg(video, &partial, false, width, height);
    if !ok || !partial.is_file() {
        let _ = std::fs::remove_file(&partial);
        return None;
    }
    if std::fs::rename(&partial, &target).is_err() {
        let _ = std::fs::remove_file(&partial);
        return None;
    }
    Some(target)
}

fn run_ffmpeg(video: &Path, output: &Path, seek: bool, width: u32, height: u32) -> bool {
    let mut command = Command::new(paths::tool("ffmpeg"));
    command.arg("-y").arg("-v").arg("error");
    if seek {
        // Input seek avoids decoding the first second of the file. The
        // `thumbnail` filter used to scan frames and made this 2.4x slower.
        command.args(["-ss", "1"]);
    }
    command.arg("-i").arg(video);
    let filter = format!(
        "scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height}"
    );
    command
        .args(["-vf", &filter, "-frames:v", "1"])
        .arg(output);
    util::run_with_timeout(&mut command, Duration::from_secs(20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_has_no_thumbnail() {
        assert!(generate(Path::new("/nonexistent/video.mp4")).is_none());
        assert!(generate_hero(Path::new("/nonexistent/video.mp4")).is_none());
    }

    #[test]
    fn the_two_sizes_do_not_share_a_cache_key() {
        // The hash includes the size label, so a 400×225 card can never be
        // served where a 960×540 hero was asked for.
        let key = |size: &str| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            "video.mp4".hash(&mut hasher);
            0u64.hash(&mut hasher);
            size.hash(&mut hasher);
            hasher.finish()
        };
        assert_ne!(key("card-400x225"), key("hero-960x540"));
    }
}
