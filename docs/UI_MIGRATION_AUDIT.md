# Ferramari UI Migration Audit — Phase 0

> Generated: 2026-11-05  
> Target repository: `hidamari-rust` (ferramari v1.9.4)  
> GTK line: gtk = "0.18", gdk-pixbuf, gdkx11, gio, glib, webkit2gtk  

---

## 1. Current Architecture Overview

Single-crate Rust application (`ferramari`), no workspace split. All modules under `src/`.

### Module Map (32 files, ~20 252 lines)

| File | Lines | Role |
|------|-------|------|
| `app.rs` | 2 336 | Application state + command loop |
| `ui/control.rs` | 4 358 | Control window (sidebar + pages) |
| `ui/online.rs` | 1 536 | Online catalogue browsing/downloading |
| `ui/wallpaper.rs` | 712 | Wallpaper surface windows per monitor |
| `ui/theme.rs` | 461 | Liquid Glass design system |
| `ui/menu.rs` | 27 | Context menu on wallpaper surfaces |
| `main.rs` | 318 | Entry point; GTK Application lifecycle |
| `config.rs` | 811 | JSON config loader/save/migration |
| `mpv.rs` | 852 | mpv subprocess controller |
| `wallpaper.rs` | 656 | GNOME background setter for static wallpapers |
| `library.rs` | 339 | Local video library CRUD, favorites, collections |
| `catalogue.rs` | 1 340 | Online catalogue client, download manager |
| `monitors.rs` | 233 | Monitor enumeration via GDK |
| `eco.rs` | 318 | Eco Engine profiles |
| `supervisor.rs` | 137 | Player watchdog / restart policy |
| `x11.rs` | 457 | X11 EWMH occlusion watcher |
| `dbus.rs` | 345 | D-Bus single-instance + remote API |
| `tray.rs` | 88 | System tray via ksni |
| `power.rs` | 101 | Battery status probing |
| `fade.rs` | 149 | Volume fade animation |
| `thumbnails.rs` | 81 | Thumbnail generation from video frames |
| `paths.rs` | 257 | Directory layout constants |
| `util.rs` | 415 | Helpers including i18n (`tr()`) |
| `handlers.rs` | 324 | Signal handlers (screensaver, sleep, logind, idle) |
| `wake.rs` | 65 | Pipe-based wakeup for GTK main loop |
| `package.rs` | 893 | `.ferramari` package format import/export/signing |
| `signing.rs` | 739 | Ed25519 package signing and verification |
| `ytdlp.rs` | 539 | yt-dlp wrapper for stream resolution |
| `cli.rs` | 82 | CLI argument parsing |
| `error.rs` | 112 | Error types and user-visible messages |
| `autostart.rs` | 115 | Autostart entry management |

### Top-Level Flow

```
main.rs (GTK Application::new)
  ├── gtk_app.connect_startup → App::new()
  │       └── subsystems init, background threads spawn
  │
  ├── gtk_app.connect_activate → App::activate()
  │       └── rebuild_windows(), show_gui()
  │
  ├── Command channel (mpsc + pipe wake)
  │       └── drain_commands() every ~40 ms or on pipe read
  │
  ├── D-Bus single-instance guard + remote API
  ├── System tray (ksni)
  ├── External watchers (idle, logind, screensaver, sleep, EWMH, monitor hotplug)
  └── Timers (player watch 1 s, rotation 5 s, power 30 s)
```

### Key Architectural Patterns

1. **Command-driven serialisation** — every callback sends a `Command` into one `mpsc` channel. The GTK main thread drains them and mutates `App` atomically.
2. **Background workers** — network ops, thumbnail generation, ytdlp resolution spawn threads; results come back through the same channel as response Commands.
3. **One WallpaperWindow per monitor** — X11 toplevel with `type_hint=Desktop`, embedding either an mpv instance or a WebKitGTK WebView.
4. **ControlWindow** — regular decorated `ApplicationWindow`, built entirely in code (no GtkBuilder XML). Sidebar navigation + `Stack` page content.
5. **I18n** — gettext via `gettext-rs` + GNU gettext system backend. PO files in `po/` (`ja.po`, `ru.po`). Helper function `tr()` used throughout UI code.

---

## 2. Full GTK/GDK Dependency List

### Cargo Dependencies

```toml
gtk = "0.18"           # GTK 3 bindings
gdk-pixbuf = "0.18"    # Pixbuf image handling (thumbnails)
gdkx11 = "0.18"        # GDK X11 integration (XID extraction)
gio = "0.18"             # GIO abstractions (ApplicationHoldGuard, GSourceId)
glib = "0.18"            # GLib event loop, timeout sources, signals
webkit2gtk = { version = "2.0", features = ["v2_40"] }  # Webpage wallpapers
```

### Native Libraries at Runtime

- `libgtk-3.so` (GTK 3 toolkit)
- `libcairo.so` (Cairo graphics + backdrop painting)
- `libpango-1.0.so` (text rendering)
- `libgdk-3.so` (GDK windowing abstraction)
- `libgio-2.0.so`, `libglib-2.0.so`, `libgobject-2.0.so`
- `libgdk_pixbuf-2.0.so` (Pixbuf image loading)
- `libwebkit2gtk-4.0.so` (HTML engine for webpage wallpapers)
- `libatk-1.0.so` (ATK accessibility bridge)
- `libx11.so` (via `x11rb` crate — X11 window ID, stacking, occlusion)
- `libdbus-1.so` (via `zbus` — D-Bus single instance)

### Files Using GTK/GDK Directly

| File | GTK Constructs Used |
|------|---------------------|
| `main.rs` | `gtk::Application`, `glib::unix_signal_add`, `glib::timeout_add_local` |
| `app.rs` | `gtk::Application`, `glib::SourceId`, `glib::IOCondition`, `gdk::Display`, `gdk::Screen` |
| `ui/control.rs` | Every widget type: `ApplicationWindow`, `HeaderBar`, `Box`, `ScrolledWindow`, `Stack`, `FlowBox`, `Label`, `Button`, `Entry`, `ComboBoxText`, `Switch`, `Scale`, `MenuButton`, `DrawingArea`, `Pixbuf`, `CssProvider`, `StyleContext`, `atk::AtkObjectExt`, pango |
| `ui/online.rs` | Same pattern: `Box`, `FlowBox`, `Spinner`, `ScrolledWindow`, `Pixbuf`, `glib::timeout_add_local` |
| `ui/wallpaper.rs` | `ApplicationWindow`, `gdk::WindowTypeHint`, `gdkx11::X11Window`, `EventMask` |
| `ui/theme.rs` | `ApplicationWindow`, `CssProvider`, `StyleContext`, `Settings`, `DrawingArea`, `cairo`, `gdk::RGBA` |
| `ui/menu.rs` | `Menu`, `MenuItem`, `EventMask` |
| `monitors.rs` | `gtk::gdk::Monitor` for monitor enumeration |
| `wallpaper.rs` | `gio::Settings` for GNOME background setter |
| `util.rs` | `gio::File` helper functions |

### Non-GTK Modules with Zero GTK Dependency (~12 000 lines)

`metrics`, `supervisor`, `catalogue`, `catalog_api`, `plugin`, `library`, `package`, `ytdlp`, `desktop`, `power`, `wake`, `handlers`, `dbus`, `tray`, `autostart`, `x11`, `mpv`, `signing`, `config`, `error`, `cli`, `eco`, `fade`, `paths`

---

## 3. Existing UI Functions Inventory

### Control Window (`ui/control.rs`)

#### Header (~line 350–500)
- Brand mark (icon + title/subtitle labels)
- Play/Pause button
- Mute/unmute toggle
- Status chip (current mode indicator with colored dot)
- Settings gear button
- Volume popover with scale slider and toggle

#### Sidebar Navigation (~line 500–650)
- Home section: monitors with live Eco states, current wallpaper info, quick actions
- Library link
- Online Catalogue link
- Performance Monitoring link
- Settings link
- Collapsible on narrow widths (< 900 px), promotes nav to top rail
- Responsive thresholds: narrow < 900, status visible >= 1020, subtitle visible >= 800

#### Pages

**Home / Dashboard:**
- Connected monitors display with live Eco Engine states
- Current wallpaper source, mode, and monitor assignment
- Recently added / last played list
- Static wallpaper toggle
- Blur radius spinbutton

**Library (Video Page):**
- Grid view (`FlowBox` cards) and List view toggle
- Search input (filter by filename)
- Collections management: new, rename, delete, add/remove
- Favorites / Recent views
- Import / Export / Sign / Verify package buttons
- Apply-to-monitor action per card
- Thumbnail lazy-loading from video frames via `thumbnails::generate()`
- Empty state messaging
- Selection summary label

**Online Catalogue (`ui/online.rs`):**
- Category filter dropdown
- Search input
- Sort dropdown (default/title/category/size/duration)
- Resolution filter (all/4K/1440p/1080p/HD)
- Paginated `FlowBox` card grid
- Download progress tracking per item
- Preview dialog (full-size image, metadata, author info)
- Loading spinner + empty/error states
- Retry button for failed loads

**Performance Page:**
- CPU usage percentage
- RAM usage (MiB / total MiB)
- GPU memory usage (if available)
- Thread count, active process count, decoder count
- FPS statistics
- Per-monitor Eco states summary
- Live updating

**Settings Page:**
- General: theme selector, language selector, start on login, systray toggle
- Appearance: rotation mode, rotation interval, playback profile, idle timeout, hardware decoding, pause on battery, web network allow
- Playback: fade duration, volume, mute, blur radius, static wallpaper, pause/maximized, mute/maximized
- Monitors summary, signing summary, battery status

**Stream Page:**
- URL entry field
- Resolve/stream button
- Progress indicator, cancel button

**Web Page:**
- URL entry field

### Dialog System
- `MessageDialog` for errors and confirmations
- `AboutDialog` for about screen
- `FileChooserDialog` (Open, Save, SelectFolder) for package/import operations
- Toast/info banners near header (`show_info`)
- Custom dialog for selecting monitor/application target

### System Integration UI
- Tray icon (`ksni`): Show, Toggle Mute, Toggle Play/Pause, Quit
- Context menu on wallpaper surfaces: Show, Toggle Mute, Toggle Play/Pause, Reload, Feeling Lucky, Quit
- D-Bus remote control interface (`org.freedesktop.portal.Ferramari`)

---

## 4. UI ↔ Core Connections

### Direction 1: UI → Core

Every UI callback sends a `Command` into one `mpsc` channel. The GTK main thread drains them via `drain_commands()` and mutates `App` serially in `App::handle(Command)`.

### Full `Command` Enum (54 variants)

```rust
pub enum Command {
    ShowGui, Null,
    Video { path: Option<String>, monitor: Option<String> },
    Stream { url: String },
    StreamReady { url: String, result: Result<ytdlp::StreamInfo, String> },
    Webpage { url: String },
    WebpageReady { url: String, result: Result<(), String> },
    PausePlayback, StartPlayback, Reload, QuitPlayer,
    FeelingLucky, Quit,
    ToggleMute, TogglePause,
    SetVolume(i64), SetMute(bool), SetPausedByUser(bool),
    SetBlurRadius(i64), SetStaticWallpaper(bool),
    SetPauseWhenMaximized(bool), SetMuteWhenMaximized(bool),
    ScreenLockActive(bool), SessionLock(bool),
    WindowState(WindowState), Idle(bool),
    MonitorsChanged, MonitorsChangedNow,
    SetAutostart(bool), SetLanguage(String), SetTheme(String),
    SetRotationMode(String), SetRotationInterval(i64),
    SetMonitorRotation { monitor: String, collection: Option<String> },
    SetPlaybackProfile(String), SetPauseOnBattery(bool),
    SetIdleTimeout(i64), SetHardwareDecoding(String),
    SetWebNetwork(bool),
    PowerStatus { on_battery: bool, percent: Option<u8> },
    Suspend(bool),
    LibraryChanged,
    ImportPackage(PathBuf), PackageImported(Result<Box<ImportedPackage>, String>),
    ExportPackage { video: PathBuf, parent: PathBuf },
    PackageExported(Result<PathBuf, String>),
    SignPackage(PathBuf), PackageSigned(Result<PathBuf, String>),
    VerifyPackage(PathBuf), PackageVerified(Box<PackageReport>),
    TrustKey(PathBuf),
    WebError { monitor: String, message: String },
    StaticWallpaperReady { source: String, blur: i64, generation: u64, file: Option<PathBuf> },
}
```

### Direction 2: Core → UI

Core calls specific methods on `ControlWindow`:

| Method | Trigger | What Changes |
|--------|---------|--------------|
| `refresh(RefreshState)` | After each batch of commands | Sidebar states, settings widgets, performance data |
| `refresh_sidebar_state()` | Mode change, playback change | Status chip text/color |
| `refresh_library_grid()` | Collection/favorites change | Rebuilds FlowBox cards |
| `reload_library()` | After package import | Refreshes local grid from library |
| `set_streaming(bool)` | Stream resolve started/completed | UI state indicator |
| `show_info(&str)` | Success notification | Toast banner |
| `show_error(UserError)` | Any error | Error dialog |
| `show_package_report(PackageReport)` | Package verification | Panel with integrity info |

### Shared State

| Object | Type | Purpose |
|--------|------|---------|
| `snapshot` | `Arc<Mutex<Snapshot>>` | Read-only state copy, exposed over D-Bus and consumed by UI refresh |
| `library` | `Rc<RefCell<Library>>` | Local video library, shared between `App` and `ControlWindow` |
| `tx` | `CommandSender` (cloneable) | Shared across UI, tray, D-Bus, external watchers |

### Triggers That Cause Full UI Refresh

- Mode changes (video/stream/web/null)
- Monitor hotplug (debounced 250 ms)
- Package import completion
- Library changed events
- Power status changes
- Screen lock / session lock state changes
- Window state (occlusion) changes
- Eco Engine plan transitions
- Configuration reload from disk

---

## 5. UI ↔ Platform / Media Connections

| UI Module | Depends On | Usage |
|-----------|-----------|-------|
| `ui/wallpaper.rs` | `mpv` | `Mpv::spawn()`, `load_video()`, volume/pause/mute/fps_cap control |
| `ui/wallpaper.rs` | `monitors` | `MonitorInfo` for positioning/resizing |
| `ui/wallpaper.rs` | `x11rb` | Getting X11 window IDs for desktop surfaces |
| `ui/wallpaper.rs` | `fade` | `Fade` animation utility |
| `ui/wallpaper.rs` | `ytdlp` | `StreamInfo` type definition |
| `ui/wallpaper.rs` | `gtk/gdkx11` | `X11Window::xid()` for embedding mpv |
| `ui/wallpaper.rs` | `webkit2gtk` | `WebView` creation for web wallpaper surfaces |
| `ui/control.rs` | `thumbnails` | `generate()` for video frame thumbnails |
| `ui/control.rs` | `library` | `Library` type — reading lists, favorites, collections |
| `ui/control.rs` | `package` | Package import/export/sign/verify |
| `ui/control.rs` | `eco` | `eco::State`, `eco::Plan` types for Eco Engine display |
| `ui/control.rs` | `config` | `Config` and `Mode` types for settings binding |
| `ui/online.rs` | `catalogue` | `Catalogue`, `Wallpaper`, `Category`, `DownloadInfo` types |
| `ui/wallpaper.rs` | `app` | `CommandSender`, `Command` types for sending state changes |

### Important Boundary Observations

1. **`ui/wallpaper.rs` manages platform-level surfaces** — X11 window IDs, mpv spawning, WebKitGTK creation, desktop icon layer ordering. This entire module should move to `ferramari-platform` or `ferramari-media` if we ever split crates. It is currently in `ui` for convenience but conceptually belongs elsewhere.

2. **No abstraction layer** — the wallpaper surface directly calls `x11rb::connect`, `Mpv::spawn()`, `webkit2gtk::WebView::builder()`. Fine for single-crate, will complicate migration if these details leak into the new Slint UI layer.

3. **Thumbnail pipeline** (`thumbnails.rs`) — invokes ffmpeg (child process) to extract frames at specific timestamps. Output is `gdk_pixbuf::Pixbuf` on the GTK thread.

---

## 6. System Integrations List

| Integration | Module | Technology | GTK Dependency? |
|-------------|--------|------------|-----------------|
| Single Instance Guard | `dbus.rs` | zbus (D-Bus activation) | No |
| Remote API (D-Bus) | `dbus.rs` | zbus D-Bus interfaces | No |
| System Tray | `tray.rs` | ksni (native tray abstraction) | No |
| Auto-start | `autostart.rs` | Desktop entry file + xdg | No |
| Monitor Detection | `monitors.rs` | `gtk::gdk::Display` APIs | Yes (enumeration only) |
| Screen Lock Watcher | `handlers.rs` | ZBus + glib signals | Partial (glib, no GTK widgets) |
| Logind Session Lock | `handlers.rs` | zbus systemd login interface | No |
| Sleep/Suspend Watcher | `handlers.rs` | systemd logind D-Bus | No |
| Desktop Idle Detection | `handlers.rs` | glib timeout (polls libgnome-desktop) | glib only |
| EWMH Window State Watcher | `x11.rs` | x11rb root window properties | No |
| Monitor Hotplug Events | `app.rs` | `gdk::Display` connect_monitor_added/removed | Yes |
| Battery Detection | `power.rs` | sysfs/power-supply Linux | No |
| GNOME Background Setter | `wallpaper.rs` | `gio::Settings` GNOME schema | Yes (static fallback only) |
| Portal Integration | `desktop.rs` | xdg-desktop-portal D-Bus | No |
| Font Configuration | `theme.rs` | Pango font scanning | Yes (Pango/GTK) |
| System Color Scheme | `theme.rs` | `gtk::Settings` dark preference | Yes |
| Accessibility | `control.rs` | AtkObjectExt | Yes (ATK is part of GTK) |
| File Chooser Dialogs | `control.rs` | `FileChooserDialog` | Yes (3 locations) |

### GTK-Tightly-Coupled (Must Be Careful With Removal)

1. **Monitor detection** (`monitors.rs`) — uses `gtk::gdk::Display` for enumeration. Replaceable via X11/XRandR directly through `x11rb`.
2. **GNOME Background setter** (`wallpaper::GnomeBackground`) — captures/restores GNOME's background via `gio::Settings`. Only used for static wallpaper fallback.
3. **Screen size / color scheme** — `gtk::gdk::Screen::default()` used in `app.rs` for DPI queries.
4. **File dialogs** — `FileChooserDialog` and `MessageDialog` blocks the calling thread via `.run()`. Will need rework for Slint's async model.

---

## 7. Proposed Slint UI Structure

Based on the spec document's structure, adapted to actual Ferramari modules:

```
data/
  ui/
    ferramari.slint              # App shell (window, global theme vars)
    theme.slint                  # Semantic colors, spacing, typography tokens
    components/
      sidebar.slint              # Left navigation rail
      headerbar.slint            # Title bar with brand, status, actions
      wallpaper_card.slint       # Reusable video/catalogue card
      monitor_card.slint         # Monitor tile with Eco state badge
      status_chip.slint          # Mode/status indicator pill
      progress_bar.slint         # Download/install progress
      dialog.slint               # Modal/dialog base component
      notification.slint         # Toast/info banner
      search_field.slint         # Search input with icon
      select_dropdown.slint      # Combo/select element
      switch_toggle.slint        # On/off toggle
      slider_control.slint       # Volume/interval/brightness slider
    pages/
      dashboard.slint            # Home/overview
      library.slint              # Local video library
      online.slint               # Online catalogue
      performance.slint          # CPU/RAM/GPU/stats
      settings.slint             # Settings form
      preview.slint              # Wallpaper preview modal

[Optional later] crates/
  ferramari-ui/
    build.rs                     # slint-build invocation
    src/
      lib.rs                     # re-export Slint-generated types
      app.rs                     # Slint App struct, event wiring
      state.rs                   # UiState, sync with core App
      commands.rs                # CommandSender wrapper
      subscriptions.rs           # Event subscription layer
      image_cache.rs             # Thumbnail/image caching for Slint Image
      pages/                     # Per-page Rust glue code
```

This structure maps cleanly to the existing CSS token system in `theme.rs` and the page composition in `control.rs`'s `Stack`.

---

## 8. Proposed Slint ↔ Core Communication Mechanism

The existing `mpsc` channel + `App::handle()` pattern is already exactly what the spec proposes. **No redesign needed.**

### Direction 1: UI → Core (Commands) — unchanged

```rust
// Current GTK callback in control.rs:
self.tx.send(Command::SetVolume(volume)).ok();

// Slint equivalent (in Rust glue code, called from .slint callbacks):
impl AppView {
    pub fn set_volume(&self, volume: f64) {
        let _ = self.tx.send(Command::SetVolume(volume as i64));
    }
}
```

Slint callbacks call Rust accessor methods which push Commands into the channel. Same dispatch loop, same `App::handle(Command)` handler.

### Direction 2: Core → UI (State Sync) — two approaches

**A. Property binding on a global Slint context object.** Extend `Snapshot` to hold all values that the UI needs to display. On each tick or after each command, update this context:

```rust
// In the command drain loop:
ctx.sync_from(&app.borrow());
```

**B. Event-driven updates** — infrequent structural changes (monitor hotplug, library refresh) trigger targeted property updates instead of full-state sync.

**Hybrid (recommended):** use A for frequent/small changes (volume, play/pause state), B for rare structural changes (new monitors, library update).

### Critical Constraint

Background threads must never touch Slint objects directly. Always go through the `Command` channel — exactly as they do now.

---

## 9. Potential Migration Problems

### High Risk

1. **WebKitGTK for webpage wallpapers** — WebKitGTK is tightly coupled to GTK3. Options:
   - Keep `webkit2gtk` as a media-layer dependency only when web mode is selected (acceptable since webpage wallpapers are fundamentally a *rendering* task, not a UI task)
   - Replace WebView renderer entirely (impractical)
   - Accept that WebKitGTK stays — just not part of the UI layer

2. **X11 window embedding for mpv surfaces** — `gdkx11::X11Window::xid()` gets the X11 window ID where mpv renders. Slint Winit windows don't expose XIDs natively. Solution: move this functionality to `ferramari-platform`:
   ```
   New Slint UI → [abstract monitor/window handle] → ferramari-platform/media → X11/Wayland window creation → mpv
   ```

3. **Pixbuf → Slint Image conversion** — Thumbnail pipeline produces `gdk_pixbuf::Pixbuf`. Slint expects different formats (PNG bytes or `Image::from_rgba8()`). Need a conversion layer.

4. **Cairo backdrop painting** — Entire Liquid Glass aesthetic relies on custom Cairo drawing (4 gradient layers, radial glows, vignette). Slint has no Cairo painting primitive. Options:
   - Use Slint native gradient brushes + opacity layers (approximates the effect)
   - Pre-render backdrop textures and use as background images
   - Use a canvas approach within Slint

### Medium Risk

5. **Single-threaded constraint** — GTK and Slint both want the main thread. Compatible, but Tokio/background async work must not block. Current architecture already avoids this via the command channel.

6. **i18n integration** — gettext `tr()` returns `String` at runtime. Slint supports localization via `.ts` files. Need to check compatibility with Russian/Japanese text expansion.

7. **Dynamic catalog page loading** — Online catalogue loads pages lazily with pagination. Slint needs efficient virtualization or manual lazy-loading logic.

8. **Flatpak packaging** — Current manifest depends on GTK 3 SDK. Moving to Slint requires changing the build environment.

9. **File/Save/Folder dialogs** — Current implementation uses blocking `FileChooserDialog.run()`. Slint has basic dialog support but may not cover folder picker or all use cases. May need custom dialog implementation or platform-layer helpers.

### Low Risk

10. **System tray** — Already `ksni`, no GTK impact.
11. **D-Bus remote API** — Already `zbus`, no GTK impact.
12. **Configuration persistence** — Pure JSON serialization, safe.
13. **Eco Engine, Library, Packages** — All pure-Rust, safe.
14. **Monitors enumeration** — Can migrate to `x11rb` XRandR queries. Small refactor.

---

## 10. Safe Transition Plan

### Phase 0 — Audit (this document)
No production code changes.

### Phase 1 — Build Infrastructure
Add Slint compilation alongside existing GTK build. No runtime changes yet.

```
[New] data/ui/*.slint           # Declarative UI files
[Kept] src/ui/control.rs        # Old GTK still builds
[Kept] src/main.rs              # Still launches GTK
```

Verify the Slint toolchain works (slint-build, compiler, FemtoVG renderer).

### Phase 2 — Feature Flag / Parallel UI
Introduce a feature flag or environment variable to choose UI backend:

```
FERRAMARI_UI=gtk|slint
```

This allows direct comparison testing against the GTK version for performance metrics.

### Phase 3 — Core Interface Stabilization
Extract UI-facing parts of `App` behind traits before full migration:

```rust
/// What the UI layer needs from the core.
pub trait AppStateProvider {
    fn snapshot(&self) -> Snapshot;
    fn config(&self) -> &Config;
    fn monitors(&self) -> &[MonitorInfo];
    fn eco_states(&self) -> &HashMap<String, eco::State>;
    // ... other read-only accessors
}
```

### Phase 4 — Incremental Page Migration
Start with simple pages first:
1. **Settings** (forms, mostly isolated)
2. **Dashboard** (read-heavy, live state display)
3. **Library** (grid + complex interactions)
4. **Online Catalogue** (network + pagination)
5. **Performance** (read-only stats display)

Each page tested in isolation before moving to the next.

### Phase 5 — Deprecate GTK UI
Once all pages migrated and verified:

1. Remove `gtk::Application` from main entry point (or make optional)
2. Conditionally compile GTK deps:
   ```toml
   [dependencies.gtk]
   optional = true
   
   [features]
   gtk-ui = ["gtk", "gdk-pixbuf", "gdkx11", "gio", "glib", "webkit2gtk"]
   default = []   # empty after migration
   ```
3. Check remaining GTK usage: `monitors.rs` (enumerate), `wallpaper.rs` (GNOME BG), `util.rs` (gio::File). These are lightweight — replace individually, don't wait for bulk removal.

### Phase 6 — Cleanup
Remove GTK dependencies. Update Makefile, CI workflows (`.github/workflows/ci.yml`, `release.yml`), Flatpak manifests. Update documentation.

---

## Appendix A: GTK Dependency Footprint by Module

Modules with **zero** GTK dependency (safe to ignore during migration):
- `metrics`, `supervisor`, `catalogue`, `catalog_api`, `plugin`, `library`, `package`, `ytdlp`, `desktop`, `power`, `wake`, `handlers`, `dbus`, `tray`, `autostart`, `x11`, `mpv`, `signing`, `config`, `error`, `cli`, `eco`, `fade`, `paths`

Modules with **light** GTK dependency (minor refactors):
- `monitors.rs` — `gdk::Monitor` enumeration → replace with XRandR
- `wallpaper.rs` — `gio::Settings` GNOME BG getter/setter → replace with portal or remove
- `util.rs` — `gio::File` helpers → replace with `std::fs` / `Path`

Modules with **heavy** GTK dependency (must be rewritten):
- `ui/control.rs` (4 358 lines)
- `ui/online.rs` (1 536 lines)
- `ui/wallpaper.rs` (712 lines — move to platform/media)
- `ui/theme.rs` (461 lines)
- `main.rs` (318 lines)
- `app.rs` (2 336 lines — mostly glib, small GTK footprint)

Total GTK-bound code: **~9 700 lines** across 6 files.

---

## Appendix B: CSS Token Reference (ready for Slint mapping)

From `theme.rs`, `Palette::dark()` and `Palette::light()`:

```css
BG_BOTTOM         -- window background
HEADER_TOP/BOTTOM -- header gradient stops
TEXT              -- primary text color
TEXT_SECONDARY    -- secondary text
TEXT_MUTED        -- disabled/faded text
SURFACE           -- translucent card surface
SURFACE_STRONG    -- stronger surface (toolbars)
SURFACE_HOVER     -- hover state
SURFACE_ACTIVE    -- active/selected state
INPUT_BG          -- input field background
INPUT_BG_FOCUS    -- focused input border/highlight
BORDER            -- default border color
BORDER_STRONG     -- strong border accent
DIVIDER           -- separator lines
SHINE             -- highlight/reflection
SHINE_SOFT        -- subtle reflection
ACCENT            -- primary accent (#F3CB7C dark, #B87A1E light)
ACCENT_TEXT       -- text on accent elements
ACCENT_BG         -- accent button fill
ACCENT_BG2        -- accent variant
ACCENT_BG_HOVER   -- accent hover state
ACCENT_INK        -- dark text on bright accent bg
ACCENT_SOFT       -- soft accent tint
ACCENT_BORDER     -- accent border glow
ACCENT_SHADOW     -- accent shadow color
FOCUS             -- focus ring outline
FOCUS_SOFT        -- soft focus ring glow
DANGER            -- error/destructive color
DANGER_SOFT       -- soft danger tint
SHADOW            -- drop shadow (strong)
SHADOW_SOFT       -- drop shadow (soft)
POPOVER_BG        -- popover/dropdown background
TOOLTIP_BG        -- tooltip background
TOOLTIP_TEXT      -- tooltip foreground text
SCROLLBAR         -- scrollbar track color
```

Plus Cairo-specific paint parameters (may map to pre-rendered textures in Slint):
```
bg_top / bg_bottom       -- backdrop gradient stops
glow_warm / glow_cool / glow_rose / glow_mint -- radial glow spots at corners
vignette                 -- corner darkening overlay
sheen                    -- upper brightness overlay
```

---

## Appendix C: Cross-Platform Notes

### Linux X11
Current primary target. Most integrations (EWMH, XRandR, X11 window embedding) are X11-specific. Slint Winit backend fully supports X11.

### Linux Wayland
Currently partially supported via GTK. Slint Winit supports Wayland natively via `smithay-client-toolkit`. Wallpaper surface on Wayland needs `layer-shell` protocol instead of X11 desktop layer — separate `ferramari-platform` task.

### Windows 10/11
Not currently supported (code is Linux-only). Slint Winit supports Windows natively. Wallpaper surface needs completely different approach (desktop window management). Not in Phase 1 scope.

---

## Appendix D: Existing Tests Coverage

Tests exist for non-GTK modules:
- `config.rs` — 10+ tests covering schema migration, field validation, atomic save/backup recovery
- `ui/wallpaper.rs` — 4 tests for `WindowKey` equality, parking permission logic, liveness latch
- `signing.rs` — key management, signature verification
- `package.rs` — checksumming, imports, exports

No tests exist for any GTK widget code (`control.rs`, `online.rs`, `theme.rs`). Adding widget-level tests for Slint will be part of Phase 4+.
