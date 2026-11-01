# Changelog

## 1.9.2 — 2026-11-02

Layout regression from 1.9.1: content was clipped at the right edge.

- The content stack reserved the widest page's minimum width; long Russian
  captions (Settings rows, the Local Video header and action bar) made that
  minimum larger than the window, so every page lost its right edge.
- Text now shrinks instead of pushing the layout: titles, button captions and
  descriptions ellipsize or wrap, and the page header stacks its action row
  below the title when the two no longer fit side by side.
- Verified by measuring every widget against the window's client edge in
  Russian and English at 680–1300 px: zero overflowing widgets.

## 1.9.1 — 2026-11-01

### Fixed

- Gray desktop after a restart with the static wallpaper enabled. Frames moved
  to `$XDG_CACHE_HOME/ferramari/static/` in 1.0.1, but the "original" background
  captured at startup could itself be one of those frames. The cache is capped,
  so pruning deleted the file GNOME was still pointing at and the desktop went
  gray. The original is now persisted to `background-original.json` outside the
  cache, cache files still referenced by GNOME or by that record are never
  pruned, and a background we cannot restore is never overwritten blindly.
  Applying a frame also repairs a dangling URI left by an older build.
- `ferramari --doctor` now reports the desktop background as `user background`,
  `Ferramari frame` or `DANGLING … (this is a gray desktop)`, so this state is
  visible without reading a log from the last restart.
- `prune_static_cache` off-by-one: protected and kept files were not counted
  against the limit, so the cache could exceed it.
- Local Video action bar: the two new package buttons made the bar wider than
  the window, so it clipped the grid and the label. Replaced with a single
  **Package…** menu that appears only for installed packages.

## 1.9.0 — 2026-11-01

Package integrity and authorship.

- `checksums.json` records the SHA-256 of every package file. Loading a package
  that carries it verifies each entry, so a modified or extra file is rejected
  with a message naming the file. Packages without checksums still load.
- `signature.json` is an Ed25519 signature over those checksums, so one
  signature covers the whole package. Verification reports **Trusted**,
  **Unknown author**, **Unsigned** or **Invalid**.
- Ferramari creates a local signing key on first use
  (`~/.config/ferramari/signing-key.json`, mode `0600`) and signs exports
  automatically. Trusted keys live in `trusted-keys.json`; trusting is always
  an explicit choice, and there is no network validation.
- New headless CLI: `ferramari package key|sign|verify|trust|keys`, with exit
  codes for scripting (0 trusted, 1 untrusted, 2 invalid).
- Local Video gains **Sign** and **Verify** for package entries, with a report
  dialog that offers to trust an unknown author. Settings gains a
  **Package signing** section showing the key, the trust store and an export
  button.
- `ring` and `base64` become direct dependencies; `ring` was already in the
  tree through `ureq`, so no new native library is introduced.
- Tests: 140 passing.

## 1.8.0 — 2026-11-01

Eco Engine completion: decoder parking and broader measurements.

- Release the mpv decoder after a local video has been paused out of sight for
  five minutes. The worker captures `time-pos`, switches mpv to idle and sends
  `stop`; the process and IPC socket stay alive. Resuming reloads the same
  source at the saved position, so the cold-start cost is not paid again.
- Parking is deliberately conservative: only local video windows, only while
  the monitor is covered by a fullscreen/maximised window, locked or suspended,
  and never for an explicit user pause or a visible Eco idle. A black frame
  cannot be left in front of the user.
- The park/unpark sequence runs on the existing mpv IPC worker; the GTK main
  loop never waits for `get_property` or `loadfile`.
- Benchmark harness gains web-wallpaper mode (`--mode web --web`), `--label`
  and `--app-log`; `make bench` accepts `MODE`, `WEB` and `LABEL`.
- Measure and document 1080p, synthetic 4K and local-canvas web wallpapers.
  The Power profile reduced GPU load but not CPU on the tested clip; the
  documentation no longer claims a CPU saving.
- Tests: 126 passing.

## 1.7.0 — 2026-11-01

Ecosystem foundation.

- Add declarative catalogue-source plugins (`$XDG_DATA_HOME/ferramari/plugins/
  <id>/plugin.json`). Plugins are never executed; they provide only an HTTP
  API base URL. Invalid ids, non-http URLs and future `engine_min` versions are
  skipped with a log line. See `docs/PLUGIN_API.md`.
- Add the Ferramari Catalog API v1 client (`src/catalog_api.rs`): pagination,
  search/category query encoding, 15 s timeout, 2 MiB body limit, up to 200
  items, HTTP-only preview/download URLs. See `docs/CATALOG_API.md`.
- Add `ferramari --catalog <base>` to inspect a deployment and list accepted
  plugins in `--doctor` and Settings → Community sources.
- Tests: 123 passing.

## 1.6.0 — 2026-11-01

Creator Engine: `.ferramari` packages.

- Add `src/package.rs`: manifest parsing, strict validation, import, export and
  installed-video discovery.
- Validation rejects newer formats/engine versions, path traversal, symlinks,
  wrong extensions, more than 10,000 files and packages over 2 GiB. Import
  revalidates after copying and cleans up on failure.
- Local Video gains **Import…** and **Export .ferramari**; imported video
  packages join the library grid, imported web packages are applied as the
  wallpaper.
- Export writes `wallpaper.<ext>`, a manifest and an optional thumbnail preview,
  then reloads the package before reporting success.
- Add `docs/FERRAMARI_FORMAT.md` and package tests.
- Tests: 117 passing.

## 1.5.0 — 2026-11-01

Cross-desktop diagnostics and session integration.

- Add `ferramari --doctor`: prints session type (X11 / Wayland-XWayland / native
  Wayland), desktop, X11 screen count, backend, tool versions (mpv, ffmpeg,
  ffprobe, yt-dlp, vainfo), DRM render nodes and their drivers, idle service and
  lock services. It never initialises GTK or claims a D-Bus name.
- Show the desktop integration summary in Settings and state explicitly that
  native Wayland surfaces are not implemented.
- Watch `org.freedesktop.login1.Session` `Lock`/`Unlock` as a fallback. Locks
  from the screensaver and logind are aggregated, so a logind unlock cannot
  resume playback while a screensaver still reports locked.
- Add pure tests for session/desktop classification and the “native Wayland is
  never claimed” invariant.
- Tests: 110 passing.

## 1.4.0 — 2026-11-01

Performance and hardening.

- Replace the 300 ms X11 polling loop with a `PropertyNotify`/`StructureNotify`
  watcher. An idle desktop produces no wakeups; fullscreen/maximise changes are
  processed immediately. Transient query errors still keep the last good state.
- Add a Performance page with live CPU, RSS, thread, process and GPU metrics
  (`src/metrics.rs`), sampled every 2 s only while the control window is
  visible.
- Add `scripts/benchmark.py` and `make bench`: isolated config/cache, private
  D-Bus, startup/CPU/RSS/threads/processes/GPU sampling and optional mpv
  recovery. See `docs/BENCHMARKS.md`.
- Add the hardware decoding setting (Auto / VA-API / Off) and pass it as mpv
  `--hwdec`.
- Harden WebKit wallpapers: explicit file-access settings, no developer
  extras, denied permissions, no pop-up windows, and an optional network switch
  that blocks non-local navigations.
- Fix a deadlock when the mpv child exited before `Mpv::Drop`: the IPC worker
  waited in `recv()` while `join()` waited for it. Always wake the worker.
- Report `loadfile` failures from the worker as a user-visible error.
- Remove abandoned `.part` thumbnail/static files older than an hour on start.
- Tests: 107 passing.

## 1.3.0 — 2026-11-01

Eco Engine completion.

- Configurable `idle_timeout_sec` (1–1440 minutes in Settings, 30 s–24 h
  accepted from config) shared with the idle worker through an atomic; changing
  it takes effect on the next 15 s poll.
- Balanced applies a 30 fps `vf` cap through mpv IPC while idle and restores the
  profile filter on activity. Eco pauses on idle. The runtime `vf` property was
  verified against mpv 0.41.
- Per-monitor Eco state (`active`, `idle`, `occluded`, `suspended`) is shown on
  Home monitor cards and exported as the D-Bus `monitor_states` property.
- Fix Mutter idle method name (`GetIdletime`), which made idle detection dead on
  GNOME. Add a probe test that is a no-op on headless CI.
- Coalesce added/removed/size monitor events into one 250 ms rebuild.
- Surface a `loadfile` failure from the mpv worker as a user-visible playback
  error instead of a log-only message.
- Cache the Home video-folder scan for one second so volume-slider commands do
  not re-read the whole directory.
- Tests: 100 passing.

## 1.2.0 — 2026-11-01

Eco Engine core and Core Stability (Phase 0 audit + Phase 1/2 first slice).

- Add a per-monitor Eco Engine (`src/eco.rs`): `Active`, `Idle`, `Occluded`,
  `Suspended`; Quality, Balanced and Eco policies; idle pause only in Eco.
- Pause and mute decisions are evaluated per monitor. A fullscreen game on one
  screen no longer stops the wallpaper on the other screens.
- X11 now reports visible window rectangles; a maximized window with a panel
  inset still counts as covering its monitor. Window state is sorted to avoid
  spurious updates on focus changes.
- Account for GDK logical vs X11 device pixels (`MonitorInfo::scale_factor`),
  so occlusion works on HiDPI/XWayland layouts.
- Move mpv JSON IPC to a dedicated worker thread. `Mpv::spawn` returns without
  waiting for the socket; volume, pause, mute and `loadfile` are queued. A
  wedged mpv can no longer block the GTK main loop for the IPC timeout.
- Move the system-bus/session-bus setup for suspend and end-session/Idle
  watchers off the GTK thread.
- Add idle detection through GNOME Mutter, the freedesktop screensaver, KDE or
  XFCE idle counters (15 s poll on a worker, commands only on threshold
  crossing).
- Flush a pending debounced config write before a monitor hotplug reloads
  `config.json`; a pending volume/blur edit is no longer lost.
- Repair config fields individually: one wrongly typed field no longer resets
  every other setting to its default.
- Keep the previous X11 window sample on a transient query error instead of
  assuming “nothing is maximized”.
- Accept local `file://` web wallpapers (including percent escapes) and reject
  remote `file://host/...` authorities.
- Mute hidden web wallpapers (WebKitGTK has no public freeze API, so JS
  timers/animations are only throttled).
- Read battery state once per construction; power polling only sends a command
  when the state changes.
- Add `docs/AUDIT.md` with the Phase 0 findings and the remaining work.
- Tests: 98 passing (up from 81).

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
