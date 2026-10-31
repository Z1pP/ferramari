# Changelog

## 1.1.0 — 2026-10-31

- Light, dark and system colour schemes in Settings → System → Theme.
- The selected theme is stored in `config.json` and exposed as the `theme`
  D-Bus property; changes apply immediately, without a restart.
- A dark-theme screenshot is included in the README.

## 1.0.1 — 2026-10-31

Performance and idle-power improvements.

- Local thumbnails: input seek, no `thumbnail` filter, two-worker pool and
  cancellation of stale grids (2.2× on 1080p, 4.5× on 4K).
- Static wallpaper frames cached by path, mtime and blur radius; bounded to 12
  files. A restart reuses the frame in ~55 ms instead of ~860 ms.
- Event-driven command loop driven by a wakeup pipe; idle voluntary context
  switches fall from ~26/s to ~1.2/s.
- Volume and blur changes are debounced into one atomic config write, flushed
  on quit and shutdown.
- The GNOME desktop-icon workaround runs only when a wallpaper window exists.
- Video-to-video switches on one monitor reuse the existing mpv and window;
  `loadfile` is sent asynchronously so the GTK thread never waits for a decode.

## 1.0.0 — 2026-10-31

First stable release.

- Reliable mpv supervision, crash-safe config and library data.
- Multi-monitor sources, hotplug handling and per-monitor rotation.
- Local library: favorites, recent history, collections and rotation.
- Catalogue: search, categories, sorting, resolution filters and frame previews.
- Web pages, streaming, static wallpaper and power profiles.
- Suspend/resume, screen lock, battery pause and autostart integration.
- English, Russian and Japanese UI.
- Flatpak manifest, tagged releases, CI and testing documentation.

## 0.9.0 — 2026-10-31

- Performance, Balanced and Power saving playback profiles.
- Power saving uses frame dropping, one decoder thread, bilinear scaling and a 30 fps filter.
- Optional pause when running on battery; playback resumes when AC returns.
- Battery percentage and AC/battery state shown in Settings.

## 0.8.0 — 2026-10-31

- Assign a library collection to an individual monitor from the Home page.
- Rotate that collection in order or randomly on a configurable interval.
- Settings page controls rotation mode and interval (minutes).
- Rotation runs on the GTK loop with a five-second tick and never blocks the UI.

## 0.7.0 — 2026-10-31

Library, catalogue filters and Flatpak.

- Local library: favorites, recent wallpapers and collections.
- Collection create/rename/delete, add/remove videos and filtering.
- Catalogue sorting by title, category, size and duration.
- Resolution filters and metadata (size/duration/resolution) from previews and downloads.
- Flatpak manifest and host-tool bridge for mpv, ffmpeg, ffprobe and yt-dlp.
- Testing matrix and smoke checklist in `docs/TESTING.md`.

## 0.6.0 — 2026-10-31

- Dedicated Settings page with playback, wallpaper and system sections.
- Settings is available from the sidebar, the header gear and the Home page.
- Configuration file, About and Quit actions live on the Settings page.
- The old quick-toggles popover was removed to avoid duplicating controls.

## 0.5.0 — 2026-10-31

Home screen and per-monitor wallpapers.

- New Home page with the current wallpaper, playback state and quick links to
  the library, catalogue and settings.
- Monitor cards show name, resolution, current source and playback state.
- Assign a local video to one monitor directly from its card; the rest of the
  monitors keep playing.
- Russian and Japanese strings for the new screen.

## 0.4.0 — 2026-10-30

Reliability and recovery.

- Detect an unexpected mpv exit and recreate the player with a capped
  exponential backoff. After five retries the automatic loop stops instead of
  spawning processes forever.
- Kill and reap wedged external commands (`mpv`, `ffprobe`, `ffmpeg`,
  `xdg-user-dir`, Flatpak `gsettings`) at a hard deadline.
- Report WebKit load failures, retry the page once and show one clear error.
- Generate static-wallpaper frames on a worker thread so ffmpeg no longer
  blocks the GTK main loop.
- Write `config.json` atomically and keep `config.json.bak`; recover from a
  truncated or invalid primary file.
- Add monitor reattachment and scaling tests, structured user errors, locked
  CI builds, metadata and translation validation, a RustSec audit and a
  tag-driven release workflow.
- Requires Rust 1.88 (security update of the ZIP parser).

## 0.3.0 — 2026-10-29

- Russian interface and a language selector (System, English, Russian,
  Japanese).
- Russian plural forms through gettext.
- Translated tray menu.

## 0.2.0 — 2026-10-29

- Live wallpaper catalogue with categories, search and pagination.
- Still preview, six-frame sheet, download progress, cache management and
  install through the monitor menu.

## 0.1.0 — 2026-10-26

- First release: local video, streaming, web-page wallpapers, multi-monitor,
  GNOME static wallpaper, tray, D-Bus and autostart.
