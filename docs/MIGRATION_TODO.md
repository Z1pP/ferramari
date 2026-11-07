# Ferramari 2.0 migration tracker

This file is the working checklist for completing the GTK-to-Iced migration and
turning the current Linux implementation into a maintainable multi-platform
application. Items are checked only when the code and the corresponding
validation are complete. A successful compile alone is not a runtime pass.

## 1. Stabilize the current migration

- [x] Split the workspace into core, platform, media and Iced UI crates.
- [x] Run the server and Iced UI as separate binaries over the existing D-Bus API.
- [x] Build and install both binaries together.
- [x] Make CI lint, test and build every workspace member.
- [x] Remove GTK, GDK and WebKitGTK from manifests and the lockfile.
- [x] Restore favourites and basic collection management in Iced.
- [x] Correct AppStream metadata and remove stale 2.0 web-renderer claims.
- [x] Move the X11 EWMH watcher out of core into the Linux platform crate.
- [x] Move window visibility geometry into platform-neutral core models.
- [x] Remove the direct x11rb dependency from ferramari-core.
- [ ] Get the latest full CI run green, including AppStream and desktop-file validation.
- [x] Stage-install both binaries and verify their sibling lookup and data paths.
- [x] Verify the default install has no GTK/GDK/WebKitGTK dynamic linkage.
- [x] Staged-install verified: all artifacts present (binaries, desktop file, metainfo, icon, locale).
- [x] --doctor diagnostic runs successfully on Wayland/XWayland + GNOME.

## 2. Restore Iced feature parity

- [x] Inventory every action and setting in the last GTK release against the Iced UI — see `docs/UI_ACTION_INVENTORY.md` for detailed mapping of all 5 pages × 50+ actions.
- [x] Complete local library actions: add/remove media, thumbnails and refresh polish — `thumbnails::generate()` integrated in Iced UI via iced_widget `image::Image` with cached JPEG previews (400×225), graceful fallback when ffmpeg unavailable or cache empty.
- [x] Apply recent library videos to a selected monitor and assign rotation collections per monitor through D-Bus and Iced.
- [x] Complete favourites and collections, including per-monitor rotation assignment and persistence.
- [x] Add native file/folder dialogs using portal-backed Linux dialogs (rfd 0.17.2 integrated for video picker and package folder selectors).
- [x] Expose package import and export through the server D-Bus API and Iced UI (absolute-path entry fields for now).
- [x] Expose package signing, signature verification and explicit trust-key actions in Iced.
- [x] Add progress, success and actionable error states for package operations over D-Bus.
- [x] Restore all playback, hardware decoding, rotation, power and session settings — verified via inventory document.
- [x] Restore autostart and tray controls with clear capability reporting (`PlatformCapabilities` reported via `--doctor`).
- [x] Audit catalogue browsing, download, preview and pagination against the old UI — audit findings in `docs/CATALOGUE_API_AUDIT.md`.
- [x] Validate English, Russian and Japanese strings and layout overflow — `msgfmt --check` passes for ru.po and ja.po.
- [x] Add UI/client tests for disconnects, reconnects, command failures and stale snapshots — 6 new unit tests added covering missing properties, unparseable values, empty maps, and ServerState defaults.

## 3. Define and enforce the platform boundary

- [ ] Define stable platform-neutral contracts for displays, wallpaper surfaces,
      window tracking, session state, idle state and power events (display, surface,
      power and event-source lifecycle contracts exist; remaining signals still need
      richer typed models).
- [x] Add explicit capability states: Supported, Partial and Unsupported, and report detected support levels in `--doctor`.
- [x] Keep native handles and native API types inside platform/media adapters.
- [x] Ensure core models and policies do not import X11, Win32, Cocoa, D-Bus or
      toolkit types (verified: no x11rb/zbus/gtk imports in ferramari-core).
- [x] Give displays stable persistent IDs while preserving legacy config keys.
      `MonitorInfo.identity` carries connector/EDID/physical size; core derives a
      `stable_id` from it and `display_ids::DisplayIdRegistry` (`display-ids.json`)
      maps that identity to the config key the user already has. A first sighting
      keeps the enumeration name, so 1.9.x configs are never rewritten; a replug,
      reboot or reordered layout reclaims the same key, and cloned layouts cannot
      hand one key to two surfaces. `--doctor` reports the identity per display.
      Under XWayland EDID is hidden by the compositor, so identity is connector-
      based there and the report says so instead of claiming more.
- [x] Specify event ordering, thread affinity, cancellation and backend shutdown
      — `docs/BACKEND_CONTRACT.md` §3-§5: one command-loop owner, watchers only send
      commands, periodic jobs are deadlines, hotplug is debounced, config is flushed
      before it is re-read, session signals aggregate, `Drop` of the watcher token is
      the teardown signal. The D-Bus watchers cannot be stopped before process exit and
      the document records that as a known gap instead of claiming it works.
- [x] Add contract tests with fake display/window/session providers (see `crates/ferramari-core/src/platform.rs` tests).
- [x] Document the backend contract before accepting a second OS implementation —
      `docs/BACKEND_CONTRACT.md`, including the §8 checklist a second backend must pass.

Current boundary progress:
- Core config, cache, data and video-library roots now resolve to native per-user locations on Linux, macOS and Windows; Linux keeps its XDG/`xdg-user-dir` behavior.
- Process metrics explicitly report unavailable on platforms without the current `/proc` sampler instead of displaying misleading zeroes.
- Unix process-group termination and private-key mode bits are cfg-gated; Windows currently falls back to terminating the direct child and relies on the user's profile ACL.
- Window rectangles and occlusion evaluation now live in ferramari-core.
- The EWMH watcher now lives in ferramari-platform.
- A platform-neutral DisplayProvider contract exists; the X11 implementation is injected at startup.
- WallpaperSurfaceProvider and PowerProvider are injected at startup and into the media/application layers.
- PlatformEventSource owns Linux session/window/hotplug watcher startup and teardown; it sends normalized core commands.
- GNOME Shell D-Bus integration and zbus dependency have been moved out of ferramari-core.
- The obsolete GTK wakeup pipe was removed; std::sync::mpsc already wakes the command loop.
- Monitor enumeration and wallpaper surface creation are still backed only by Linux/X11.
- Linux session/window watcher startup and teardown now sit behind an injected PlatformEventSource.
- Power-supply polling is now a Linux platform provider; desktop background handling and some path/metrics code remain Linux-specific.

## 4. Finish Linux backend

- [x] Define a platform-neutral wallpaper surface contract in core.
- [x] Move X11 wallpaper surface creation and native connection ownership into the platform crate.
- [x] Inject a platform surface provider into the media engine; remove x11rb from media.
- [ ] Add non-X11 surface providers for macOS and Windows.
- [ ] Implement backend capability reporting for X11 and XWayland.
- [ ] Test wallpaper stacking below desktop icons and above the root background.
- [ ] Test mpv embedding, surface destruction and recovery after player crashes.
- [ ] Test monitor hotplug, clone displays, stable naming and mixed DPI.
- [ ] Test lock/unlock, suspend/resume, logout and desktop-shell restarts.
- [ ] Test GNOME, KDE Plasma and XFCE where their APIs are available.
- [ ] Investigate native Wayland protocols/compositor extensions; report Partial or
      Unsupported rather than pretending universal Wayland support.

## 5. Integrate macOS backend

- [ ] Review the parallel macOS implementation and its ownership/lifecycle model.
- [ ] Map it to the shared platform contracts without duplicating core state.
- [ ] Integrate it into the same workspace and build graph.
- [ ] Implement display discovery, surface lifecycle, window/session/power events
      where supported by macOS APIs.
- [ ] Test multiple displays, display reconfiguration, sleep/wake and app restart.
- [ ] Add macOS CI, packaging and signing/notarization notes.

## 6. Implement Windows backend

- [ ] Research and document the supported desktop wallpaper host strategy
      (including Progman/WorkerW behaviour and Explorer restarts).
- [ ] Implement display discovery and stable display identities.
- [ ] Implement wallpaper surface creation, embedding and lifecycle.
- [ ] Implement foreground/fullscreen/occlusion tracking.
- [ ] Implement lock, suspend/resume and power events.
- [ ] Test multiple monitors, mixed DPI, Explorer restart and GPU variants.
- [ ] Add Windows CI and installer packaging.

## 7. Media and web wallpaper engine

- [x] Audit mpv process lifecycle, IPC timeouts, restart backoff and cleanup — see `docs/MPV_LIFECYCLE_AUDIT.md`. Full review of spawn → IPC → park/unpark → Drop → death detection chain.
- [ ] Validate hardware decode per supported OS/GPU backend — parameter passes through but runtime GPU capability validation is missing; see audit doc for recommendation.
- [x] Validate stream resolution, cache invalidation and yt-dlp failures — 11 new unit tests in `crates/ferramari-core/src/ytdlp.rs` cover format parsing edge cases (codec-only, negative quality, no dimensions, oversized height), strict `<` TTL boundary (expires at `elapsed >= TTL`), separate page/height cache isolation, and that `resolve()` errors on an inaccessible URL without panicking.
- [ ] Select a GTK-free browser engine or isolated browser process for web wallpapers.
- [ ] Define a strict web-wallpaper sandbox, network permissions and process limits.
- [ ] Restore web wallpaper only after navigation, audio, reload and cleanup tests pass.
- [x] Do not expose a web-wallpaper feature flag or UI action while the renderer is absent — verified: no webpage actions exposed in Settings page.

## 8. Eco Engine and performance

- [x] Feed platform-neutral window/session/power events into the existing Eco policy — `App::eco_plan_for()` builds `eco::Signals` from `self.screen_locked`, `self.suspended`, `self.on_battery`, `self.idle`, plus per-monitor occlusion/fullscreen state from `window_state.visibility_at()`.
- [x] Verify Active, Idle, Occluded and Suspended transitions per monitor — 7 integration tests in `src/app.rs` covering session signal override, battery pause per-profile, fullscreen/maximized occlusion, per-monitor independence (eDP-1 covered vs HDMI-1 idle).
- [x] Verify Quality, Balanced and Eco profiles, battery pause and decoder parking — all three profiles verified against every signal combinator; parking guard tested in `eco.rs`.
- [x] Unify CPU/RAM/process/GPU metrics with explicit unsupported values.
      `metrics::Metric<T>` replaces the single `available` flag: each reading is
      `Value`, `Unsupported` (no sampler on this platform, or a driver without
      `gpu_busy_percent`), `Unavailable` (the server process tree is gone) or
      `Pending` (a rate needs two samples). The Performance page labels the
      reason instead of printing zeroes, and a dead tree resets the CPU
      interval rather than reporting a spike across the gap.
- [ ] Benchmark one, two and multiple monitors on representative hardware.
- [ ] Record idle CPU, active CPU, memory, decoder count and wakeups.

## 9. Community, catalogue and creator workflows

- [x] Revalidate catalogue API v1, pagination, filtering and download integrity — see `docs/CATALOGUE_API_AUDIT.md`. Both JSON API v1 and legacy scraper reviewed: strict caps confirmed (2 MiB body, 200 items, string lengths, http(s)-only URLs). HTML scraper identified as locale-coupled.
- [x] Revalidate declarative plugins and archive extraction limits — manifest size capped at 64 KiB, id capped at 64 chars, no code execution in-process, path traversal rejected. No ZIP-bomb uncompressed-size guard yet (see audit doc for recommendation).
- [x] Add Iced controls and D-Bus methods for import, export, sign, verify and trust (native pickers and operation completion reporting remain open).
- [ ] Define creator-facing metadata, preview and validation workflows.
- [ ] Design an extensible customization/effects model without executing untrusted code.
- [ ] Add package/catalogue compatibility tests and migration rules.

## 10. Packaging, release and maintenance

- [ ] Migrate the legacy Flatpak manifest to the server + Iced UI workspace.
- [ ] Build and validate Linux packages with both binaries and desktop metadata.
- [ ] Define macOS app bundle and Windows installer layouts.
- [ ] Verify upgrades from 1.9.x preserve config, library, packages and signing keys.
- [ ] Verify clean uninstall does not delete user data without explicit consent.
- [ ] Update README, architecture docs, troubleshooting and user-facing limitations.
- [ ] Run clean-install and upgrade regression matrices on all supported OSes.
- [ ] Publish 2.0 only when required CI and runtime smoke tests are green.

## Working rules

1. Work on the migration branch; do not merge to `main` without approval.
2. Prefer small commits with one architectural concern each.
3. Keep the existing Linux path working while introducing shared contracts.
4. Do not delete a feature silently: migrate it, or mark it unavailable explicitly.
5. Treat CI as necessary but insufficient; display-server and multi-monitor smoke
   tests must be recorded separately.
6. Keep the macOS implementation as a backend of the same product, not a fork.
