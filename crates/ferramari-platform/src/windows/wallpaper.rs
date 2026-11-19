//! Windows static wallpaper support.
//!
//! The desktop image is read and written through `SystemParametersInfoW`
//! (`SPI_GETDESKWALLPAPER` / `SPI_SETDESKWALLPAPER`). The user's original path
//! is remembered in `background-original.json` before the first frame is
//! installed, so a crash cannot lose it to the generated-frame cache.
//!
//! Rendering the frame itself is shared with the Linux backend through
//! [`crate::static_frame`]; only the final installation step differs.

use std::fs;
use std::path::{Path, PathBuf};

use ferramari_core::paths;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_GETDESKWALLPAPER,
    SPI_SETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

use crate::win32util::{from_wide, wide};

/// Sidecar that remembers the user's real wallpaper. It deliberately lives in
/// the config directory, not in the prunable frame cache.
const ORIGINAL_STATE_FILE: &str = "background-original.json";
/// `SPI_GETDESKWALLPAPER` fills a caller buffer; 512 UTF-16 units covers the
/// classic `MAX_PATH` limit and the extended paths Windows accepts today.
const WALLPAPER_BUFFER: usize = 512;

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OriginalBackground {
    wallpaper: String,
}

fn original_state_path() -> PathBuf {
    paths::config_dir().join(ORIGINAL_STATE_FILE)
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

/// The current desktop image path, or `None` when Windows has no wallpaper
/// configured (a solid colour) or the call failed in a non-interactive
/// session.
fn current_wallpaper() -> Option<PathBuf> {
    let mut buffer = vec![0u16; WALLPAPER_BUFFER];
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buffer.len() as u32,
            Some(buffer.as_mut_ptr().cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    let value = from_wide(&buffer);
    (!value.trim().is_empty()).then(|| PathBuf::from(value))
}

fn set_wallpaper(path: &Path) -> bool {
    let value = wide(&path.to_string_lossy());
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(value.as_ptr() as *mut _),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
        .is_ok()
    }
}

/// Windows path identity: case-insensitive and both separators accepted.
fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |value: &Path| {
        value
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

/// Whether two paths point into the same directory tree. Windows paths are
/// case-insensitive and accept both separators, so neither a plain `starts_with`
/// nor a string equality is correct.
fn is_under(path: &Path, dir: &Path) -> bool {
    let normalize = |value: &Path| {
        value
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    let path = normalize(path);
    let dir = normalize(dir);
    path == dir || path.starts_with(&(dir + "\\"))
}

/// True when the path is a frame Ferramari produced, which may be pruned and
/// must never be treated as the user's original. The directories are passed
/// in so the classification can be tested without touching the real cache.
fn is_generated(path: &Path, static_dir: &Path, config_dir: &Path) -> bool {
    is_under(path, static_dir) || is_under(path, config_dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Background {
    /// A real file that was there before Ferramari ran.
    Original(PathBuf),
    /// A frame an earlier run installed. Replacing it is fine; restoring it
    /// means leaving the newest frame in place.
    Ours,
    /// Nothing usable (solid colour, missing file, non-interactive session).
    Missing,
}

fn classify_background(current: Option<&Path>, saved: Option<&OriginalBackground>) -> Background {
    classify_background_in(
        current,
        saved,
        &paths::static_cache_dir(),
        &paths::config_dir(),
    )
}

/// Whether the desktop image changed to a real file other than the one
/// Ferramari put there. Such a change came from the user (Settings, the desktop
/// context menu or another program), so quitting must leave it in place
/// instead of restoring the image captured at startup.
///
/// `active` is the frame currently installed; `original` is the startup image
/// recorded in [`Background::Original`]. When no frame is installed the
/// startup image is the reference, so a desktop that still shows it is not a
/// user change.
fn user_replaced_background(
    current: Option<&Path>,
    active: Option<&Path>,
    original: Option<&Path>,
    static_dir: &Path,
    config_dir: &Path,
) -> bool {
    let Some(current) = current else {
        return false;
    };
    if is_generated(current, static_dir, config_dir) || !current.is_file() {
        return false;
    }
    match active {
        Some(active) => !same_path(current, active),
        None => original.is_none_or(|original| !same_path(current, original)),
    }
}

/// Whether restoring the recorded original would only repeat what Explorer is
/// already showing.
///
/// This is deliberately separate from [`StaticBackground::restore`] so the
/// decision can be tested without touching the live desktop. It only skips
/// when no frame is active: an active frame means disabling the static
/// wallpaper must still put the original back.
fn restore_can_be_skipped(
    current: Option<&Path>,
    background: &Background,
    active: Option<&Path>,
) -> bool {
    active.is_none()
        && matches!(
            background,
            Background::Original(original)
                if current.is_some_and(|value| same_path(value, original))
        )
}

/// The real wallpaper to remember before installing a frame, or `None` when
/// the desktop still shows what Ferramari already knows about. A user-chosen
/// image must become the new original before it is covered, otherwise a later
/// quit or static-wallpaper toggle would roll it back.
fn wallpaper_to_remember(
    current: Option<&Path>,
    background: &Background,
    static_dir: &Path,
    config_dir: &Path,
) -> Option<PathBuf> {
    let current = current?;
    if is_generated(current, static_dir, config_dir) || !current.is_file() {
        return None;
    }
    if matches!(background, Background::Original(path) if same_path(path, current)) {
        return None;
    }
    Some(current.to_path_buf())
}

fn classify_background_in(
    current: Option<&Path>,
    saved: Option<&OriginalBackground>,
    static_dir: &Path,
    config_dir: &Path,
) -> Background {
    let ours = |path: &Path| is_generated(path, static_dir, config_dir);
    // A real image currently on the desktop is the truth. Generated frames
    // are not, and must not hide a sidecar record of the user's wallpaper:
    // without this order a crash after installing a frame would restore the
    // frame even though the original path was saved.
    if let Some(path) = current {
        if !ours(path) && path.is_file() {
            return Background::Original(path.to_path_buf());
        }
    }
    if let Some(saved) = saved {
        let path = PathBuf::from(&saved.wallpaper);
        if !saved.wallpaper.trim().is_empty() && path.is_file() && !ours(&path) {
            return Background::Original(path);
        }
    }
    if current.is_some_and(ours) {
        return Background::Ours;
    }
    Background::Missing
}

fn protected_cache_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(current) = current_wallpaper() {
        if is_under(&current, &paths::static_cache_dir()) {
            files.push(current);
        }
    }
    if let Some(saved) = read_original_state() {
        let path = PathBuf::from(saved.wallpaper);
        if is_under(&path, &paths::static_cache_dir()) {
            files.push(path);
        }
    }
    files
}

/// Extract a frame at the golden-ratio point of the video, blur it and return
/// the generated file. The decoding itself is shared with Linux.
pub fn render_static_frame(video: &Path, blur_radius: u32) -> Option<PathBuf> {
    crate::static_frame::render_static_frame(video, blur_radius, &protected_cache_files())
}

/// One-line description of the desktop image for `--doctor`.
pub fn background_diagnostic() -> String {
    let current = current_wallpaper();
    let saved = read_original_state();
    match classify_background(current.as_deref(), saved.as_ref()) {
        Background::Original(path) => format!("user wallpaper {}", path.display()),
        Background::Ours => match current {
            Some(path) => format!(
                "Ferramari frame {} (a previous run took over; the next frame replaces it)",
                path.display()
            ),
            None => "Ferramari frame (no path reported)".to_string(),
        },
        Background::Missing => match current {
            Some(path) => format!(
                "WALLPAPER {} does not exist (this leaves a blank desktop)",
                path.display()
            ),
            None => "no desktop wallpaper set (solid colour), or this is a non-interactive session"
                .to_string(),
        },
    }
}

/// Static wallpaper state used by the server's static-frame job.
pub struct StaticBackground {
    supported: bool,
    background: Background,
    active_file: Option<PathBuf>,
}

impl StaticBackground {
    /// A background that cannot be read or written. Headless tests use it so
    /// constructing an `App` never touches the user's desktop.
    pub fn unsupported() -> Self {
        Self {
            supported: false,
            background: Background::Missing,
            active_file: None,
        }
    }

    pub fn capture() -> Self {
        let supported = current_wallpaper().is_some();
        let current = current_wallpaper();
        let saved = read_original_state();
        let background = classify_background(current.as_deref(), saved.as_ref());
        // Persist a genuine original immediately: a later crash must not be
        // able to lose it to the frame cache.
        if let Background::Original(path) = &background {
            let value = OriginalBackground {
                wallpaper: path.to_string_lossy().into_owned(),
            };
            if saved.as_ref() != Some(&value) {
                write_original_state(&value);
            }
        }
        Self {
            supported,
            background,
            active_file: None,
        }
    }

    /// `SystemParametersInfoW` is always present; a failed read means session
    /// 0 or a locked-down shell, where installing a wallpaper would fail too.
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    pub fn apply_static_frame(&mut self, file: &Path) -> Option<PathBuf> {
        if !self.supported {
            return None;
        }
        // Before covering the desktop, keep a wallpaper the user picked while
        // Ferramari was running. Without this a static toggle or quit would
        // restore the image from server startup instead.
        self.remember_current_background();
        if !set_wallpaper(file) {
            return None;
        }
        self.cleanup_files_except(Some(file));
        self.active_file = Some(file.to_path_buf());
        log::info!("[StaticWallpaper] Applied {}", file.display());
        Some(file.to_path_buf())
    }

    fn remember_current_background(&mut self) {
        let current = current_wallpaper();
        let Some(path) = wallpaper_to_remember(
            current.as_deref(),
            &self.background,
            &paths::static_cache_dir(),
            &paths::config_dir(),
        ) else {
            return;
        };
        log::info!(
            "[StaticWallpaper] Remembering the wallpaper set while running: {}",
            path.display()
        );
        let value = OriginalBackground {
            wallpaper: path.to_string_lossy().into_owned(),
        };
        self.background = Background::Original(path);
        write_original_state(&value);
    }

    pub fn restore(&mut self) {
        // A wallpaper the user picked after our frame went up is their choice,
        // not a state to roll back. Without this check quitting would restore
        // the image captured at startup and silently discard the newer one.
        let current = current_wallpaper();

        // `update_static_wallpaper` calls this after every rebuild while the
        // static wallpaper is off. `SPI_SETDESKWALLPAPER` is not a cheap
        // no-op: Explorer replaces its wallpaper WorkerW and the live surface
        // dies with it. If the desktop already shows the recorded original,
        // there is nothing to restore and nothing to touch.
        if restore_can_be_skipped(
            current.as_deref(),
            &self.background,
            self.active_file.as_deref(),
        ) {
            return;
        }

        let original = match &self.background {
            Background::Original(path) => Some(path.as_path()),
            Background::Ours | Background::Missing => None,
        };
        if user_replaced_background(
            current.as_deref(),
            self.active_file.as_deref(),
            original,
            &paths::static_cache_dir(),
            &paths::config_dir(),
        ) {
            if let Some(path) = &current {
                log::info!(
                    "[StaticWallpaper] Keeping the wallpaper set while running: {}",
                    path.display()
                );
                let value = OriginalBackground {
                    wallpaper: path.to_string_lossy().into_owned(),
                };
                self.background = Background::Original(path.clone());
                write_original_state(&value);
            }
            self.active_file = None;
            self.cleanup_files_except(None);
            return;
        }
        match self.background.clone() {
            Background::Original(path) => {
                if set_wallpaper(&path) {
                    log::info!("[StaticWallpaper] Restored {}", path.display());
                } else {
                    log::warn!("[StaticWallpaper] Could not restore {}", path.display());
                }
            }
            // A frame from an earlier run: we do not know the user's own
            // wallpaper, so leave a real image in place rather than a blank
            // desktop.
            Background::Ours => log::info!("[StaticWallpaper] Keeping the last frame"),
            Background::Missing => log::warn!("[StaticWallpaper] Original wallpaper unknown"),
        }
        self.active_file = None;
        self.cleanup_files_except(None);
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
            "ferramari-windows-background-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn a_real_wallpaper_is_the_original() {
        let root = temp_dir("original");
        let cache = root.join("static");
        let config = root.join("config");
        let desktop = make(&root, "wallpaper.png");
        assert_eq!(
            classify_background_in(Some(&desktop), None, &cache, &config),
            Background::Original(desktop.clone())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_generated_frame_is_never_treated_as_the_original() {
        let root = temp_dir("ours");
        let cache = root.join("static");
        let config = root.join("config");
        fs::create_dir_all(&cache).unwrap();
        let frame = make(&cache, "frame.png");
        assert!(is_generated(&frame, &cache, &config));
        assert_eq!(
            classify_background_in(Some(&frame), None, &cache, &config),
            Background::Ours
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_saved_original_survives_our_frame_being_the_live_image() {
        let root = temp_dir("saved");
        let cache = root.join("static");
        let config = root.join("config");
        fs::create_dir_all(&cache).unwrap();
        let desktop = make(&root, "wallpaper.png");
        let frame = make(&cache, "frame.png");
        let saved = OriginalBackground {
            wallpaper: desktop.to_string_lossy().into_owned(),
        };
        assert_eq!(
            classify_background_in(Some(&frame), Some(&saved), &cache, &config),
            Background::Original(desktop.clone())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_wallpaper_is_missing_not_restorable() {
        let root = temp_dir("missing");
        let cache = root.join("static");
        let config = root.join("config");
        let gone = root.join("gone.png");
        assert_eq!(
            classify_background_in(Some(&gone), None, &cache, &config),
            Background::Missing
        );
        assert_eq!(
            classify_background_in(None, None, &cache, &config),
            Background::Missing
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_wallpaper_chosen_while_running_is_kept() {
        let root = temp_dir("user-change");
        let cache = root.join("static");
        let config = root.join("config");
        fs::create_dir_all(&cache).unwrap();
        let startup = make(&root, "startup.png");
        let frame = make(&cache, "frame.png");
        let chosen = make(&root, "chosen.png");
        assert!(user_replaced_background(
            Some(&chosen),
            Some(&frame),
            Some(&startup),
            &cache,
            &config
        ));
        assert!(!user_replaced_background(
            Some(&frame),
            Some(&frame),
            Some(&startup),
            &cache,
            &config
        ));
        assert!(!user_replaced_background(
            None,
            Some(&frame),
            Some(&startup),
            &cache,
            &config
        ));
        // Static wallpaper off: no frame is installed, but a real image that
        // differs from the startup one is still the user's choice.
        assert!(user_replaced_background(
            Some(&chosen),
            None,
            Some(&startup),
            &cache,
            &config
        ));
        assert!(!user_replaced_background(
            Some(&startup),
            None,
            Some(&startup),
            &cache,
            &config
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn another_generated_frame_is_not_a_user_change() {
        let root = temp_dir("other-frame");
        let cache = root.join("static");
        let config = root.join("config");
        fs::create_dir_all(&cache).unwrap();
        let frame = make(&cache, "frame.png");
        let stale = make(&cache, "stale.png");
        assert!(!user_replaced_background(
            Some(&stale),
            Some(&frame),
            None,
            &cache,
            &config
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_chosen_wallpaper_is_remembered_before_a_frame_covers_it() {
        let root = temp_dir("remember");
        let cache = root.join("static");
        let config = root.join("config");
        fs::create_dir_all(&cache).unwrap();
        let startup = make(&root, "startup.png");
        let chosen = make(&root, "chosen.png");
        let frame = make(&cache, "frame.png");
        let original = Background::Original(startup.clone());
        assert_eq!(
            wallpaper_to_remember(Some(&chosen), &original, &cache, &config),
            Some(chosen.clone())
        );
        assert_eq!(
            wallpaper_to_remember(Some(&startup), &original, &cache, &config),
            None
        );
        assert_eq!(
            wallpaper_to_remember(Some(&frame), &original, &cache, &config),
            None
        );
        assert_eq!(
            wallpaper_to_remember(Some(&chosen), &Background::Ours, &cache, &config),
            Some(chosen.clone())
        );
        assert_eq!(
            wallpaper_to_remember(Some(&chosen), &Background::Missing, &cache, &config),
            Some(chosen)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_identity_ignores_case_and_separators() {
        let left = PathBuf::from(r"C:\Users\Me\Wallpaper.PNG");
        let right = PathBuf::from(r"c:/users/me/wallpaper.png");
        assert!(same_path(&left, &right));
        assert!(!same_path(&left, &PathBuf::from(r"C:\Users\Me\Other.png")));
    }

    #[test]
    fn path_prefix_checks_ignore_case_and_separators() {
        let root = temp_dir("prefix");
        let nested = root.join("sub").join("frame.png");
        assert!(is_under(&nested, &root));
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase()).join("frame.png");
        assert!(is_under(&upper, &root));
        let other = temp_dir("other");
        assert!(!is_under(&other.join("frame.png"), &root));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(other);
    }

    #[test]
    fn the_state_roundtrips_through_json() {
        let state = OriginalBackground {
            wallpaper: r"C:\Users\me\wallpaper.png".to_string(),
        };
        let text = serde_json::to_string(&state).unwrap();
        let read: OriginalBackground = serde_json::from_str(&text).unwrap();
        assert_eq!(read, state);
    }

    #[test]
    fn the_live_probes_do_not_panic() {
        let _ = current_wallpaper();
        let _ = background_diagnostic();
        let _ = protected_cache_files();
    }

    #[test]
    fn restoring_the_original_that_is_already_shown_is_a_no_op() {
        let root = temp_dir("restore-noop");
        let original = make(&root, "original.png");
        let other = make(&root, "other.png");
        let background = Background::Original(original.clone());

        assert!(restore_can_be_skipped(Some(&original), &background, None));
        assert!(!restore_can_be_skipped(Some(&other), &background, None));
        assert!(!restore_can_be_skipped(None, &background, None));

        // An installed frame must still be replaced by the original when the
        // user disables static wallpaper.
        assert!(!restore_can_be_skipped(
            Some(&original),
            &background,
            Some(&other)
        ));

        let _ = fs::remove_dir_all(root);
    }
}
