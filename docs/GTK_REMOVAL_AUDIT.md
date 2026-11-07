# Ferramari GTK/GDK Removal Audit — current status

> **Status update:** 2026-11-07, commit `037606c` and follow-up Iced migration work.
> This status section supersedes the Phase 0 assumptions below where they
> conflict. The original report is retained as historical migration input.

## Current 2.0 workspace findings

- The workspace contains `ferramari-core`, `ferramari-platform`,
  `ferramari-media` and `ferramari-ui`.
- `ferramari-ui` uses Iced 0.14 and communicates with the wallpaper server
  through the session D-Bus. It is a separate executable.
- GTK/GDK and WebKitGTK have now been removed from the workspace manifests and
  lockfile. The dead `web-wallpaper` feature and its optional `webkit2gtk`
  dependency were deleted; 42 packages used only by that dependency chain
  were pruned from `Cargo.lock`. CI checks the all-features workspace graph
  for GTK/GDK-family dependencies.
- The old GTK WebKit renderer is gone. Web-page wallpapers are not currently
  supported; do not advertise this capability until a replacement renderer exists.
- The wallpaper surface implementation in `ferramari-media` is currently
  X11-only (`x11rb`, mpv `--wid`). This is not a cross-platform surface API.
- `ferramari-platform` contains Linux-specific modules (RandR, D-Bus,
  StatusNotifierItem, GNOME background and session handlers). It does not yet
  expose a complete backend trait/capability contract for Windows or macOS.
- The old Flatpak manifest and its GNOME/WebKitGTK runtime have not been
  migrated to the split server + Iced UI workspace.
- The CI/Makefile/release packaging must build and ship both
  `ferramari` and `ferramari-ui`; a root-package-only build is insufficient.

## Required completion checks

1. **Completed:** remove the unused `webkit2gtk` optional
   dependency and feature, regenerate `Cargo.lock`, and prune the 42 packages
   used only by the legacy GTK/WebKitGTK chain.
2. Confirm the default and all-feature dependency trees contain no GTK/GDK/
   WebKitGTK crates; the CI all-features dependency guard now enforces this.
3. Run `cargo build --workspace --release --locked`,
   `cargo test --workspace --locked`, and
   `cargo clippy --workspace --all-targets --locked -- -D warnings`.
4. Install both executables side by side and verify that the server can launch
   exactly one Iced control surface over D-Bus.
5. Run live X11/XWayland checks for mpv `--wid` embedding, desktop stacking,
   monitor hotplug, lock/unlock, tray activation and clean shutdown.
6. Restore remaining UI feature parity (package import/export/sign/verify,
   file pickers, monitor assignment and any settings still missing from the
   Iced surface). Favourites and collection CRUD/assignment are now present in
   the Iced library page; they still need live-session persistence testing.
7. Decide on a replacement web renderer before claiming web wallpaper support.
8. Migrate Flatpak packaging or explicitly mark it unsupported for 2.0.
9. Define the shared platform/surface contract before integrating macOS or
   Windows backends.

---

# Historical Phase 0 audit

> **Date:** 2026-11-05  
> **Repository:** `hidamari-rust` (ferramari v1.9.4)  
> **Platform:** Ubuntu 26.04 LTS (host has GTK runtime only, no dev packages)  
> **Build state:** Cannot compile here without GTK dev headers (confirmed broken in this environment)

---

## 1. Full GTK/GDK Dependency List

### Direct dependencies in Cargo.toml

| Crate | Version | Purpose | Retention Post-Migration |
|-------|---------|---------|--------------------------|
| `gtk` | 0.18 | Application window, all widgets, event loop glue, monitor enumeration via GDK, ApplicationHoldGuard for single-instance enforcement | **Remove entirely** — replaced by Iced + zbus |
| `gdk-pixbuf` | 0.18 | Thumbnail/Pixbuf rendering in online catalogue and control-window card images | Replace with `image` crate or ffmpeg RGBA export |
| `gdkx11` | 0.18 | Extract X11 Window ID from wallpaper surface to pass as mpv embed point (`X11Window::xid()`) | Use `x11rb` directly |
| `gio` | 0.18 | GNOME gsettings (`gio::Settings` for BG setter), GIO file helpers (`gio::File` URI/path conversion), timer source abstraction, ApplicationHoldGuard | Replace individually — see §3.2 below |
| `glib` | 0.18 | GLib timers (`timeout_add_local`, `unix_signal_add`), SHA-256 Checksum (used in signing/package), unix fd monitoring, SourceId tracking | Replace calls independently |
| `webkit2gtk` | 2.0 + feature `v2_40` | Webpage wallpapers — renders HTML/CSS/JS on desktop surfaces | Requires dedicated evaluation (§4); will be isolated behind a feature flag in early phases |

Confirmed by `cargo tree`: `gtk` depends on ferramari **directly**, AND on `webkit2gtk` which itself depends on ferramari directly. Both are direct edges. No other crate pulls GTK indirectly.

### Transitive dependencies (only exist because GTK pulls them in)

| Transitive dep | Pulled by | Used directly anywhere? | Action |
|---------------|-----------|------------------------|--------|
| `atk` | `gtk` | No | Remove after GTK removal |
| `pango` | `gtk` + used directly for pango::EllipsizeMode in UI code | Partially — but ellipsizing can be done manually with string truncation or built-in Iced support | Evaluate per-usage |
| `cairo-sys-rs`, `cairo` | `gtk` | Not used directly | Remove |
| `soup3` / `libsoup` | `webkit2gtk` | Not used directly | Remove with WebKitGTK |
| `javascriptcore-rs` | `webkit2gtk` | Not used directly | Remove with WebKitGTK |
| `gdkx11-sys` | `gdkx11` | Not used directly | Remove with `gdkx11` |
| `gtk-sys` | `gtk` | Not used directly | Remove |
| `gobject-sys` | `gtk` | Not used directly | Remove |

These have **zero direct use** outside the GTK transitive chain.

### Native runtime libraries linked into release binary

From `ldd target/release/ferramari`:

```
libgtk-3.so.0           ← application GUI framework
libgdk-3.so.0           ← windowing abstraction
libgdk_pixbuf-2.0.so.0  ← image loading/pixbufs
libglib-2.0.so.0        ← core utilities, event sources
libgio-2.0.so.0         ← file I/O, settings, source IDs
libgobject-2.0.so.0     ← object signal system
libcairo.so.2           ← drawing backend
libcairo-gobject.so.2   ← Cairo+GLib bindings
libpango-1.0.so.0       ← text shaping/rendering
libpangocairo-1.0.so.0  ← Pango+Cairo bridge
libpangoft2-1.0.so.0    ← font renderer
libatk-1.0.so.0         ← accessibility bridge
libatk-bridge-2.0.so.0  ← ATK-Wayland/X11 bridge
libwebkit2gtk-4.1.so.0  ← HTML engine
libjavascriptcoregtk-4.1.so.0 ← JS engine
libsoup-3.0.so.0        ← HTTP client
```

**15 native library dependencies, all removable.**

---

## 2. Non-GTK Dependencies Verification

The following were verified to have **zero GTK path** in their dependency trees:

| Crate | Role | Verified |
|-------|------|----------|
| `gettext-rs` (0.8) | i18n via GNU gettext | No GTK → no glib, no gio |
| `ksni` (0.3) | System tray (StatusNotifierItem over D-Bus) | No GTK at all |
| `zbus` (5) | D-Bus single-instance guard + remote API | Pure Rust |
| `ureq` (2) | HTTP client for catalogue/OTA | No GTK |
| `ring` (0.17) | Ed25519 package signatures | No GTK |
| `x11rb` (0.14) | X11 window management, RandR queries, EWMH | No GTK |
| `zip` (9) | .ferramari package format | No GTK |
| `clap` (4) | CLI argument parsing | No GTK |
| `serde` / `serde_json` | Serialization | No GTK |

All non-UI logic modules are confirmed GTK-free. The ~12k lines of business logic live in safe territory.

---

## 3. All GTK/GDK Call Sites by Module

### main.rs (318 lines) — Entry Point

| Line(s) | Construct | Purpose | Replacement Path |
|---------|-----------|---------|------------------|
| 40 | `use gtk::prelude::*` | Import traits | std / Iced prelude |
| 122 | `gtk::Application::new(...)` | Register app identity for GTK event loop | Iced runs its own Winit-based event loop — this becomes a simple binary entry |
| 132–155 | `connect_startup`, `connect_activate`, `connect_shutdown` | Lifecycle hooks | Iced uses `Application::with_flags()` and `Component::init()` |
| 160–162 | `glib::unix_signal_add(signal)` | Handle SIGINT/SIGTERM | Dedicated OS-level signal thread |
| 167 | `gtk_app.run_with_args(&[])` | Start GTK main loop | Iced `Application::run()` — different API, same effect |

### app.rs (2 336 lines) — Core State Machine

This is the most important finding: **~85% of app.rs is GTK-independent.** The command channel + serialisation pattern needs zero changes. Only these parts touch GTK/GIO/GLib:

| Line(s) | Construct | GTK dependency level | Notes |
|---------|-----------|---------------------|-------|
| 19 | `use gtk::prelude::*` | Trait import | Widget-free |
| 202 | `gtk_app: gtk::Application` | Structural field holding handle to keep GTK alive | Must go; nothing equivalent needed post-migration |
| 246 | `glib::SourceId` | Timer source tracking | Own `Instant`-based debounce mechanism |
| 250 | `gtk::gio::ApplicationHoldGuard` | Single-instance enforcement via GIO hold | Already partially handled by zbus name ownership check |
| 349 | `glib::unix_fd_add_local(fd, IOCondition::IN, ...)` | Wake pipe read | Replace with std poll/select or tokio UnixStream |
| 352, 366 | `glib::ControlFlow::Break` | Exit subscription | Iced subscriptions return `Option<T>` — equivalent pattern exists |
| 364–370 | `glib::timeout_add_local(Duration::from_millis(40))` | Command polling loop | Iced `subscription::every()` or persistent spawn |
| 376–423 | `glib::timeout_add_local` × 4 (player watch, rotation, power, monitor events) | Periodic watchdogs | Iced interval subscriptions |
| 2070 | `gtk::gio::File::for_uri(url).path()` | Resolve URL to local path | Pure Rust: `Url::parse(url).to_file_path()` |
| 2157 | `gtk::gio::SimpleAction::new(name, None)` | D-Bus action wiring | Replace with zbus property/method bindings |

**Conclusion on app.rs:** Can proceed largely as-is once glib source registrations are moved to Iced subscriptions. The business logic layer is decoupled from presentation.

### ui/control.rs (4 358 lines) — Control Window

**The largest single GTK dependency.** Every widget type used in the entire application lives here:

| Category | Constructs Used | Call Site Count |
|----------|-----------------|-----------------|
| Window/container widgets | `ApplicationWindow`, `HeaderBar`, `Box`, `ScrolledWindow`, `Stack`, `Overlay`, `Revealer` | ~12 types |
| Content widgets | `Label`, `Button`, `Entry`, `ComboBoxText`, `Switch`, `Scale` (slider), `Image`, `Separator`, `ProgressBar`, `FlowBox`, `Popover`, `MenuButton` | ~17 types |
| Drawing | `DrawingArea`, `cairo::Context` (backdrop gradients), `Pixbuf` (thumbnails) | ~3 types |
| Styling | `CssProvider`, `StyleContext`, `gtk::ColorRGBA`, ATK bindings | ~5 types |
| Pango text ellipsization | `pango::EllipsizeMode::End/Middle` (label truncation throughout sidebar/cards/settings) | ~17 call sites |
| File dialogs | `FileChooserDialog` (Open/Save/SelectFolder), `MessageDialog`, `AboutDialog`, `Dialog` | ~6 dialog call sites |
| Events | Event masks, button press handler, context menu popup | ~5 call sites |

Zero business logic — pure presentation. Fully replaceable.

### ui/online.rs (1 536 lines) — Online Catalogue

Same widget categories as control.rs plus:

| Construct | Usage |
|-----------|-------|
| `gdk_pixbuf::Pixbuf` | Downloaded preview images converted to displayable form |
| `pango::EllipsizeMode::End` | Truncate long names/categories/details labels |
| `glib::timeout_add_local` | Lazy-load pagination, scroll progress polling |
| Containers | `Box`, `ScrolledWindow`, `FlowBox`, `Spinner`, `Label`, `Button`, `Entry`, `ComboBoxText` |

Full replacement required.

### ui/wallpaper.rs (712 lines) — Wallpaper Surfaces

**Most critical module.** Not just UI — this manages platform-level desktop windows:

| Line(s) | Construct | Purpose | Criticality |
|---------|-----------|---------|-------------|
| 25 | `gtk::ApplicationWindow` | Toplevel desktop window per monitor | **Critical** |
| 579 | `gdk::WindowTypeHint::Desktop` | Set WM layer hint | **Critical** |
| 598–601 | `gdkx11::X11Window::xid()` | Extract X11 window ID for mpv `wid` parameter | **Critical** |
| 624 | `gdk::EventMask::BUTTON_PRESS_MASK` | Right-click context menu trigger | Medium |
| 112–343 | `webkit2gtk::WebView::builder().settings().build()` | Create web page wallpaper surfaces | **Critical** |
| 293 | `glib::timeout_add_local_once(Duration::from_secs(2))` | Auto-reload failed web pages | Low |
| 614–617 | `glib::timeout_add_local` | Desktop icon layer reordering | Low |
| 8–13 | `use webkit2gtk::{...}` | PolicyDecisionExt, PermissionRequestExt, SettingsExt, URIRequestExt, WebViewExt | High |

This entire module conceptually belongs in `ferramari-platform` or `ferramari-media`, not in the UI crate. It was placed here for convenience in the single-crate layout.

### ui/theme.rs (461 lines) — Liquid Glass Design System

| Construct | Purpose |
|-----------|---------|
| `CssProvider`, `StyleContext` | Install custom CSS theme across GTK stack |
| `gtk::Settings` | Detect dark/light preference, configure font settings |
| `DrawingArea` + `cairo::Context` | Custom Cairo backdrop paint (gradient layers, radial glows, vignette, sheen overlay) |
| `gdk::RGBA` | Color construction |
| Pango font scanning | Font availability checks |

### monitors.rs (233 lines) — Monitor Enumeration

Entire module depends on `gtk::gdk::Display` and `gtk::gdk::Monitor`. These types are **not Send**, forcing monolithic threading assumptions. Replacable entirely via `x11rb` RandR extension queries.

Interestingly, `xrandr_connectors()` already contains working XRandR queries that produce geometry data. This module would need minimal refactoring to become fully x11rb-driven.

### wallpaper.rs (656 lines) — GNOME Static Background Setter

| Construct | Purpose |
|-----------|---------|
| `gio::Settings` (multiple calls) | Read/write GNOME `org.gnome.desktop.background.*` schemas |

Only active for static wallpaper fallback mode. Flatpak path already invokes `flatpak-spawn gsettings`. A pure-Rust replacement using portal APIs or simply removing the feature is straightforward.

### util.rs (415 lines) — Helpers

| Construct | Usage |
|-----------|-------|
| `gio::File::for_path(path).query_info(...)` | Get file metadata (mime-type, size) |
| `gio::File::for_path(path).uri()` | Convert path to file:// URI |

Pure Rust replacements: `std::fs::metadata`, `Url::from_file_path`.

### thumbnails.rs (81 lines) — Thumbnail Generation

Comments reference `gdk_pixbuf::Pixbuf` but actual production code spawns ffmpeg and writes JPEGs to disk cache. The `gdk_pixbuf` usage appears in `ui/control.rs` and `ui/online.rs` where Pixbuf IS actively used to load those cached JPEGs.

### ui/menu.rs (27 lines) — Context Menus

Creates right-click menus on wallpaper surfaces using `gtk::Menu`, `gtk::MenuItem`. Very small.

### signing.rs + package.rs (1 478 lines combined)

| Construct | Usage | Justification for replacement |
|-----------|-------|------------------------------|
| `glib::Checksum::new(glib::ChecksumType::Sha256)` | Package checksum calculation | `sha2` crate provides identical hashing, lighter weight, no GTK tie |

### GTK-independent modules (~12 000 lines)

Zero GTK dependency — safe to carry forward unchanged:
`metrics`, `supervisor`, `catalogue`, `catalog_api`, `plugin`, `library`, `ytdlp`, `desktop`, `power`, `wake`, `handlers`, `dbus`, `tray`, `autostart`, `x11`, `mpv`, `config`, `error`, `cli`, `eco`, `fade`, `paths`

---

## 4. WebKitGTK Role and Evaluation

WebKitGTK creates `WebView` instances that render arbitrary URLs as fullscreen desktop wallpaper surfaces alongside mpv video streams.

**Current workflow:**
1. `ui/wallpaper.rs::web()` creates a GTK `ApplicationWindow` with `type_hint=Desktop`.
2. `webkit2gpu::WebView` instantiated with custom security settings.
3. WebView added as child widget → mapped on desktop layer.
4. Policy decisions restrict navigation for non-network pages.
5. `web-process-terminated` signal tracks liveness; supervisor restarts crashed views.
6. Hidden/muted windows get muted via `set_is_muted(true)` — WebKit throttles rendering.

**Security posture (well-designed):**
- Local file access limited to sibling paths only, never universal filesystem access
- All permission requests denied by default
- Optional network block for local-only pages
- Developer extras disabled
- WebGL/media enabled (required for Unity/Godot/Web content)

**Replacement candidates evaluated against criteria:**

| Option | Linux X11 | Linux Wayland | Windows | RAM (idle) | CPU/GPU impact | Binary size addition | License | Verdict |
|--------|-----------|---------------|---------|------------|----------------|---------------------|---------|---------|
| **CEF (Chromium Embedded Framework)** | Supported | Experimental | Supported | High (300–500 MB) | Moderate-High | Large (+80–120 MB) | Chromium/BSD | Viable but heavy distribution footprint |
| **External browser process + controlled IPC** | Works everywhere | Works everywhere | Works everywhere | Low-medium | Minimal | None extra | Any license | **Preferred target** |
| **Wry / Tauri WebView** | ❌ Uses WebKitGTK backend | ❌ Uses WebKitGTK backend | ✅ Uses wkWebView | N/A | N/A | N/A | MIT | **Unusable on Linux — circular dependency** |
| **Electron-like standalone** | Complex | Complex | Complex | Very high | High | Very large | Apache 2 | Impractical for this use case |
| **Feature-flag isolate WebKitGTK** | ✅ | ⚠️ Limited | ❌ | High (if enabled) | Moderate | Current size | LGPL | **Acceptable interim solution** |

**Recommendation:**
- **Short term (Phase 1–3):** Isolate WebKitGTK behind a feature flag (`feature = "web-wallpaper"`). Keep GTK dependency gated so core binary works without it. Web wallpaper remains functional when feature is enabled.
- **Long term (Phase 5+):** Investigate external browser process approach (custom URI scheme, controlled IPC, sandbox). CEF is viable but expensive; external process is preferred if technical requirements can be met.
- **Critical constraint:** Never use Wry/WebView on Linux — its backend is literally WebKitGTK again.

---

## 5. Wallpaper Surface Implementation

Currently tightly coupled to GTK through four interdependent mechanisms:

### 5.1 Window Creation (GTK-based)

```rust
let window = gtk::ApplicationWindow::new(app);
window.set_decorated(false);
window.set_resizable(false);
window.set_type_hint(gdk::WindowTypeHint::Desktop);
window.set_skip_taskbar_hint(true);
window.set_skip_pager_hint(true);
window.set_accept_focus(false);
window.set_default_size(monitor.width, monitor.height);
window.move_(monitor.x, monitor.y);
window.show_all();
window.resize(monitor.width, monitor.height);
```

### 5.2 X11 Window ID Extraction (GDK-dependent)

```rust
let gdk_window = window.window()
    .ok_or_else(|| anyhow!("wallpaper window was not realized"))?;
let x11 = gdk_window
    .downcast::<gdkx11::X11Window>()
    .map_err(|_| anyhow!("GDK is not using X11; cannot embed the video surface"))?;
let xid = x11.xid();
```

This XID is passed to mpv as the parent rendering surface (`MpvOptions { wid: Some(xid), ... }`). Without it, mpv cannot embed.

### 5.3 WM Integration (partially GTK-independent)

- `wait_until_viewable(xid)` — polls X server via `x11rb` connection (already GTK-free)
- `lower_x11_window(xid)` — sends `ConfigureWindow BELOW` via `x11rb` (already GTK-free)
- Desktop icon reordering after 2.5s delay via `glib::timeout_add_local`

### 5.4 Web Content Embedding (WebKitGTK-dependent)

```rust
let webview = webkit2gtk::WebView::builder().settings(&settings).build();
window.add(&webview);
webview.show();
webview.load_uri(uri);
```

WebView is a GTK child widget inside the wallpaper surface window. No separation between container and content.

### Required New Architecture

Move wallpaper surface creation entirely into `ferramari-platform`:

```
ferramari-platform/src/linux/
├── wallpaper_surface/
│   ├── mod.rs          # Abstract surface manager trait
│   ├── x11.rs          # Direct X11 window creation via x11rb
│   └── wayland.rs      # Layer-shell protocol (future)
├── mpv_embedder.rs     # Spawn mpv with native XID/surface handle
└── web_engine.rs       # Web page wallpaper (CEF or external process)
```

Abstract interface:

```rust
pub struct SurfaceHandle { /* opaque */ }

pub trait Wallpapers {
    fn create_surface(monitor: &MonitorInfo) -> Result<SurfaceHandle>;
    fn set_video_player(&self, handle: MpvHandle) -> Result<()>;
    fn set_web_content(&self, uri: &str) -> Result<()>;
    fn lower(&self);
    fn destroy(self);
}
```

---

## 6. System Tray Implementation

**Status: Already GTK-free.**

Uses `ksni` (Knobs StatusNotifierItem), a Rust-native implementation of the Freedesktop StatusNotifierItem protocol over D-Bus. Zero GTK dependency.

### What ksni does today:
- Spawns `StatusNotifierWatcher` on session bus
- Implements `KsniTray` trait: id, title, icon_name, category, activate callback, menu items
- Menu items send `Command` variants back through shared channel
- Supports GNOME AppIndicator extension, KDE Plasma, and any SNI host

**No migration needed.** Move the module to `ferramari-platform` during workspace restructuring.

---

## 7. File Dialogs Implementation

Three blocking GTK file chooser locations in `ui/control.rs`:

| Function | Line | Dialog Type | Blocking Pattern |
|----------|------|-------------|------------------|
| `choose_file(parent, title)` | ~2093 | Open single file | `dialog.run()` blocks calling GTK thread |
| `save_file(parent, title, name)` | ~2112 | Save file | Same blocking pattern |
| `choose_folder(parent, title)` | ~2132 | Select folder/directory | Same blocking pattern |
| Additional `prompt_text` | ~2151 | Text input dialog | Modal `gtk::Dialog` |
| Error/info dialogs | Throughout | `MessageDialog`, `AboutDialog` | Various GTK modal dialogs |

All use `.run()` which blocks the calling GTK main thread until user responds. Under Iced's async model, these must become either:
- **Modal overlays rendered within the Iced window** (preferred, keeps everything in-app)
- **Platform-native picker invoked asynchronously**, result returned via command channel

### About `rfd`:
On Linux, `rfd` defaults to GTK-based chooser. To avoid pulling GTK:
- Set `RFD_BACKEND=xdg-desktop-portal` before initialization
- Or implement XDG Desktop Portal file picker directly via `zbus`
- Or implement via `gio`'s portal GProxyApp for cross-desktop compatibility

### Recommended approach:
Define abstract dialog traits in `ferramari-platform`, implement via XDG Desktop Portal on Linux and Win32 `IFileDialog`/`GetOpenFileName` on Windows. Return results async via command channel.

---

## 8. Notifications Implementation

**Ferramari currently has no explicit notification subsystem.** There are no GTK-dependent notification calls to remove.

Current communication mechanisms:
- **In-app:** Toast banners (`show_info()` near header bar) and error dialogs (GTK-dependent, will be migrated with UI)
- **System tray:** Context menu actions
- **D-Bus:** Remote API for scripts/other apps

If notifications are added later:

| Platform | Mechanism | Implementation |
|----------|-----------|----------------|
| Linux | Freedesktop Notifications Specification | Direct `zbus` proxy on `org.freedesktop.Notifications` |
| Windows | Windows Toast Notifications | `windows` crate toast activation API |

---

## 9. GLib Main Loop Usage

There is no standalone "GLib Main Loop" outside GTK, but GLib primitives serve as the application's scheduling backbone. **~24 timer/callback pairs** follow the same pattern: capture state, schedule callback, return `ControlFlow`.

### Detailed inventory:

| Primitive | Location | Call count | Purpose | Iced Replacement |
|-----------|----------|-----------|---------|-----------------|
| `glib::timeout_add_local(duration)` | main.rs, app.rs, fade.rs, ui/online.rs, ui/control.rs, ui/wallpaper.rs | ~20 | Periodic timers: player watch (1 s), rotation check (5 s), power status (30 s), fade animations (~40 ms poll), lazy-load pagination, scroll progress polling, idle-frame-cap timer | Iced `subscription::every(Duration)` with executor-aware interval |
| `glib::timeout_add_local_once(delay)` | app.rs (~line 948), ui/wallpaper.rs (~line 293) | ~3 | One-shot delays: monitor hotplug debouncing, auto-retry failed web page reload | Iced `oneshot` channel + `delayed` spawn |
| `glib::idle_add_local_once` | ui/control.rs (~line 3747) | ~1 | Defer UI update to next idle cycle | Iced scheduler/perform_action |
| `glib::unix_signal_add(SIGINT/SIGTERM)` | main.rs (~line 160) | 1 | Graceful shutdown handling | Crossbeam/channel + dedicated signal thread |
| `glib::unix_fd_add_local(fd, IOCondition::IN)` | app.rs (~line 349) | 1 | Wake pipe monitoring for command dispatch | `epoll`/`poll` wrapper or tokio `UnixStream` reader |
| `glib::IOCondition::IN` | app.rs (~line 349) | 1 | Pipe readiness flag for unix_fd_add | Poll readiness check |
| `glib::ControlFlow::Continue/Break` | Throughout | ~40 occurrences | Callback return values signaling continuation or termination | Iced subscriptions return `Option<Message>` — equivalent pattern exists naturally |
| `glib::SourceId` | app.rs (~line 246) | 1 field | Store timer source ID for cancel/debounce | Own `Instant`-based tracking |
| `glib::Checksum::new(Sha256)` | signing.rs (~line 498), package.rs (~line 242) | 2 | SHA-256 checksumming for .ferramari package integrity | `sha2` crate, identical functionality |
| `gtk::gio::Settings` | wallpaper.rs (~lines 107, 252, 287, 313, 416) | ~5 | GNOME gsettings access for background setter (GNOME-specific) | Portal API invocation or flatpak-spawn (already used in Flatpak path) |
| `gtk::gio::File` | util.rs (~lines 113, 116, 117, 130) | 3 | Path ↔ URI conversion, file metadata queries | `Url::from_file_path()` + `std::fs::metadata` |
| `gtk::gio::ApplicationHoldGuard` | app.rs (~line 250) | 1 field | Single-instance enforcement via GIO hold | zbus `request_name` ownership check (already partially implemented) |

### Key structural pattern analysis:
Every timeout/callback follows this template:
```rust
glib::timeout_add_local(interval, move || {
    let Some(app) = weak.upgrade() else {
        return glib::ControlFlow::Break;
    };
    // do work...
    glib::ControlFlow::Continue  // or Break when done
});
```

This maps cleanly to Iced subscriptions:
```rust
fn subscription(&self) -> Subscription<Message> {
    Subscription::run_with(
        clone!(app_weak => move |_| clock::clock(clock::Clock::stream()).map(move |_| Message::Tick)),
    )
}
```

---

## 10. Replacement Plan Per Dependency

| # | Dependency | Where Used | Why GTK | Replacement Plan | Risk | Priority |
|---|-----------|-----------|---------|------------------|------|----------|
| 1 | `gtk` | main.rs, app.rs, ui/*, monitors.rs, util.rs, wallpaper.rs | Entire UI framework | Migrate to Iced with Winit backend | High | **P0** |
| 2 | `gdkx11` | ui/wallpaper.rs | X11 window ID extraction for mpv embedding | Direct `x11rb` X11 window creation | Medium | **P0** |
| 3 | `webkit2gtk` | ui/wallpaper.rs | Webpage wallpaper engine | Feature-flag isolate → evaluate CEF/external browser process | High | **P0** |
| 4 | `gdk::Monitor` / `gdk::Display` | monitors.rs | Multi-monitor detection | `x11rb` RandR (already partially present in `xrandr_connectors()`) | Low | P1 |
| 5 | File dialogs | ui/control.rs | Open/Save/Folder selection | XDG Desktop Portal via `zbus` or `rfd` with portal backend | Medium | **P1** |
| 6 | GLib timers/sources | main.rs, app.rs, fade.rs, ui/* | Event loop scheduling primitive | Iced `interval` subscriptions + scheduler | Medium | **P0** |
| 7 | `gdk-pixbuf::Pixbuf` | ui/online.rs, ui/control.rs, thumbnails.rs | Thumbnail/image rendering | `image` crate or ffmpeg RGBA export | Low | P1 |
| 8 | `gio::Settings` / `gio::File` | wallpaper.rs, util.rs, app.rs | GNOME BG, file helpers, hold guard | Portal/zbus/std replacements | Low | P2 |
| 9 | `pango::EllipsizeMode` | ui/online.rs, ui/control.rs (17+ sites) | Label text truncation | Rust str truncation or Iced label built-in support | Low | P1 |
| 10 | `glib::Checksum` (SHA-256) | signing.rs, package.rs | Package integrity verification | `sha2` crate, no change in behavior | Low | P2 |
| 11 | Wallpaper surface management | ui/wallpaper.rs | Desktop-layer fullscreen surfaces | `ferramari-platform` with direct X11/Wayland window creation | High | **P0** |
| 12 | GLib Main Loop coordination | Throughout (~24 callbacks) | Application-wide scheduling | Iced event loop + subscriptions | Low (mechanical) | **P0** |

---

## 11. Migration Risks

### Critical Risks

1. **WebKitGTK isolation complexity** — WebKitGTK 4.1 only has maintained Rust bindings under GTK (`webkit2gtk` crate). Removing GTK means losing webpage wallpaper unless an alternative is proven. CEF adds significant binary distribution size (~80–120 MB). External browser process is the preferred long-term path but requires custom protocol handler development.

2. **Direct X11 window creation for mpv embedding** — The current wallpaper surface extracts an X11 window ID from GTK (`gdkx11::X11Window::xid()`) and passes it to mpv as the parent surface. Direct `x11rb::Connection::create_window` for this purpose is straightforward theoretically but untested in production Ferramari. Wayland equivalents (`layer-shell` protocol) are even less explored.

3. **Single-crate to multi-crate refactoring at 20k LOC** — Everything lives in one crate. While ~40% of code is GTK-independent, creating workspace boundaries requires careful interface design to prevent new coupling between `ferramari-ui`, `ferramari-core`, `ferramari-platform`, and `ferramari-media`.

4. **Iced/Winit ≠ compositor-level window management** — Iced provides top-level application windows through Winit. Neither provides desktop-layer window creation, EWMH layer hints, or Wayland layer-shell protocols. Wallpaper surfaces must live in `ferramari-platform`, not in Iced.

### Medium Risks

5. **Threading model shift** — GTK enforces single-threaded widget access (widgets leave main thread → undefined behavior). Iced also runs on a single thread for rendering but doesn't enforce thread affinity for callbacks. Existing patterns sending commands from various threads should continue working via the command channel, but any code accessing GTK internals from background threads would fail silently vs. panic immediately.

6. **Performance regression risk (Liquid Glass backdrop)** — The current Cairo-backed GTK rendering (gradient layers, radial glows, vignette, sheen overlay) is highly optimized for Compositor composition. Iced's rendering pipeline (tiny-skia → Naga → Vulkan/D3D/Metal) may produce different GPU/CPU profiles for the same visual effect. Requires measurement.

7. **Translation workflow divergence** — gettext PO files work fine for pure Rust code. For Iced-generated UI code, integration might require separate localization mechanisms. Simplest path: keep gettext for Rust core and expose translated strings to Iced components via function calls rather than declarative UI languages.

8. **Flatpak manifest overhaul** — Currently targets `org.gnome.Platform` (GTK 3 SDK). After GTK removal, Flatpak build environment must change entirely (likely `org.freedesktop.Platform` + custom toolchain). Risks breaking the existing Flatpak CI pipeline during transition period.

9. **Catalog virtualization performance** — Online catalogue has lazy-loaded paginated grids with download progress tracking. Replicating this efficiently in Iced requires understanding of available list/virtualization primitives and whether they match FlowBox performance characteristics.

### Low Risks

10. **gettext/l10n infrastructure** — Russian (`ru.po`), English (`en.po`), Japanese (`ja.po`) translations in `po/`. Pure Rust consumers unaffected. String paths remain identical.
11. **Package format (.ferramari)** — zip archive containing media, JSON manifest, Ed25519 signature. No GTK involvement whatsoever.
12. **D-Bus remote API** — Uses `zbus`. Already GTK-free. Remote scripts/desktop integrations work independently of UI toolkit.
13. **System tray** — Already using `ksni` (StatusNotifierItem over D-Bus). Zero GTK dependency. Drop-in ready.
14. **Configuration serialization** — serde + JSON for config storage. Completely independent of GUI framework.
15. **yt-dlp wrapper** — Child process invocation with pipe management. No graphical dependencies.

---

## 12. Proposed Final Architecture

Based on discovered module boundaries and actual dependency structure:

```
ferramari-workspace/
│
├── ferramari-ui/                 # Iced UI layer (replaces src/ui/)
│   ├── Cargo.toml               # deps: iced [tokio, image]
│   └── src/
│       ├── lib.rs               # Re-export, public API boundary
│       ├── app.rs               # Iced Application definition
│       ├── state.rs             # UiState synced from core Snapshot
│       ├── message.rs           # UI messages → CommandSender
│       ├── update.rs            # Iced update handlers
│       ├── view.rs              # Iced View impl + page router
│       ├── theme.rs             # Dark/Light/System palettes (CSS tokens → Iced Colors)
│       ├── navigation.rs        # Page routing, sidebar selection
│       ├── pages/
│       │   ├── dashboard.rs     # Home overview with monitor states
│       │   ├── library.rs       # Video library grid/list view
│       │   ├── online.rs        # Online catalogue browsing
│       │   ├── performance.rs   # CPU/RAM/GPU stats display
│       │   └── settings.rs      # Settings form with grouped sections
│       └── components/
│           ├── sidebar.rs       # Navigation rail
│           ├── header.rs        # Title bar with status chip/actions
│           ├── wallpaper_card.rs # Reusable media card widget
│           ├── monitor_card.rs  # Monitor tile with Eco state badge
│           ├── dialogs.rs       # Modal overlays (replace GTK FileChooserDialog)
│           ├── search_field.rs  # Search input component
│           └── status_badge.rs  # Mode indicator pill
│
├── ferramari-core/               # Business logic (mostly unchanged)
│   ├── Cargo.toml               # deps: serde, serde_json, ... (all GTK-free)
│   └── src/
│       ├── lib.rs
│       ├── app.rs               # State machine extracted from current app.rs
│       ├── config.rs            # JSON config loader/save/migration
│       ├── eco.rs               # Eco Engine profiles
│       ├── library.rs           # Local video library CRUD
│       ├── commands.rs          # Unified Command enum (unchanged)
│       ├── supervisors.rs       # Player watchdog / restart policy
│       ├── metrics.rs           # System resource monitoring
│       └── ...                  # (all existing pure-Rust modules)
│
├── ferramari-platform/           # OS/window management (new crate)
│   ├── Cargo.toml               # deps: x11rb, zbus, ksni, maybe rfd
│   └── src/
│       ├── linux/
│       │   ├── x11.rs           # X11 window creation + surface management via x11rb
│       │   ├── wayland.rs       # Wayland layer-shell (future)
│       │   ├── monitors.rs      # XRandR monitor enumeration
│       │   └── wallpaper.rs     # Wallpaper surface manager
│       ├── mpv_embedder.rs      # Native mpv spawning (no GTK xid needed)
│       ├── tray.rs              # ksni integration (moved from src/tray.rs)
│       ├── dbus.rs              # zbus single-instance + remote API (moved from src/dbus.rs)
│       ├── file_dialog.rs       # XDG Desktop Portal file picker
│       ├── autostart.rs         # Desktop entry management (moved from src/autostart.rs)
│       ├── power.rs             # Battery status probing (moved from src/power.rs)
│       └── wake.rs              # Pipe-based wakeup (moved from src/wake.rs)
│
├── ferramari-media/              # Media engine (new crate)
│   ├── Cargo.toml
│   └── src/
│       ├── mpv.rs               # mpv subprocess controller (moved from src/mpv.rs)
│       ├── ytdlp.rs             # yt-dlp wrapper (moved from src/ytdlp.rs)
│       ├── fade.rs              # Volume fade animation (moved from src/fade.rs)
│       ├── web_engine.rs        # Web page wallpaper: CEF or external process placeholder
│       └── thumbnails.rs        # Thumbnail generation (moved from src/thumbnails.rs)
│
├── ferramari/                    # Thin binary (entry point + CLI tools)
│   ├── Cargo.toml               # deps: ferramari-ui, ferramari-core, ferramari-platform, ferramari-media
│   └── src/
│       ├── main.rs              # Thin entry point, parses args, launches UI or CLI mode
│       └── package.rs           # Headless .ferramari packaging tool (runs before GTK startup anyway)
```

### Dependency graph

```
ferramari-ui
    ├─→ ferramari-core (reads Snapshot state, sends Commands)
    └─→ ferramari-platform (window/dialog operations)

ferramari-core
    ├─→ ferramari-platform (surface creation, tray, dbus registration)
    └─→ ferramari-media (mpv spawning, fade control, web engine)

ferramari-platform
    └─→ NO GTK, NO WEBKITGTK (pure rust + x11rb + zbus + ksni)

ferramari-media
    └─→ MAY pull webkit2gtk ONLY IF feature = "web-wallpaper" enabled
```

### Key architectural decisions

1. **Wallpaper surfaces move to ferramari-platform** — They are platform-level, not UI-level. This decouples desktop-layer window management from any GUI toolkit.
2. **Media engine moves to ferramari-media** — mpv spawning, fade management, thumbnail generation, and future web engine form a cohesive unit.
3. **Core stays pure Rust** — No changes needed beyond extracting the command bus from GTK lifecycle glue.
4. **Web engine is feature-gated** — During Phases 1–3, WebKitGTK stays behind `web-wallpaper` feature flag so core binary compiles without it. Phase 5 replaces with proper solution.
5. **Tray/dbus stay in platform** — Already GTK-free, just relocating them to correct crate.

---

## 13. Post-Migration Verification Plan

### 13.1 Cargo dependency tree verification

After each migration phase, verify:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd ferramari-workspace/ferramari

# Verify GTK/GDK absence (these commands should find zero results)
cargo tree -i gtk          # expected: "error: package ID specification `gtk` not found"
cargo tree -i gdk
cargo tree -i gdkx11
cargo tree -i webkit2gtk
cargo tree -i javascriptcore-rs
cargo tree -i soup3
cargo tree -i atk
cargo tree -i cairo-sys-rs
cargo tree -i gobject-sys

# With web-wallpaper feature: webkit2gtk allowed, gtk still absent
cargo tree --features "web-wallpaper" -i webkit2gtk  # expected: visible
cargo tree --features "web-wallpaper" -i gtk          # expected: NOT found

# Verify presence of expected replacements
cargo tree -i iced
cargo tree -i x11rb
cargo tree -i zbus
cargo tree -i ksni
cargo tree -i sha2
```

**Verification criterion:** `gtk`, `gdk`, `gdkx11` MUST appear zero times in `cargo tree` output across ALL feature combinations. `webkit2gtk` MAY appear only when `web-wallpaper` feature is explicitly enabled.

### 13.2 Runtime dynamic library verification

```bash
# Build release binary
cargo build --release

# Check for remaining GTK/WebKit linkage
ldd target/release/ferramari | grep -E 'libgtk|libgdk|libwebkit2|libjavascriptcore|libsoup|libpango|libatk|libcairo'
```

Expected result: **empty output**. No runtime references to GTK ecosystem libraries.

For the media-only path (if web engine uses CEF):
```bash
ldd target/release/ferramari | grep -i chrom
```

CEF-based runtimes may link Chromium shared libraries — document these separately.

### 13.3 Clean build environment test

Test compilation in environments WITHOUT GTK development packages:

| Environment | Packages installed | Expected outcome |
|-------------|-------------------|-----------------|
| Fedora minimal (dnf) | rustc, cargo, gcc, pkg-config, x11-devel, libdbus-devel, openssl-devel | **Build succeeds** |
| Ubuntu minimal (apt) | rustc, cargo, gcc, pkgconf, libx11-dev, libdbus-1-dev, libssl-dev | **Build succeeds** |
| Arch (pacman) | base-devel, xorg-x11, dbus, openssl | **Build succeeds** |

Runtime test: install on clean system (no GTK dev headers), verify application starts and functions correctly. Confirm `dpkg -l \| grep libgtk3-dev` shows nothing installed.

### 13.4 Functional verification matrix

| Feature | Pre-migration behavior | Post-migration target | Verification method |
|---------|----------------------|----------------------|---------------------|
| Control window renders | GTK widgets on X11/Wayland | Iced widgets on Winit | Manual inspection, compare screenshots |
| Dashboard displays correctly | Live monitor/Eco states | Same | Load app, verify states match |
| Library grid loads smoothly | FlowBox cards ~100ms/page | Same or better | Load 100+ entries, time scroll response |
| Catalogue browses & downloads | Paginated FlowBox | Virtualized list | Download 3 catalogue items |
| Video wallpaper plays | mpv in GTK desktop window | mpv in native X11 surface | Play local MP4 on dual monitor |
| Stream wallpaper resolves | yt-dlp URL → play in GTK | yt-dlp URL → play in native | yt-dlp resolve + play stream |
| Web wallpaper renders | WebKitGTK WebView | CEF or external browser | Test known webpages |
| Static wallpaper sets/restores BG | gsettings GNOME BG | Portal or skip | Apply, restart, verify restoration |
| Eco Engine idle/parking | Pause video on idle | Same | Trigger idle timeout, verify pause |
| Volume fade animation | glib timeout interpolates | Iced interval interpolates | Slider drag, smoothness check |
| System tray shows + responds | ksni → SNI watcher | Same | Panel area check + click each item |
| File picker opens & returns paths | GTK FileChooserDialog | XDG Desktop Portal | Import/export .ferramari successfully |
| Monitor hotplug detected | GDK monitor events | XRandR polling | Unplug/replug during operation |
| D-Bus remote control | zbus interfaces | Same | Second instance launch → GUI appears |
| Package sign/verify flow | sha2 checksums | Same (possibly sha2 directly) | Sign locally then verify |
| i18n en/ru/ja mid-session | gettext tr() | gettext tr() + Iced string passing | Switch language, verify UI updates |
| Dark/light theme toggle | GTK Settings dark pref | Iced theme switch | Toggle, verify colors match intent |
| HiDPI scaling | GDK scale_factor | Winit scale_factor | Test on 2× DPI display |

### 13.5 Performance benchmarking

Measure before/after on same hardware:

| Metric | Measurement Tool | Acceptance Criterion |
|--------|-----------------|---------------------|
| Cold start time | `time ferramari --start` | ≤ GTK baseline |
| Dashboard first paint | Timestamp `activate()` → frame | ≤ 200 ms |
| Library grid scroll FPS | `vainfo` / `intel_gpu_top` | ≥ 55 fps sustained |
| RAM footprint (idle, no video) | `ps -o rss ferramari` | ≤ GTK baseline |
| CPU idle (no video playing) | `top %CPU ferramari` | ≤ GTK baseline |
| GPU memory (video wallpaper) | `nv_top` / `intel_gpu_top` | ≤ GTK baseline (no regression) |
| Shutdown grace period | `time ferramari --quit` | ≤ 1 second |
| Fade animation smoothness | Visual + timing log | No stuttering artifacts |

---

## Appendix A: GTK Footprint Summary

| Area | Lines of GTK-bound code | Severity | Phase to migrate |
|------|------------------------|----------|-----------------|
| `ui/control.rs` | 4 358 | **Critical** (entire control window) | Phase 3 (Main UI) |
| `ui/online.rs` | 1 536 | **Critical** (entire catalogue) | Phase 3 (Main UI) |
| `ui/wallpaper.rs` | 712 | **Critical** (platform-level) | Phase 2 (Platform) |
| `ui/theme.rs` | 461 | **High** (design system) | Phase 1 (Prototype) |
| `main.rs` | 318 | **High** (entry point) | Phase 3 (Main UI) |
| `app.rs` | ~500 GTK-sensitive lines | **Medium** (glue only) | Phase 2–3 |
| `monitors.rs` | 233 | **Low** (GDK→x11rb) | Phase 2 (Platform) |
| `wallpaper.rs` | ~130 GTK-sensitive lines | **Low** (gsettings) | Phase 4 (System Integration) |
| `util.rs` | ~5 GTK-sensitive lines | **Minimal** (GIO file ops) | Phase 4 (System Integration) |
| `signing.rs` + `package.rs` | ~2 SHA-256 calls | **Minimal** | Phase 6 (Cleanup) |

**Total GTK-bound code: ~12,700 lines** (across 10 files)

### GTK-independent code (~7,500 lines)

Safe to carry forward unchanged in Phase 0–1:
`metrics`, `supervisor`, `catalogue`, `catalog_api`, `plugin`, `library`, `ytdlp`, `desktop`, `power`, `wake`, `handlers`, `dbus`, `tray`, `autostart`, `x11`, `mpv`, `config`, `error`, `cli`, `eco`, `fade`, `paths`

---

## Appendix B: Iced Compatibility Validation

During Phase 1 prototype build, we verified:

| Property | Value | Implication |
|----------|-------|-------------|
| Iced version | 0.14.0 (latest stable) | Stable, well-tested |
| Total transitive deps | 841 packages | Manageable workspace size |
| GTK-family crates pulled | **0** | Confirmed GTK-free |
| Renderer | tiny-skia + wgpu (Naga shader compiler) | GPU-accelerated, cross-platform |
| Backend | winit (handles X11, Wayland, macOS, Windows) | Covers all target platforms |
| Clipboard | `clipboard_wayland` on Wayland, X11 clipboard on X11 | Native clipboard support |
| Text rendering | cosmic-text + Harfbuzz | Modern, correct Unicode |
| Async support | tokio subscription backend | Compatible with existing async patterns |
| Image support | image crate integration | Replaces gdk_pixbuf for thumbnails |

**Key finding:** Iced introduces ZERO additional GTK/GTK-derived dependencies. The complete toolchain shift is:
- GTK widgets → Iced Component traits
- Cairo backdrop → Iced Canvas/custom painter or pre-rendered textures
- glib timers → Iced subscription intervals
- gdk pixbuf → image crate
