//! Static-frame extraction shared by the Linux GNOME background and the
//! Windows desktop wallpaper.
//!
//! Rendering is deliberately platform-neutral: ffmpeg writes a PNG into the
//! per-user cache, keyed by the video path, mtime and blur radius. Only the
//! final "install this file as the wallpaper" step belongs to a backend.

use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use ferramari_core::{paths, util};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const FRAME_TIMEOUT: Duration = Duration::from_secs(60);

/// Keep the cache bounded; one PNG per video can be several megabytes.
pub const STATIC_CACHE_LIMIT: usize = 12;

/// Extract a frame at the golden-ratio point of the video, blur it and return
/// the generated file. This is CPU/GPU work and must run off the command loop.
/// Results are cached by `(path, mtime, blur)` so restarts and source switches
/// do not decode the video again.
///
/// `protected` are files the caller still points at (a user's original
/// wallpaper, the currently installed frame) and must not be pruned.
pub fn render_static_frame(
    video: &Path,
    blur_radius: u32,
    protected: &[PathBuf],
) -> Option<PathBuf> {
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
    prune_static_cache(&cache_dir, &file, protected);
    Some(file)
}

/// Delete the oldest generated frames so the directory stays within
/// [`STATIC_CACHE_LIMIT`]. Pinned files count against the budget instead of
/// being added on top of it.
pub fn prune_static_cache(dir: &Path, keep: &Path, protected: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut pinned = 0usize;
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "png")
            || path.to_string_lossy().ends_with(".part.png")
        {
            continue;
        }
        if path == keep || protected.contains(&path) {
            pinned += 1;
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        candidates.push((modified, path));
    }
    let allowed = STATIC_CACHE_LIMIT.saturating_sub(pinned);
    if candidates.len() <= allowed {
        return;
    }
    candidates.sort_by_key(|(modified, _)| *modified);
    let remove = candidates.len() - allowed;
    for (_, path) in candidates.into_iter().take(remove) {
        let _ = fs::remove_file(path);
    }
}

/// Length of the video in seconds, or `None` when ffprobe is unavailable or
/// the container does not report one. Callers fall back to the first frame.
pub fn probe_duration(video: &Path) -> Option<f64> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-static-frame-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pruning_keeps_pinned_frames_and_stays_within_the_limit() {
        let dir = temp_dir("prune");
        let mut names = Vec::new();
        for index in 0..STATIC_CACHE_LIMIT + 3 {
            let name = format!("frame{index:02}.png");
            fs::write(dir.join(&name), b"x").unwrap();
            names.push(name);
        }
        let keep = dir.join("keep.png");
        fs::write(&keep, b"x").unwrap();
        let protected = dir.join("protected.png");
        fs::write(&protected, b"x").unwrap();

        prune_static_cache(&dir, &keep, std::slice::from_ref(&protected));

        assert!(keep.is_file());
        assert!(protected.is_file());
        let remaining = fs::read_dir(&dir).unwrap().count();
        assert!(remaining <= STATIC_CACHE_LIMIT, "{remaining}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_files_do_not_count_and_are_not_pruned() {
        let dir = temp_dir("partial");
        let partial = dir.join("thing.part.png");
        fs::write(&partial, b"x").unwrap();
        prune_static_cache(&dir, &dir.join("missing.png"), &[]);
        assert!(partial.is_file());
        let _ = fs::remove_dir_all(dir);
    }
}
