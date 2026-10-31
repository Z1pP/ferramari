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