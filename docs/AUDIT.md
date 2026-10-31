# Ferramari audit — 2026-11-01

This is the Phase 0 report from the Ferramari platform brief. It records what
the repository actually does today, what is missing relative to the target
platform, and which findings were addressed in the first stabilisation step.
It deliberately separates verified facts from untested assumptions.

Environment used for the audit:

- Ubuntu 26.04, GNOME Shell 50.1, Wayland session, Ferramari through XWayland.
- One 1920×1080 display. Multi-monitor behaviour is covered by unit tests and
  the manual checklist, not by a live two/three-display measurement.
- `cargo 1.99.1`, Rust stable 1.88+ toolchain pinned by `rust-toolchain.toml`.
- Baseline before changes: 81 unit tests, `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings` clean.

## 1. Current architecture

One process, one GTK 3 main loop and one command channel:

```
GUI / tray / D-Bus / X11 watcher / lock/suspend/idle watchers
                       │ Command
                       ▼
                  ┌──────────┐
                  │   App    │  GTK main thread, owns widgets and config
                  └────┬─────┘
        ┌──────────────┼───────────────┬────────────────┐
        ▼              ▼               ▼                ▼
 WallpaperWindow   Config/D-Bus   GNOME background   Library/catalogue
 (mpv / WebKit)    snapshot       (ffmpeg+gsettings) (JSON + cache)
```

| Area | Files | Notes |
| --- | --- | --- |
| Application state | `src/app.rs` | commands, windows, config, rotation, supervision |
| Config | `src/config.rs` | schema v4, atomic save, backup recovery, migration |
| Video player | `src/mpv.rs` | mpv child + JSON IPC over a Unix socket |
| Wallpaper windows | `src/ui/wallpaper.rs` | one X11 `DESKTOP` toplevel per monitor |
| Monitors | `src/monitors.rs` | GDK geometry + XRandR connector names |
| Window detection | `src/x11.rs` | EWMH polling (maximised/fullscreen) |
| Desktop integration | `src/handlers.rs` | screen lock, logout, suspend, idle |
| Library | `src/library.rs` | favourites, history, collections in JSON |
| Catalogue | `src/catalogue.rs`, `src/ui/online.rs` | third-party winzoro.net scrape |
| Control UI | `src/ui/control.rs` | Home, Local Video, catalogue, streaming, web, settings |
| Theme | `src/ui/theme.rs`, `data/ui/ferramari.css` | Liquid Glass dark/light tokens |
| D-Bus | `src/dbus.rs` | server and player interfaces, single instance |
| Tray | `src/tray.rs` | StatusNotifierItem through `ksni` |
| External tools | `src/ytdlp.rs`, `src/thumbnails.rs`, `src/wallpaper.rs` | yt-dlp, ffmpeg, ffprobe |
| Wakeups | `src/wake.rs` | pipe-based event-driven command loop |

The central invariant is sound: GTK objects never leave the main thread, and
background work sends `Command` values back through one channel. The weak
points are the places where that rule is violated (D-Bus setup, mpv IPC) and
the places where a decision is global although the spec requires it per
monitor (pause/mute).

## 2. Existing capabilities

- Local video wallpaper, one source per monitor, per-monitor video switching
  without restarting the other decoders.
- Streaming (`yt-dlp` resolution) and web-page wallpaper (WebKitGTK 4.1).
- Multi-monitor discovery, hotplug, reconnect and per-monitor source keys.
- Screen lock, suspend/resume, logout and end-session handling.
- Maximise/fullscreen detection on X11 and pause/mute of all wallpapers.
- Static GNOME wallpaper from a blurred video frame, cached and generated off
  the GTK thread.
- Power profiles (stored as `performance`, `balanced`, `power`), optional pause
  on battery.
- Library favourites, history, collections, rotation; catalogue with search,
  categories, sorting, resolution filters, download, preview sheet and install.
- Autostart, tray, D-Bus API, single instance, English/Russian/Japanese UI,
  light/dark/system theme.
- mpv crash supervision with capped exponential backoff and a give-up limit.
- Event-driven command loop; thumbnails on two workers with cancellation;
  config writes debounced.

## 3. Findings

### 3.1 Correctness and stability

| ID | Finding | Status |
| --- | --- | --- |
| B1 | Pause/mute decisions were global (`is_any_maximized`, `is_any_fullscreen`, `should_play`). A fullscreen game on one monitor paused every monitor. | **fixed** — per-monitor `eco::plan` + X11 rectangles |
| B2 | `Mpv::spawn` waited up to 3 s for the IPC socket on the GTK thread; every volume/pause command performed a synchronous IPC round-trip with a 2 s timeout on the GTK thread. A wedged mpv froze the UI. | **fixed** — dedicated IPC worker; spawn returns immediately |
| B3 | `start_sleep_watcher` and `start_end_session_watcher` created system/session-bus connections and registered the GNOME client on the GTK thread. A missing bus delayed startup. | **fixed** — all setup moved to worker threads |
| B4 | `Command::MonitorsChanged` called `Config::load` while a debounced volume/blur write was pending, silently reverting the edit. | **fixed** — flush before reload |
| B5 | The X11 watcher used `query(...).unwrap_or_default()`; a transient X error looked like “no maximised window” and resumed playback behind a fullscreen game. | **fixed** — keep the previous sample on error |
| B6 | `Command::Webpage` sent `file://` URLs through the network reachability check, so a local HTML document reported a network error. | **fixed** — local path handling |
| B7 | A single badly typed JSON field (e.g. `"audio_volume": "loud"`) failed the whole `Config` deserialisation and reset every setting. | **fixed** — per-field type repair |
| B8 | `power::status()` was read twice during construction. | **fixed** — one sample |
| B9 | Web wallpapers ignored lock/suspend/occlusion in `pause_fade`; their audio kept playing. | **partially fixed** — muted on pause; WebKitGTK has no public freeze API, JS still runs |
| B10 | `Mpv::Drop` can still block the GTK thread for up to the IPC timeout while joining the worker. `loadfile` errors are now surfaced as a user-visible playback error. | partly fixed |
| B11 | Monitor add/remove/size events are not debounced; a layout change can rebuild all windows several times. | **fixed** — 250 ms coalescing |
| B12 | HiDPI: X11 window rectangles are device pixels while GDK monitor geometry is logical pixels. Occlusion needs the scale factor. | **fixed** — `MonitorInfo::scale_factor` |
| B13 | Native Wayland has no wallpaper surface in the current implementation; the X11 watcher is disabled on Wayland sessions, so occlusion/smart pause is unavailable there. | open, documented |
| B14 | WebKit can crash independently; `player_alive` always returns true for a WebView, and only load failures are reported. | open |
| B15 | `Mpv::spawn` with `--hwdec=auto-safe` assumes mpv picks a working hardware decoder; there is no VA-API probe and no fallback report. | manual override added (Auto/VA-API/Off); probe still open |

### 3.2 Performance and resource use

| ID | Finding | Status |
| --- | --- | --- |
| P1 | The command loop is event-driven; player supervision (1 s), rotation (5 s) and power (30 s) timers remain. | partly open |
| P2 | The X11 watcher polls every 300 ms. | **fixed** — `PropertyNotify`/`StructureNotify` watcher with no idle timer |
| P3 | `HomePage::update` runs `util::video_paths()` on every `refresh_ui`, including every volume-slider tick. That spawns `xdg-user-dir`, reads the video directory and runs a GIO content-type query per file. | partly fixed — `xdg-user-dir` cached, folder scan reused for 1 s |
| P4 | All library cards are created for the whole folder; thumbnails are lazy and pooled, but there is no virtualisation for very large libraries. | open |
| P5 | Static/thumbnail `.part` files can survive a process crash. Catalogue partials are cleaned after one hour; static/thumbnail partials are not. | open, low impact |
| P6 | `reload_config` deliberately re-reads from disk and closes/recreates windows; a D-Bus reload during a pending edit discards the edit. | open by design |
| P7 | WebKit views are never frozen (no API in the binding); memory and JS timers remain while a page is hidden. | open |
| P8 | No automated resource benchmark exists. | **fixed** — `scripts/benchmark.py`, `docs/BENCHMARKS.md`; single-display results recorded |

### 3.3 Memory / lifetime

- `WallpaperWindow` owns its GTK window, mpv/WebView and menu; `Drop` closes
  the window and cancels the fade. No unbounded per-window growth was found.
- `Library` history is capped at 50. Static frames are capped at 12. Catalogue
  video cache is capped at 2 GB and pages/previews expire. Thumbnails are
  content-addressed and not capped in count (only by number of distinct
  videos), which is acceptable for a local library but worth documenting.
- `Mpv` sockets are per-process/counter and removed on drop; a hard crash can
  leave a stale socket in the cache directory.
- D-Bus watcher threads live for the process lifetime. They exit with the
  process; no join is attempted, which is acceptable for a desktop app.
- WebView contexts are per-window default contexts; no explicit context reuse
  or cap.

### 3.4 Architectural limitations relative to the target platform

- **Wayland**: the supported path is X11/XWayland (`GDK_BACKEND=x11`). There is
  no `wlr-layer-shell`, GNOME Shell extension or KDE plugin integration. The
  README does not claim native Wayland, and that stays true.
- **Per-monitor content**: video is per monitor, but streaming and web
  wallpapers are global default sources. Independent web/stream per monitor is
  not implemented.
- **Eco Engine**: the state model, per-monitor occlusion, idle detection and
  profile semantics are now present. Dynamic FPS reduction, decoder release on
  long pause and a measured performance page remain open.
- **`.ferramari` format**: not implemented. There is no package reader,
  validator, metadata editor, import/export or asset isolation.
- **Creator / Plugin API**: not implemented. The catalogue is a third-party
  scrape; there is no first-party catalogue API or server.
- **Desktop environments**: only GNOME/XWayland is tested. KDE, XFCE and
  Hyprland are untested; XFCE/KDE have partial EWMH compatibility by design.
- **Resource UI**: Settings exposes profiles and battery state, but there is no
  live CPU/RAM/GPU monitor and no dedicated Performance page.

### 3.5 Security and untrusted content

- Web wallpapers load arbitrary local/remote pages with JavaScript enabled.
  Explicit WebKit settings, denied permissions, no pop-ups and an optional
  network switch were added; per-wallpaper resource isolation and a full
  subresource network filter are still missing. WebKitGTK's own sandbox remains
  the main boundary.
- Catalogue and streaming URLs are handled as remote input with size limits
  and timeouts; `yt-dlp` gets `--` before the URL.
- `file://` paths are now accepted only for local authorities (`localhost` or
  empty); remote `file://host/...` is rejected.
- The D-Bus API can change wallpaper sources; it is a session-bus API and has
  no additional authentication, consistent with other desktop integrations.

## 4. Baseline test and measurement evidence

Baseline commands (before this change):

```sh
cargo fmt --check                                      # clean
cargo clippy --all-targets -- -D warnings              # clean
cargo test --all-targets                               # 81 passed
```

After the first stabilisation step:

```sh
cargo fmt --check                                      # clean
cargo clippy --all-targets -- -D warnings              # clean
cargo test --all-targets                               # 107 passed
```

Live smoke checks on the single 1080p XWayland display, with an isolated
config/cache and a private D-Bus session:

- Background start with no wallpaper: clean start and SIGTERM shutdown.
- Local video: mpv spawned, IPC connected ~77 ms after spawn, clean quit.
- Fullscreen mpv on the only monitor: watcher reported one fullscreen window,
  Eco state changed to `Occluded` and playback paused; closing it restored
  `Active` and playback.
- No leftover mpv child from the tested instance after quit.

No multi-monitor, native Wayland, KDE/XFCE or WebView-resource measurements
were made. Do not extrapolate the single-display numbers to those scenarios.

## 5. Recommended next steps

1. **Phase 1 remainder** — debounce monitor hotplug; observe `loadfile` results
   and surface a user error; make `Mpv::Drop` non-blocking (or bounded off the
   GTK thread); cache the video directory listing used by the Home page.
2. **Phase 2** — add dynamic FPS reduction for `Idle` (Balanced/Eco), optional
   decoder release on long pause, a visible per-monitor Eco state in Home, and
   a measured before/after energy/CPU table.
3. **Phase 3** — event-driven X11 watcher; VA-API probe/override; WebKit
   security settings and a page-suspension strategy; resource page.
4. **Phase 4** — investigate `wlr-layer-shell`, GNOME and KDE integrations as
   separate backends behind the same `DesktopBackend` seam. Do not claim
   native Wayland until a compositor-specific path is implemented and tested.
5. **Phase 5** — `.ferramari` package reader/validator/importer/exporter with
   path traversal checks, size limits and manifest versioning.
6. **Phase 6** — plugin API and online catalogue API only after the local
   engine and format are stable.