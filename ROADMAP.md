# Roadmap

Ferramari 1.0 is a Linux/X11 (including XWayland) live-wallpaper
application. Reliability and predictable state come before new surface area.

## 1.0

- [x] Local video, streaming and web-page wallpapers
- [x] Per-monitor sources and hotplug handling
- [x] Pause/mute on maximized/fullscreen windows, lock and suspend
- [x] Automatic player recovery and crash-safe configuration
- [x] Catalogue with search, categories, sorting, resolution filters and preview
- [x] Local library with favorites, recent history and collections
- [x] Per-monitor collection rotation (order/random)
- [x] Power profiles and optional battery pause
- [x] English, Russian and Japanese interface
- [x] CI, release workflow, AppStream metadata and testing matrix
- [x] Self-hosted Flatpak manifest, built and smoke-tested
- [x] Performance baseline and known limitations documented

Known limitations are listed in `README.md` and `docs/TESTING.md`: native
Wayland is unsupported, GNOME X11/KDE rows are not yet tested, and the Flatpak
bundle uses host media tools.

## After 1.0

### 1.7 — Ecosystem foundation (current)

- [x] Declarative catalogue-source plugin manifests
- [x] First-party Catalog API v1 client and `--catalog` inspection command
- [x] Plugin/API documentation and validation tests
- [ ] Executable renderer plugin protocol (process-isolated)
- [ ] Hosted catalogue, author profiles, publishing and sync
- [ ] Plugin installer in the UI

### 1.6 — Creator Engine foundation

- [x] `.ferramari` manifest format and strict validation
- [x] Import/export from the Local Video page
- [x] Generated preview frame on export
- [x] Path traversal, symlink, file-count and 2 GiB limits
- [ ] Standalone package manager and metadata editor
- [ ] Web-package asset isolation beyond the WebKit policy

### 1.2 — Eco Engine foundation

- [x] Per-monitor Active/Idle/Occluded/Suspended policy
- [x] Quality/Balanced/Eco profiles and idle detection
- [x] Asynchronous mpv IPC worker (no GTK-thread waits)
- [x] Non-blocking session/suspend watchers
- [x] Config hotplug and per-field corruption safety
- [x] HiDPI-aware per-monitor occlusion
- [x] Configurable idle delay and per-monitor state display
- [x] Dynamic 30 fps cap while Balanced is idle
- [x] Event-driven X11 window watcher
- [x] Live Performance page and benchmark harness
- [x] Hardware decoding selector and WebKit hardening
- [ ] Decoder resource release after a long pause
- [ ] Per-monitor stream/web sources
- [ ] 4K / multi-monitor / Web-wallpaper benchmarks

### 1.1 — distribution and desktop coverage

- Flathub submission with bundled `mpv`/`ffmpeg`/`yt-dlp` and vendored Cargo
  sources.
- Test and document GNOME X11, KDE Plasma X11 and KDE Plasma Wayland.
- Verify tray and portal autostart inside the Flatpak sandbox.
- Package metadata screenshots and a short demo recording.
- Debian/RPM packaging if there is demand.

### 1.2 — power and automation

- Suspend/resume and lid/power events on non-GNOME sessions.
- Per-monitor rotation intervals and richer collection schedules.
- A profile editor with measured defaults instead of fixed profiles.
- Battery-aware static-wallpaper fallback.

### Later — native Wayland

Native Wayland is deliberately not on the 1.0 roadmap. It needs a compositor
integration strategy:

- `wlr-layer-shell` for wlroots compositors;
- a GNOME Shell extension or portal-based surface for GNOME;
- a KDE Plasma plugin or equivalent;
- a different embedding path for mpv/WebKit on each surface.

Until those exist and are tested, XWayland remains the supported path.

### Out of scope

- Cloning Wallpaper Engine's scene format or its proprietary renderer.
- A general-purpose desktop environment or compositor.
- Bundling copyrighted wallpapers; the app only handles content the user
  chooses.