# Phase 2 Results — Platform Decoupling

> **Date:** 2026-11-05  
> **Status:** Complete (compile-verified; linking requires GTK dev packages which
> are absent in this build environment — see §6)

---

## 1. Workspace structure

```
hidamari-rust/
├── Cargo.toml                      # workspace root (members = crates/*)
├── crates/
│   ├── ferramari-core/             # GTK-free: business logic + command bus
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── bus.rs              # Command, Snapshot, CommandSender (moved from app.rs)
│   │       ├── monitor.rs          # MonitorInfo model + naming rules (GDK-free)
│   │       ├── catalog_api.rs  ┐
│   │       ├── catalogue.rs    │
│   │       ├── cli.rs          │
│   │       ├── config.rs       │  moved from src/
│   │       ├── eco.rs          │  (18 modules, 9 400+ LOC)
│   │       ├── error.rs        │
│   │       ├── library.rs      │
│   │       ├── metrics.rs      │
│   │       ├── package.rs      │  glib::Checksum → sha2
│   │       ├── paths.rs        │  gio::File helpers → std + url
│   │       ├── plugin.rs       │
│   │       ├── power.rs        │
│   │       ├── signing.rs      │  glib::Checksum → sha2
│   │       ├── supervisor.rs   │
│   │       ├── thumbnails.rs   │
│   │       ├── util.rs         │  gio → mime_guess + url
│   │       ├── wake.rs         │
│   │       ├── x11.rs          │  (EWMH watcher, already x11rb)
│   │       └── ytdlp.rs        ┘
│   ├── ferramari-media/            # GTK-free: mpv controller
│   │   └── src/
│   │       ├── lib.rs
│   │       └── mpv.rs
│   ├── ferramari-platform/         # GTK-free: OS integration
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── autostart.rs        # (moved from src/)
│   │       ├── dbus.rs             # single-instance + remote API (zbus)
│   │       ├── desktop.rs          # --doctor (x11rb)
│   │       ├── gnome_bg.rs         # was src/wallpaper.rs; gio::Settings → gsettings CLI
│   │       ├── handlers.rs         # screen lock / logind / idle (zbus)
│   │       ├── monitors.rs         # NEW: GDK → x11rb RandR port + hotplug watcher
│   │       ├── examples/probe_monitors.rs
│   │       └── tray.rs             # ksni (was already GTK-free)
│   └── ferramari-ui/               # Iced prototype (Phase 1)
│       └── src/{lib,main,theme}.rs
└── src/                            # GTK binary (shrinking — Phase 3 target)
    ├── main.rs                     # re-exports core/media/platform modules
    ├── app.rs                      # App state (bus definitions removed → core)
    ├── fade.rs                     # glib timers (stays until Iced takes over)
    └── ui/{control,online,theme,wallpaper,menu}.rs
```

**Files moved:** 25 (18 core, 1 media, 6 platform) + 2 new (bus.rs, monitor.rs,
monitors.rs). **Remaining in `src/`:** `main.rs`, `app.rs`, `fade.rs`, `ui/*`.

---

## 2. GTK removal in moved code (not just moved, actually removed)

| Location | Was | Now | Behaviour preserved |
|----------|-----|-----|---------------------|
| `package.rs::file_digest` | `glib::Checksum::new(Sha256)` | `sha2::Sha256` (streaming, 64 KiB buffer) | Identical hex digest format |
| `signing.rs::key_id` | `glib::Checksum::new(Sha256)` | `sha2::Sha256` | Identical 16-hex prefix |
| `util.rs::is_video` | `gio::File::query_info("standard::content-type")` | magic bytes (ftyp/EBML/OggS) + extension list | Same accept set for mp4/mov/mkv/webm/ogg; extension fallback covers the rest |
| `util.rs::path_to_uri` | `gio::File::for_path(...).uri()` | `url::Url::from_file_path` | Same percent-escaping |
| `gnome_bg.rs` (was `wallpaper.rs`) | `gio::Settings` read/write of `org.gnome.desktop.background` | `gsettings` CLI (`get`/`set`/`list-keys`), same `flatpak-spawn --host` path as before | Same schema keys, same Flatpak semantics; verified live: `background_diagnostic()` → `user background file:///home/blanc/.config/background` |

No new native dependencies: `sha2`, `url`, `mime_guess` are pure Rust. `ring`
was already in the tree (used by ureq/rustls).

---

## 3. GDK monitor port (the compatibility-critical piece)

`MonitorInfo.name` doubles as the `data_source` config key, so the port must
produce the **same names** as GDK.

Source of truth (GTK 3.24 `gdk/x11/gdkscreen-x11.c`, `init_randr15`):

```c
name = gdk_x11_get_xatom_name_for_display(display, rr_monitors[i].name);
gdk_monitor_set_model (GDK_MONITOR (monitor), name);
gdk_monitor_set_scale_factor (GDK_MONITOR (monitor), x11_screen->window_scale);
```

i.e. **model = RandR monitor atom name** (= the connector name on Xorg), and
geometry is divided by `GDK_SCALE`. The port reproduces exactly:

| Aspect | GDK | Port (`ferramari-platform::monitors`) |
|--------|-----|----------------------------------------|
| Name source | `RandR monitor atom name` | `randr_get_monitors` → atom name |
| Fallback name | — | `Monitor {n}` when atom is 0/empty |
| Duplicate disambiguation | caller-side index suffix | same: `{name} #{n}` |
| Geometry scale | `window_scale` (`GDK_SCALE`) | same env var, same division |
| Primary | `rr_monitors[i].primary` → LVDS → first | same cascade |
| Hotplug | `connect_monitor_added/removed` + `connect_size_changed` | `randr_select_input(SCREEN_CHANGE|CRTC_CHANGE|OUTPUT_CHANGE|RESOURCE_CHANGE)` on a dedicated thread → `Command::MonitorsChanged` (the app's existing debounce smooths it) |

**Live verification** (this machine, single VM display):

```
$ DISPLAY=:0 cargo run -p ferramari-platform --example probe_monitors
monitor #0 name="eDP-1" 1920x1080+0+0 scale=1 primary=true
background: user background file:///home/blanc/.config/background
```

`name="eDP-1"` matches the user's existing config key exactly:

```json
"data_source": { "eDP-1": "/home/blanc/Videos/Ferramari/arcane-jinx.mp4", ... }
```

---

## 4. Verification

```
$ cargo check -p ferramari-core      # OK
$ cargo check -p ferramari-media     # OK
$ cargo check -p ferramari-platform  # OK
$ cargo check -p ferramari --tests   # OK  (binary incl. unit tests)

$ cargo test -p ferramari-core --lib
test result: ok. 113 passed; 0 failed
$ cargo test -p ferramari-platform --lib
test result: ok. 19 passed; 0 failed
$ cargo test -p ferramari-media --lib
test result: ok. 9 passed; 0 failed

$ grep -rnE '(gtk|gdk|glib|gio|gdk_pixbuf|webkit2gtk)::' crates/{ferramari-core,ferramari-platform,ferramari-media}/src/
# → only doc-comment mentions, zero code references
```

141 unit tests pass in the three migration crates.

---

## 5. What the binary still links (Phase 3 target)

`src/app.rs` + `src/ui/*` + `src/fade.rs` + `src/main.rs` — the GTK control
window, wallpaper surfaces (`ui/wallpaper.rs`), the Cairo backdrop
(`ui/theme.rs`), glib timer fades, and the `gtk::Application` lifecycle.
The command bus, config, Eco Engine, library, package/signing, mpv, tray,
D-Bus, monitors, and GNOME background are all out of that GTK closure and
already GTK-free.

---

## 6. Build environment note

This build machine has the GTK **runtime** libraries but **no dev packages**
(`libgtk-3.so` symlink and `gtk+-3.0.pc` absent). Consequences:

- `cargo check` / `cargo test --lib` work everywhere (no linking).
- Final linking of the `ferramari` binary fails on `-lgtk-3` etc. **by
  environment**, not by code — the same failure existed before Phase 2.
- `ferramari-core`, `ferramari-media`, `ferramari-platform`,
  `ferramari-ui` link standalone without any GTK package.
- CI (which installs `libgtk-3-dev`) or any dev machine with GTK headers
  links the current binary as before. After Phase 3 the GTK dependency is
  optional (feature-gated web engine), so a GTK-header-free build becomes
  possible — see `GTK_REMOVAL_AUDIT.md` §13 for the target verification.
