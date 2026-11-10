# Wayland Support Research for Ferramari

**Status:** Investigation (MIGRATION_TODO §4)  
**Date:** 2026-01-08  
**Reference:** `docs/BACKEND_CONTRACT.md` — §2 table row for surface provider; §3 layering rules  

---

## 1. Problem Statement

Ferramari currently runs as an X11 client under every Linux desktop, including native Wayland sessions via XWayland. The existing wallpaper surface creates a desktop-type X11 window marked `_NET_WM_WINDOW_TYPE_DESKTOP`. Under Wayland this works only because the compositor provides an X server, but:

- **Surface stacking** relative to desktop icons is controlled by the compositor, not our process (documented as `Partial`).
- **Window tracking** for the Eco Engine occlusion detection sees only X11 clients in `_NET_CLIENT_LIST`; native Wayland fullscreen windows are invisible (documented as `Partial`).
- **Display hotplug and enumeration** work through RandR mirroring the compositor's output state, so they are actually Supported even on Wayland — but this is a side-effect, not an intentional integration.
- If the user launches the app without `DISPLAY` set (`WaylandNoDisplay`), nothing works at all.

A native Wayland backend would replace the X11 path when `XDG_SESSION_TYPE == "wayland"` and eliminate all three `Partial` ratings.

The existing architecture is ready for this: `WallpaperSurfaceProvider`, `DisplayProvider`, `PlatformEventSource` are already typed traits in `ferramari-core::platform`, injected at startup. A Wayland adapter lives in `ferramari-platform` alongside the X11 implementation; the binary wires whichever one the session supports.

---

## 2. Protocol Landscape by Capability

### 2.1 Wallpaper Surface Embedding

Ferramari needs to:

1. Create a per-monitor toplevel positioned exactly over its output rectangle.
2. Mark it as wallpaper/desktop-layer so the WM/compositor does not decorate or taskbar it.
3. Place it below desktop icons extensions but above the root background.
4. Track when the surface itself is mapped / unmapped (for `wait_until_viewable` semantics).
5. Receive the surface handle (wl_surface + wl_subsurface, or wlr-surface-id) that mpv can render into.

#### 2.1.1 `zwp-shell-item-v1` (KDE Plasma 6+)

**Repository:** [KWin / KDE Plasma](https://invent.kde.org/plasma/kwin/-/blob/master/src/server/stablesurface.h)  
**Stable:** Yes (Plasma 6.0+)  
**Protocol name:** `org_kde_plasma_shell_item_v1`

KDE Plasma 6 introduced `kde-plasma-desktop-shell-v1` / `zwp-shell-item-v1` as a general-purpose shell-item protocol. It was originally designed for KRunner and the application switcher, but Plasma adopted it for wallpaper too.

How it works:
- Client binds `zwp_shell_item_manager_v1`.
- Creates a `zwp_shell_item_v1` bound to a wl_surface.
- Sets role `desktop` via `set_role("desktop")`.
- Compositor draws the surface as wallpaper, handles scaling, rotation, etc.

**Advantages:**
- Explicitly intended for wallpaper rendering — this is what Plasma *intends* backends to use.
- The compositor manages placement under panel/overlay, no manual stacking needed.
- Handles multi-monitor layout natively (each item is bound to a specific output via monitor argument).
- Plasma reports surface visibility through the same protocol (visible/invisible when covered).

**Limitations:**
- KDE Plasma only. No other compositor implements this protocol.
- Only available in Plasma 6 (not Plasma 5).

#### 2.1.2 `zwlr_foreign_toplevel_v1` with shell extension (Not viable)

Some projects tried using the foreign-toplevel protocol combined with a custom overlay surface, but this approach never achieved proper desktop-layer semantics and is not used by any major compositor for wallpaper purposes. **Skip.**

#### 2.1.3 `zwlr_layer_shell_v1` (wlroots ecosystem + Hyprland + others)

**Repository:** [wlr-protocols](https://github.com/wlesg/zswlr-layer-shell-unstable-v1)  
**Protocol version:** `unstable-v1` (never stabilized)  
**Compositors that support it:** sway, Hyprland,river, labwc, mutter (GNOME, since 43, limited), Niri, sratte, wayfire

This is the most widely adopted protocol among non-KDE compositors. While primarily designed for overlays (bars, panels, notifications), several compositors interpret a layer-shell surface with the right parameters as a suitable wallpaper carrier.

How it works:
- Client binds `zwlr_layer_shell_v1`.
- Creates a layer surface with `configure.layer = ZWLR_LAYER_SHELL_V1_LAYER_BACKGROUND`.
- Optionally sets `configure.zone = -1` (full coverage).
- The compositor passes a `zwlr_layer_surface_v1` back, which produces a `wl_surface`.

**Compositor-specific behavior for BACKGROUND layer:**

| Compositor | Background layer behavior | Notes |
|---|---|---|
| **Sway** | Draws behind everything in the bg layer | Supports multiple surfaces per workspace; last-mapped wins visually. Does not auto-size. |
| **Hyprland** | Draws in the `bg` layer | Respects `workspace` assignment. Has `background` command for native wallpapers but also renders bg-layer surfaces. |
| **GNOME/Mutter** | Limited background-layer support since ~43 | Mutter added partial support; GNOME still prefers its own API. Known issues: flickering, scale factor handling sometimes wrong. GNOME users typically get the XWayland fallback anyway. |
| **Niri** | Full background support | Designed with tiling in mind; background layer works correctly. |
| **River** | Supports layer v1 | River uses this protocol directly for its desktop layer. |
| **Wayfire** | Supports layer v1 | Works; wayfire has a separate wallpaper plugin but accepts external surfaces too. |

**Advantages:**
- Available on every major wlroots-based compositor (the largest share of independent Wayland compositors).
- Also usable in Hyprland (which has its own separate protocol but also respects layer-shell).
- Well-documented, many reference implementations exist (mako, eww, various bar projects).
- No GTK dependency.

**Limitations:**
- Protocol is still `unstable-v1` — the wire format could change, though adoption across compositors makes this unlikely.
- Not standardized; each compositor interprets the background layer slightly differently.
- GNOME's implementation is fragile and incomplete — may not be worth optimizing for.
- No built-in notification to compositor about "I am your wallpaper now" — you must manually manage stacking order relative to icon extensions (same issue as X11).

#### 2.1.4 `org_gnome_shell_extension_desktop` (GNOME Shell Extension protocol)

**Available in:** GNOME Shell (proprietary D-Bus protocol)

GNOME Shell does not expose a public Wayland protocol for third-party wallpaper management. Instead, it exposes a **D-Bus interface** for the desktop extension: `org.gnome.Shell.Extensions` or the private protocol `Shell` via introspection.

However, there is **no published Wayland protocol** that allows a third-party client to render directly into GNOME's desktop layer. GNOME expects:
- Static wallpapers are handled by GSD (gnome-settings-daemon) painting an image onto the root window.
- Dynamic wallpapers require writing a GNOME Shell JS extension that hooks into `Meta.Background` — this is the supported mechanism.
- Third-party video wallpaper players like `plasma-pa` and similar use X11 or fall back to static images.

**Conclusion:** There is no clean Wayland-native wallpaper embedding API for GNOME. The practical options are:
1. Use XWayland (current behavior).
2. Attempt layer-shell background — works partially on Mutter 43+ but is officially unsupported.
3. Write a GNOME Shell extension (requires JavaScript/TypeScript, GLib/GJS, deep coupling with GNOME internals).

For Ferramari's scope, option 2 (layer-shell) with graceful degradation to XWayland is the most pragmatic. Option 3 is out of scope for a Rust-first project unless a separate GSExtension component is justified.

#### 2.1.5 Summary: Which protocol for surfaces?

**Primary recommendation: `zwlr_layer_shell_v1`** as the universal Wayland surface path, with `kde-plasma-desktop-shell-v1` as a specialized optimization for Plasma 6+.

| Protocol | KDE Plasma 6 | wlroots (sway/river/niri) | Hyprland | Mutter (GNOME) |
|---|---|---|---|---|
| `kde-plasma-desktop-shell-v1` | Supported (native) | Not available | Not available | Not available |
| `zwlr_layer_shell_v1` | Supported (works) | Supported (primary) | Supported | Partial (fragile) |
| X11 desktop window | XWayland only | XWayland only | XWayland only | XWayland only |

**Recommended strategy:** Probe for available protocols at startup. Priority: Plasma desktop-shell → layer-shell v1 → fail to XWayland/fallback error. Both paths produce a `wl_surface` handle that becomes the opaque `u64` embedding handle — but unlike X11 where mpv receives an XID, Wayland mpv cannot directly render into an arbitrary `wl_surface` over IPC. This leads to §2.1.6.

#### 2.1.6 Critical issue: mpv embedding on Wayland

On X11, Ferramari passes the X window ID to mpv via `--wid=<xid>`. mpv then creates a child window inside that XID and plays directly.

**mpv does not support embedding into an arbitrary Wayland surface.** Its Wayland support is:
- mpv outputs its own Wayland surface directly (`--vo=wayland`).
- mpv supports `--wayland-disable-csd` for client-decoration handling.
- But mpv **cannot take an external `wl_surface` as a parent**. There is no `--wid` equivalent for Wayland in mpv upstream.

This means a pure-Wayland backend needs a different rendering pipeline for the media engine:

**Option A — Shared memory compositing:** Use `wp-linux-dmabuf-v1` (LinuxDMABUF) to export dmabuf file descriptors from mpv's rendering, then import them in a Wayland client that composites frames onto the wallpaper surface. This is what tools like `wlroots-wlr-video` and some OBS-Wayland integrations do. It requires:
- mpv running with `--vo=gpu-context=wayland --gpu-debug` or similar.
- A shared memory buffer exported via LinuxDMABUF.
- A compositor-side importer rendering the buffer onto the wallpaper surface.

**Option B — xwm / virtual X11:** Run a minimal X11 server (like `x11rb`'s virtual display or `Xvfb`) within the process, pass the XID to mpv, then composite the X11 output onto the Wayland surface. This is what `xcwe`-style approaches do. Adds X11 as a runtime dependency within the Wayland session.

**Option C — Hybrid (recommended for Phase 1):** Keep the XWayland path as the Wayland wallpaper backend. Detect `XDG_SESSION_TYPE=wayland` + `DISPLAY` present → run the existing X11 surface creation code. This is what many "native Wayland" applications do today (Firefox, Steam). Layer-shell is implemented as a future optimization once the mpv embedding question is solved.

If Option C is adopted, a native layer-shell backend would only need to handle:
- Display hotplug events (different from RandR).
- Window tracking (foreign-toplevel-list instead of EWMH).
- But reuses the exact same X11 surface code for rendering.

This keeps the initial investment low while delivering measurable capability improvements (hotplug events arrive faster, window tracking sees native Windows).

---

### 2.2 Display Hotplug Detection

On X11, Ferramari polls RandR via `x11rb::protocol::randr` subscription. Under Wayland, RandR only mirrors the compositor's state. Native Wayland hotplug needs a protocol that reports when outputs are connected/disconnected/reconfigured.

#### 2.2.1 `zwlr_output_management_v1` / `zwlr_output_manager_v1`

**Repository:** [wlr-protocols](https://github.com/swaywm/wlr-protocols/tree/unstable/output-management)  
**Protocol versions:** `zwlr_output_manager_v1` (deprecated), `zwlr_output_manager_presentation_offset_v1` (newer)  
**Supported by:** sway, river, hyprland (with limitations), niri, wayfire, mutter (since 44, as `zmutter_output_manager_v1`)

How it works:
- Client binds `zwlr_output_manager_v1`.
- Requests configuration snapshot via `get_configuration()`.
- Gets a `zwlr_output_configuration_v1` containing `zwlr_output_mode_v1` and `zwlr_output_layout_rect_v1` entries.
- Receives `output_enabled`, `output_disabled`, `done` events on config changes.

Each `zwlr_output_state_v1` carries: name, description, make, model, serial, physical dimensions, transform, current mode (width/height/refresrate), position, enabled state.

**Coverage:**

| Compositor | Protocol support | Coverage level |
|---|---|---|
| sway | Full `zwlr_output_management` | Complete |
| Hyprland | Partial (needs `--enable-hyprland-grabs` or similar) | Mostly complete, some metadata missing |
| river | Full | Complete |
| niri | Full | Complete |
| gnome/mutter | `zmutter_output_manager_v1` (private, since 44) | Partial; fewer properties than wlr variant |
| KDE Plasma | Uses KOutputManagement (DBus), NOT wlr-output-management | None via this protocol |
| wlroots-generic | Full if compositor implements it | Varies |

**Note:** This is the standard approach used by `waybar`, `swayimg`, `wlr-randr`, `hyprctl monitor` (Hyprland uses its own command but also advertises via this protocol in recent versions).

#### 2.2.2 KDE KOutputManagement (D-Bus)

**Interface:** `org.kde.KScreen` / `org.kde.KOutputManagement`  
**Bus:** Session bus, accessible everywhere KDE runs

KDE Plasma provides `KScreen` (legacy, `org.kde.KScreen`) and `KOutputManagement` as D-Bus services. These are mature, stable APIs with full EDID and connector-level information.

Ferramari already uses `zbus` for D-Bus communication. Adding a KDE output manager watcher would follow the same pattern as the existing XRandR watcher.

**Coverage:** KDE Plasma only (including X11 sessions). Same service runs regardless of display server.

#### 2.2.3 GNOME Mutter (proprietary)

GNOME does not expose a stable, published protocol for output management over Wayland. Mutter's internal `MetaMonitorInfo` objects are accessed via:
- `zmutter_output_manager_v1` — a private wlr-compatible protocol since Mutter 44.
- `org.gnome.Mutter.DisplayConfig` — a D-Bus interface, somewhat documented but not part of any specification.

Both are less reliable than the wlr or KDE equivalents. For a first-pass Wayland backend, GNOME can be treated as a "partial" capability and default to XWayland polling.

#### 2.2.4 Recommendation for hotplug

Priority order for hotplug detection:
1. `zwlr_output_management_v1` — covers the widest range of compositors (all wlroots ones + Hyprland + Mutter 44+).
2. KDE `KOutputManagement` D-Bus — for Plasma sessions.
3. Fall back to XRandR if running under XWayland (already works).

The implementation should probe for all three at startup and use whichever responds first. If none respond, report `Unsupported` and log why.

---

### 2.3 Window Tracking (Occlusion Detection)

Ferramari needs to know when other windows are fullscreen or cover parts of the wallpaper surface. On X11 this is done through `_NET_CLIENT_LIST_STACKING` and geometry queries on each client. On Wayland, this is fundamentally harder because windows are managed entirely by the compositor.

#### 2.3.1 `zwlr_foreign_toplevel_management_v1`

**Repository:** [wlr-protocols](https://github.com/swasco/zwlr-foreign-toplevel-list-unstable-v1)  
**Protocol version:** `unstable-v1` (never stabilized, but de facto standard)  
**Supported by:** sway, Hyprland, river, niri, wayfire, mutter (since 43)

How it works:
- Client binds `zwlr_foreign_toplevel_manager_v1`.
- Receives `toplevel_added` event with a `zwlr_foreign_toplevel_handle_v1` object.
- Each toplevel fires `title`, `app_id`, `window_type` events.
- `state` events report `fullscreen`, `maximized`, `inactive`, `tiled`.
- `closed` event signals when a window is destroyed.

The compositor returns all known toplevel handles. By querying their state and optionally getting screen position (via `get_geometry` or proprietary extensions), we can determine occlusion.

**Limitation:** This protocol reports top-level application windows, not individualXdgToplevel surfaces. It gives us:
- Whether a window is fullscreen → full occlusion of all monitors.
- App ID → filter out system UI.
- Title → optional logging.
- **But NOT geometry/position** — the protocol explicitly does not provide window positions. A fullscreen window means full occlusion, but a maximized window (not fullscreen) position is unknown.

To get per-monitor occlusion levels (what the Eco Engine needs for smart-pause), we would need additional data:
- Sway/Hyprland expose window positions through their own protocols.
- `xdg_foreign` protocol (separate) can provide more info.
- Some compositors add `zwlr_foreign_toplevel_handle_v1` extensions for geometry.

**Coverage:**

| Compositor | Full toplevel list | Geometry/position | Occlusion accuracy |
|---|---|---|---|
| sway | Yes | Via swaymsg IPC (external query) | High (can query positions separately) |
| Hyprland | Yes | Hyprland stores full window DB | High (can query via `hyprctl clients`) |
| river | Yes | Minimal — relies on state flags | Medium (fullscreen/maximized detectable) |
| niri | Yes | Some extensions | High |
| mutter | Yes | Via Mutter DBus or internal API | Medium-High |
| KDE Plasma | **No** — uses different protocol | Via KDE WindowRules | See below |

#### 2.3.2 KDE Window Rules (org.kde.KWin.Client)

KDE Plasma manages windows through KWin's `org.kde.KWin.Client` D-Bus interface. Each client exposes:
- geometry, size, active/fullscreen/maximized state
- window type (normal, dialog, desktop, dock, splash)
- transient parent relationships
- active application

Combined with the plasma-workspace `org.kde.plasmashell` interfaces, this provides complete occlusion awareness for Plasma sessions.

Already partially integrated: Ferramari's `desktop.rs` detects `org.kde.screensaver` via D-Bus ownership checks. Adding KWin.Client queries follows the same zbus pattern.

#### 2.3.3 GNOME Shell (proprietary)

GNOME Shell does not expose a foreign-toplevel-like API to third parties. `zwlr_foreign_toplevel_list_v1` is implemented in Mutter and returns GNOME window handles, so the basic list works. However, occlusion detail requires querying GNOME internals:
- `org.gnome.Shell` D-Bus interface with proprietary methods.
- Reading `MetaWindow` objects through GI bindings (GTK-dependent, not feasible here).

For GNOME sessions, window tracking will remain `Partial` — we can detect fullscreen windows but not precise occlusion percentages.

#### 2.3.4 Recommendation for window tracking

Priority:
1. `zwlr_foreign_toplevel_management_v1` — primary path for wlroots/Hyprland/GNOME.
2. KDE WindowRules via D-Bus — primary path for Plasma.
3. Combine with compositor-specific IPC (swaymsg, hyprctl) for geometry data where available.
4. Accept that GNOME occlusion is `Partial` until a better mechanism exists.

---

### 2.4 Session Events (Lock, Idle, Sleep)

These mostly already work via D-Bus on Wayland:

| Event | X11 Path | Wayland Equivalent |
|---|---|---|
| Session lock | org.freedesktop.ScreenSaver + login1 | Same D-Bus services (they are display-server-independent) |
| User idle | org.gnome.Mutter.IdleMonitor / org.freedesktop.ScreenSaver | Same (idle monitoring is compositor-agnostic) |
| Suspend/sleep | logind (`org.freedesktop.login1.Manager`) | Same |

No changes needed for session events on Wayland — they are delivered through freedesktop D-Bus services, not display-server protocols.

---

### 2.5 Surface Creation Lifecycle Differences

| Aspect | X11 | Wayland |
|---|---|---|
| **Create window** | `create_window()` with specified geometry at monitor position | `layer_shell.get_layer_surface()` or `shell_item.create()` |
| **Position/size** | Fixed by X server at creation time; must re-query on hotplug | Set by compositor; client requests via configure ack |
| **Mapping** | `map_window()` + poll `MapState::VIEWABLE` | `wl_surface.commit()` + layer surface configure |
| **Decoration removal** | MWM hints + WM_HINTS input=false | Built into layer-shell role (no decorations possible) |
| **Focus exclusion** | WM_HINTS input=false | Layer surfaces are never focusable by design |
| **Destroy** | `destroy_window()` | `layer_surface.destroy()` / `wl_surface.destroy()` |
| **Handle passed to mpv** | XID (u32 → u64) | No direct equivalent — see §2.1.6 |
| **Hotplug response** | RandR OutputChangeNotify event | Output management protocol event |

Key difference: On X11 the surface position is set once at creation and stays fixed. On Wayland with layer-shell, the compositor controls positioning; the client sends a `configure` event and must `ack_configure()` with the new dimensions. This means:

- Monitor replug/reorder → compositor sends new configure → surface resizes.
- Scale factor change → compositor sends new configure.
- Must maintain a map of `(monitor_name, surface)` pairs and recreate surfaces for monitors that appear/disappear.
- Unlike X11 where destroying and recreating a window works trivially, Wayland surfaces must be cleaned through the protocol sequence (unmap → destroy proxy → release resource).

---

## 3. Compositor Support Matrix

Detailed mapping of every major Wayland compositor/session to the protocols Ferramari would need:

| Compositor | Surface Protocol | Hotplug Protocol | Window Tracking Protocol | Notes |
|---|---|---|---|---|
| **Sway** | layer-shell v1 (bg layer) | zwlr-output-mgmt | zwlr-foreign-toplevel + swaymsg IPC | Excellent coverage; layer-shell is well-supported |
| **Hyprland** | layer-shell v1 (plus own bg layer) | zwlr-output-mgmt (partial) | zwlr-foreign-toplevel + hyprctl IPC | Good but Hyprland's bg-layer handling has edge cases |
| **River** | layer-shell v1 (bg layer) | zwlr-output-mgmt | zwlr-foreign-toplevel | Minimal but functional |
| **Niri** | layer-shell v1 | zwlr-output-mgmt | zwlr-foreign-toplevel | Solid support across all three |
| **GNOME/Mutter 44+** | layer-shell v1 (partial) | zmutter-output-mgmt | zwlr-foreign-toplevel | Best-effort; XWayland fallback preferred |
| **GNOME < 44** | layer-shell v1 (broken) | D-Bus (DesktopConfig) | zwlr-foreign-toplevel | Effectively requires XWayland |
| **KDE Plasma 6** | kde-desktop-shell-v1 | KOutputManagement (DBUS) | KWin.Client (DBUS) | Most complete via D-Bus; layer-shell also works |
| **KDE Plasma 5** | XWayland only (no native solution) | KScreen (DBUS) | KWin.Client (DBUS) | No native Wayland wallpaper API exists |
| **wlroots (generic)** | layer-shell v1 | zwlr-output-mgmt | zwlr-foreign-toplevel | Depends on compositor implementation |
| **Wayfire** | layer-shell v1 | zwlr-output-mgmt | zwlr-foreign-toplevel | Requires wf-shell for icons |

---

## 4. Prioritized Implementation Plan

### P0: Essential (blocks all Wayland capability gains)

**P0-A: Layer-shell wallpaper surface**

Implement `zwlr_layer_shell_v1` surface creation for wlroots compositors. Reuse the existing X11 rendering path through XWayland (Strategy C from §2.1.6). The layer-shell surface serves as the compositor entry point; mpv renders through an embedded XWayland connection (or the same process connects to the X server provided by the Wayland compositor's XWayland instance).

This alone upgrades:
- `wallpaper_surface`: Partial → Supported on wlroots/Hyprland
- `display_hotplug`: can attach to output-management protocol
- `window_tracking`: can attach to foreign-toplevel protocol

Implementation estimate: 1-2 weeks for core surface impl + testing.

### P1: Important (significant quality-of-life improvements)

**P1-A: Display hotplug via zwlr-output-management**

Add `ferramari-platform/src/wayland/outputs.rs` with a `zwlr_output_manager` watcher that feeds `Command::MonitorsChanged` events. Parallel implementation alongside XRandR; choose the protocol based on compositor capabilities detected at startup.

**P1-B: Window tracking via foreign-toplevel-list**

Add `ferramari-platform/src/wayland/toplevels.rs` with `zwlr_foreign_toplevel_manager`. Report fullscreen windows as full occlusion. Supplement with compositor-specific IPC where available (swaymsg, hyprctl).

### P2: Nice to have (compositor-specific optimizations)

**P2-A: KDE Plasma desktop-shell integration**

Detect Plasma 6, bind `kde-plasma-desktop-shell-v1`, and use the explicit wallpaper role. This replaces the generic layer-shell approach on Plasma with a purpose-built API. Requires separate test machine with Plasma 6.

**P2-B: Pure-Wayland mpv rendering (long-term)**

Investigate LinuxDMABUF shared-memory bridge between mpv (rendering to its own Wayland surface) and a compositor importer on the wallpaper surface. This would eliminate the XWayland dependency entirely. Estimated effort: 3-6 months of R&D. Not required for initial Wayland support.

### P3: Long-term research

**P3-A: GNOME native integration**

Evaluate whether writing a GNOME Shell extension (in JS) is justified for full GNOME Wayland integration, or if the XWayland fallback + best-effort layer-shell is sufficient. If the user base includes significant GNOME-native users, a small companion component could be warranted.

**P3-B: Standardized protocol advocacy**

Track proposals for a `wp-desktop-wallpaper` protocol (discussed at wlroots community meetings). If a freedesktop-standard protocol emerges, prefer it over the unstable variants.

---

## 5. Cargo Dependencies

A Wayland backend would introduce these dependencies to `ferramari-platform/Cargo.toml`:

| Crate | Purpose | License | Size |
|---|---|---|---|
| `wayland-client` | Wayland protocol client library | LGPL-2.1 | ~200 KB source |
| `wayland-scanner` | Procedural macro for generating protocol bindings | LGPL-2.1 | ~50 KB source |
| `wl-clipboard` (optional) | Clipboard support if exposed | LGPL-2.1 | Small |

No heavy dependencies. All are thin wrappers around raw Wayland socket protocol — comparable in footprint to `x11rb`. The build-time cost is modest (~15-30 seconds extra compilation due to code generation).

All licenses are compatible with Ferramari's GPLv2 license (LGPL-2.1 dynamically links, which satisfies the license terms).

---

## 6. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Layer-shell background layer inconsistently supported | Low-Medium | Medium | Document per-compositor notes in --doctor; keep XWayland as fallback |
| Protocol instability (unstable-v1) | Low | Medium | Wire format has been stable for >5 years; breaking changes are extremely rare once adopted by multiple compositors |
| mpv embed limit blocks full native Wayland | Certain (Phase 1) | Medium | Use XWayland hybrid approach; note limitation explicitly |
| GNOME session effectively unsupported natively | Medium | Low | Expected; XWayland path serves GNOME users adequately |
| Build complexity doubles (two path branches) | Medium | Low | Feature-flagged at runtime via capability probing, not compile-time cfg |

---

## 7. References

- **Layer Shell Protocol**: https://github.com/swaywm/wlr-protocols/tree/master/protocols/stable/wlr-layer-shell-unstable-v1.xml
- **Foreign Toplevel List**: https://github.com/swaywm/wlr-protocols/tree/master/protocols/unstable/zwlr-foreign-toplevel-list-unstable-v1.xml
- **Output Management**: https://github.com/swaywm/wlr-protocols/tree/master/protocols/unstable/zwlr-output-management-unstable-v1.xml
- **KDE Shell Item**: https://invent.kde.org/plasma/kwin/-/blob/master/src/server/stablesurface.h
- **Sway Layer Shell Documentation**: https://github.com/swaywm/sway/wiki/Webpage-and-Videos-background#using-layer-shell
- **Hyprland Background**: https://wiki.hyprland.org/Useful-Utilities/appearance/#setting-backgrounds
- **Mutter Layer Shell Issues**: GNOME bug #98022 (layer-shell background flickering)
- **mpv Wayland documentation**: https://mpv.io/manual/stable/#options-wayland-display
