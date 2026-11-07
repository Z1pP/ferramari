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

use ferramari_core::paths;
use ferramari_core::util;

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
    prune_static_cache(&cache_dir, &file, &protected_cache_files());
    Some(file)
}

/// Keep the cache bounded; one PNG per video can be several megabytes.
/// Frames that something is still pointing at: our persisted original plus any
/// live GNOME background. Deleting one of these is what produced the gray
/// desktop after a restart.
fn protected_cache_files() -> Vec<PathBuf> {
    let mut uris: Vec<String> = read_original_state()
        .iter()
        .flat_map(|state| [state.light.clone(), state.dark.clone()])
        .collect();
    if schema_available() {
        uris.push(gsettings_get("picture-uri").unwrap_or_default());
        uris.push(gsettings_get("picture-uri-dark").unwrap_or_default());
    }
    uris.iter()
        .map(|uri| PathBuf::from(path_from_uri(uri).to_string()))
        .collect()
}

fn prune_static_cache(dir: &Path, keep: &Path, protected: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    // The cap counts every file in the directory, so the ones that must stay
    // reduce the budget instead of being added on top of it.
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

/// Sidecar that remembers the user's real background. It cannot live in the
/// static frame cache: those files are pruned, and pointing GNOME at a deleted
/// file leaves a gray desktop.
const ORIGINAL_STATE_FILE: &str = "background-original.json";

fn original_state_path() -> PathBuf {
    paths::config_dir().join(ORIGINAL_STATE_FILE)
}

fn path_from_uri(uri: &str) -> &str {
    uri.strip_prefix("file://").unwrap_or(uri)
}

/// True when `uri` points into our own prunable frame cache, i.e. a previous
/// run of Ferramari set it.
fn is_cache_uri(uri: &str, cache_dir: &Path) -> bool {
    let path = path_from_uri(uri);
    !path.is_empty() && Path::new(path).starts_with(cache_dir)
}

/// A background GNOME can actually show: not one of our prunable frames, and
/// either a non-file URI we do not own or a file that still exists.
fn is_restorable(uri: &str, cache_dir: &Path) -> bool {
    if is_cache_uri(uri, cache_dir) {
        return false;
    }
    match uri.strip_prefix("file://") {
        Some(path) => Path::new(path).is_file(),
        // `resource://`, `none` and anything we did not create: leave it be.
        None => true,
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OriginalBackground {
    light: String,
    dark: String,
}

fn read_original_state() -> Option<OriginalBackground> {
    let text = fs::read_to_string(original_state_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_original_state(state: &OriginalBackground) {
    let path = original_state_path();
    let Ok(text) = serde_json::to_string_pretty(state) else {
        return;
    };
    let temp = path.with_extension("json.tmp");
    if fs::write(&temp, format!("{text}\n")).is_ok() {
        let _ = fs::rename(&temp, &path);
    }
}

/// Who owns the desktop background right now, and what to hand back.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Background {
    /// A background we can restore because the file is real and not ours.
    Original((String, String)),
    /// A frame a previous Ferramari set. We may replace it, and "restoring"
    /// means leaving the newest frame in place rather than deleting it.
    Ours,
    /// Nothing usable: the setting points at a file that no longer exists.
    Missing,
}

/// Classify the current setting, falling back to the persisted copy when the
/// live setting was already overwritten by a previous run.
fn classify_background(
    current: &(String, String),
    saved: Option<&OriginalBackground>,
    cache_dir: &Path,
) -> Background {
    let (light, dark) = current;
    let ours = is_cache_uri(light, cache_dir) && is_cache_uri(dark, cache_dir);
    if !ours && is_restorable(light, cache_dir) && is_restorable(dark, cache_dir) {
        return Background::Original((light.clone(), dark.clone()));
    }
    if let Some(saved) = saved {
        if is_restorable(&saved.light, cache_dir) && is_restorable(&saved.dark, cache_dir) {
            return Background::Original((saved.light.clone(), saved.dark.clone()));
        }
    }
    if ours {
        Background::Ours
    } else {
        Background::Missing
    }
}

/// One-line description of the desktop background for `--doctor`. A dangling
/// URI is exactly the gray-desktop symptom, so it has to be visible without
/// reading a log from the last restart.
pub fn background_diagnostic() -> String {
    if !paths::is_gnome() {
        return "not GNOME, background left alone".to_string();
    }
    if !schema_available() {
        return "background schema unavailable".to_string();
    }
    let light = gsettings_get("picture-uri");
    let dark = gsettings_get("picture-uri-dark");
    match (light, dark) {
        (Some(light), Some(dark)) => describe_background(
            &(light, dark),
            read_original_state().as_ref(),
            &paths::static_cache_dir(),
        ),
        _ => "background schema unavailable".to_string(),
    }
}

fn describe_background(
    current: &(String, String),
    saved: Option<&OriginalBackground>,
    cache_dir: &Path,
) -> String {
    match classify_background(current, saved, cache_dir) {
        Background::Original((light, _)) => format!("user background {light}"),
        Background::Ours => format!(
            "Ferramari frame {} (a previous run took over; the next frame replaces it)",
            path_from_uri(&current.0)
        ),
        Background::Missing => {
            format!(
                "DANGLING {} does not exist (this is a gray desktop)",
                path_from_uri(&current.0)
            )
        }
    }
}

pub struct GnomeBackground {
    /// Whether the background schema is reachable through gsettings.
    schema: bool,
    background: Background,
    active_file: Option<PathBuf>,
}

impl GnomeBackground {
    pub fn capture() -> Self {
        if !paths::is_gnome() {
            return Self {
                schema: false,
                background: Background::Missing,
                active_file: None,
            };
        }
        if paths::is_flatpak() {
            let light = host_gsettings_get("picture-uri");
            let dark = host_gsettings_get("picture-uri-dark");
            return Self {
                schema: light.is_some(),
                background: match light.zip(dark) {
                    Some(pair) => classify_background(&pair, None, &paths::static_cache_dir()),
                    None => Background::Missing,
                },
                active_file: None,
            };
        }
        let schema = schema_available();
        let current = schema.then(|| {
            (
                gsettings_get("picture-uri"),
                gsettings_get("picture-uri-dark"),
            )
        });
        let cache_dir = paths::static_cache_dir();
        let background = match current {
            Some((Some(light), Some(dark))) => {
                classify_background(&(light, dark), read_original_state().as_ref(), &cache_dir)
            }
            _ => Background::Missing,
        };
        // Persist a genuine original immediately: a later crash must not be
        // able to lose it to the frame cache.
        if let Background::Original((light, dark)) = &background {
            write_original_state(&OriginalBackground {
                light: light.clone(),
                dark: dark.clone(),
            });
        }
        Self {
            schema,
            background,
            active_file: None,
        }
    }

    /// Can we write the desktop background at all?
    ///
    /// An unknown original must not disable this on a normal session: pointing
    /// GNOME at a real frame is exactly how a dangling URI left by an older
    /// build heals itself. Flatpak writes through `flatpak-spawn`, so it only
    /// proceeds when the host value was actually read.
    pub fn is_supported(&self) -> bool {
        self.schema || (paths::is_flatpak() && self.background != Background::Missing)
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
        // Deliberately keep `background`: the same process may apply a static
        // wallpaper again later (mode switch, blur change) and must still know
        // what to put back.
        match self.background.clone() {
            Background::Original((light, dark)) => {
                self.set_uri("picture-uri", &light);
                self.set_uri("picture-uri-dark", &dark);
                log::info!("[StaticWallpaper] Restored {light}");
            }
            // A frame set by an earlier run: we do not know the user's own
            // background, so leave a real image in place rather than risk a
            // dangling URI. Protected from pruning by `protected_cache_files`.
            Background::Ours => log::info!("[StaticWallpaper] Keeping the last frame"),
            Background::Missing => log::warn!("[StaticWallpaper] Original background unknown"),
        }
        self.active_file = None;
        self.cleanup_files_except(None);
    }

    fn set_uri(&self, key: &str, value: &str) {
        if self.schema {
            gsettings_set(key, value);
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

/// Whether the background schema can be read through the `gsettings` tool.
/// This replaces the old `gio::SettingsSchemaSource` lookup without pulling
/// GLib into the process.
fn schema_available() -> bool {
    let mut command = Command::new("gsettings");
    command.args(["list-keys", BACKGROUND_SCHEMA]);
    let output = util::output_with_timeout(&mut command, Duration::from_secs(5));
    output.is_some_and(|out| out.status.success())
}

/// Read one string key from the background schema. The `gsettings` CLI
/// prints GVariant literals (a quoted string), so the surrounding quotes and
/// type suffix are stripped.
fn gsettings_get(key: &str) -> Option<String> {
    let mut command = Command::new(if paths::is_flatpak() {
        "flatpak-spawn"
    } else {
        "gsettings"
    });
    if paths::is_flatpak() {
        command.arg("--host");
    }
    command.args(["get", BACKGROUND_SCHEMA, key]);
    let output = util::output_with_timeout(&mut command, Duration::from_secs(5))?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let value = raw.trim();
    let unquoted = value.trim_start_matches('\'').trim_end_matches(['\'', 's']);
    Some(unquoted.to_string())
}

fn gsettings_set(key: &str, value: &str) {
    let mut command = Command::new(if paths::is_flatpak() {
        "flatpak-spawn"
    } else {
        "gsettings"
    });
    if paths::is_flatpak() {
        command.arg("--host");
    }
    command.args(["set", BACKGROUND_SCHEMA, key, value]);
    let _ = util::run_with_timeout(&mut command, Duration::from_secs(5));
}

/// Flatpak reads always go through `flatpak-spawn`; `gsettings_get` already
/// does that, but the capture() Flatpak branch keeps a dedicated call site so
/// the flatpak probe stays explicit.
fn host_gsettings_get(key: &str) -> Option<String> {
    let mut command = Command::new("flatpak-spawn");
    command.args(["--host", "gsettings", "get", BACKGROUND_SCHEMA, key]);
    let output = util::output_with_timeout(&mut command, Duration::from_secs(5))?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let value = raw.trim();
    let unquoted = value.trim_start_matches('\'').trim_end_matches(['\'', 's']);
    Some(unquoted.to_string())
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
            "ferramari-background-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make(dir: &Path, name: &str) -> String {
        fs::write(dir.join(name), b"x").unwrap();
        format!("file://{}", dir.join(name).display())
    }

    #[test]
    fn a_real_background_is_the_original() {
        let root = temp_dir("original");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let desktop = make(&root, "ubuntu.png");
        let pair = (desktop.clone(), desktop.clone());
        assert_eq!(
            classify_background(&pair, None, &cache),
            Background::Original(pair.clone())
        );
        assert!(is_restorable(&desktop, &cache));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn our_own_frame_is_never_treated_as_the_original() {
        // The 1.0.1 regression: the frame cache moved to ~/.cache, so a run
        // could record one of its own prunable frames as the user's original.
        let root = temp_dir("ours");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let frame = make(&cache, "frame.png");
        let pair = (frame.clone(), frame.clone());
        assert!(is_cache_uri(&frame, &cache));
        assert_eq!(classify_background(&pair, None, &cache), Background::Ours);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_persisted_original_survives_our_frame_being_the_live_setting() {
        let root = temp_dir("persisted");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let desktop = make(&root, "ubuntu.png");
        let frame = make(&cache, "frame.png");
        let saved = OriginalBackground {
            light: desktop.clone(),
            dark: desktop.clone(),
        };
        let live = (frame.clone(), frame);
        assert_eq!(
            classify_background(&live, Some(&saved), &cache),
            Background::Original((desktop.clone(), desktop))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_deleted_original_is_missing_not_restorable() {
        // Restoring this is exactly what painted the gray desktop.
        let root = temp_dir("deleted");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let gone = format!("file://{}/gone.png", root.display());
        let pair = (gone.clone(), gone);
        assert!(!is_restorable(&pair.0, &cache));
        assert_eq!(
            classify_background(&pair, None, &cache),
            Background::Missing
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_deleted_persisted_original_does_not_become_restorable() {
        let root = temp_dir("deleted-saved");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let frame = make(&cache, "frame.png");
        let saved = OriginalBackground {
            light: format!("file://{}/gone.png", root.display()),
            dark: format!("file://{}/gone.png", root.display()),
        };
        let live = (frame.clone(), frame);
        assert_eq!(
            classify_background(&live, Some(&saved), &cache),
            Background::Ours
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn non_file_uris_we_do_not_own_are_left_alone() {
        let root = temp_dir("resource");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        for uri in ["resource:///org/gnome/shell/theme/bg.png", "none"] {
            assert!(is_restorable(uri, &cache), "{uri} should be restorable");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pruning_keeps_protected_frames_and_stays_within_the_limit() {
        let root = temp_dir("prune");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let mut names = Vec::new();
        for index in 0..STATIC_CACHE_LIMIT + 3 {
            let name = format!("frame{index:02}.png");
            fs::write(cache.join(&name), b"x").unwrap();
            // Oldest first so the sort order is deterministic.
            names.push(name);
        }
        let newest = "newest.png";
        fs::write(cache.join(newest), b"x").unwrap();
        // One existing frame is the user's recorded original and must survive.
        let protected_name = "frame00.png";
        let protected = vec![cache.join(protected_name)];
        prune_static_cache(&cache, &cache.join(newest), &protected);

        assert!(cache.join(newest).is_file(), "the kept frame vanished");
        assert!(
            cache.join(protected_name).is_file(),
            "a protected frame was pruned"
        );
        let remaining = fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
            .count();
        assert!(
            remaining <= STATIC_CACHE_LIMIT,
            "cache not bounded: {remaining} files"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_diagnostic_names_a_dangling_uri() {
        let root = temp_dir("diag-dangling");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let gone = format!("file://{}/gone.png", root.display());
        let text = describe_background(&(gone.clone(), gone), None, &cache);
        assert!(text.contains("DANGLING"), "unexpected: {text}");
        assert!(text.contains("gone.png"), "must name the file: {text}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_diagnostic_distinguishes_our_frame_from_a_user_background() {
        let root = temp_dir("diag-kinds");
        let cache = root.join("static");
        fs::create_dir_all(&cache).unwrap();
        let frame = make(&cache, "frame.png");
        let ours = describe_background(&(frame.clone(), frame), None, &cache);
        assert!(ours.contains("Ferramari frame"), "unexpected: {ours}");

        let desktop = make(&root, "ubuntu.png");
        let theirs = describe_background(&(desktop.clone(), desktop.clone()), None, &cache);
        assert!(theirs.contains("user background"), "unexpected: {theirs}");
        assert!(
            theirs.contains("ubuntu.png"),
            "must name the file: {theirs}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn state_roundtrips_through_json() {
        let root = temp_dir("state");
        let path = root.join("state.json");
        let state = OriginalBackground {
            light: "file:///a.png".to_string(),
            dark: "file:///b.png".to_string(),
        };
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&state).unwrap()),
        )
        .unwrap();
        let read: OriginalBackground =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, state);
        let _ = fs::remove_dir_all(root);
    }
}
