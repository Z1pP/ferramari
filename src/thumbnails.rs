//! Video thumbnails for the control window.
//!
//! Rendering happens on a worker thread; the resulting file is loaded into a
//! `gdk_pixbuf::Pixbuf` on the GTK thread. Frames are cached per (path, mtime).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use crate::paths;
use crate::util;

/// Return a cached 16:9 JPEG preview for `video`, generating it with ffmpeg
/// when no cache entry exists. Returns `None` when no thumbnail can be
/// produced. The 400×225 size covers the card grid at HiDPI.
pub fn generate(video: &Path) -> Option<PathBuf> {
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
    "card-400x225".hash(&mut hasher);
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
    let ok = run_ffmpeg(video, &partial, true) || run_ffmpeg(video, &partial, false);
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

fn run_ffmpeg(video: &Path, output: &Path, seek: bool) -> bool {
    let mut command = Command::new(paths::tool("ffmpeg"));
    command
        .arg("-y")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(video);
    if seek {
        command.args(["-ss", "1"]);
    }
    command
        .args([
            "-vf",
            "thumbnail,scale=400:225:force_original_aspect_ratio=increase,crop=400:225",
            "-frames:v",
            "1",
        ])
        .arg(output);
    util::run_with_timeout(&mut command, Duration::from_secs(20))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_has_no_thumbnail() {
        assert!(generate(Path::new("/nonexistent/video.mp4")).is_none());
    }
}
