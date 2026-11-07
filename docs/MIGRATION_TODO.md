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
- [ ] Stage-install both binaries and verify their sibling lookup and data paths.
- [ ] Smoke-test server startup, UI launch, D-Bus reconnect and clean shutdown.
- [ ] Test old config/library migration and preserve existing user data.
- [ ] Verify the default install has no GTK/GDK/WebKitGTK dynamic linkage.

## 2. Restore Iced feature parity

- [ ] Inventory every action and setting in the last GTK release against the Iced UI.
- [ ] Complete local library actions: add, remove, refresh, thumbnails and monitor assignment.
- [ ] Complete favourites and collections, including rotation assignment and persistence.
- [ ] Add native file/folder dialogs using portal-backed Linux dialogs and native macOS/Windows dialogs.
- [ ] Expose package import and export through the server D-Bus API and Iced UI.
- [ ] Expose package signature verification and explicit trust-key actions in Iced.
- [ ] Add progress, success and actionable error states for long-running operations.
- [ ] Restore all playback, hardware decoding, rotation, power and session settings.
- [ ] Restore autostart and tray controls with clear capability reporting.
- [ ] Audit catalogue browsing, download, preview and pagination against the old UI.
- [ ] Validate English, Russian and Japanese strings and layout overflow.
- [ ] Add UI/client tests for disconnects, reconnects, command failures and stale snapshots.

## 3. Define and enforce the platform boundary

- [ ] Define stable platform-neutral contracts for displays, wallpaper surfaces,
      window tracking, session state, idle state and power events.
- [ ] Add explicit capability states: Supported, Partial and Unsupported.
- [ ] Keep native handles and native API types inside platform/media adapters.
- [ ] Ensure core models and policies do not import X11, Win32, Cocoa, D-Bus or
      toolkit types.
- [ ] Give displays stable persistent IDs while preserving legacy config keys.
- [ ] Specify event ordering, thread affinity, cancellation and backend shutdown.
- [ ] Add contract tests with fake display/window/session providers.
- [ ] Document the backend contract before accepting a second OS implementation.

Current boundary progress:
- Window rectangles and occlusion evaluation now live in ferramari-core.
- The EWMH watcher now lives in ferramari-platform.
- Monitor enumeration and wallpaper surface creation are still Linux/X11-specific.
- The main application still directly orchestrates Linux session services.

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

- [ ] Audit mpv process lifecycle, IPC timeouts, restart backoff and cleanup.
- [ ] Validate hardware decode per supported OS/GPU backend.
- [ ] Validate stream resolution, cache invalidation and yt-dlp failures.
- [ ] Select a GTK-free browser engine or isolated browser process for web wallpapers.
- [ ] Define a strict web-wallpaper sandbox, network permissions and process limits.
- [ ] Restore web wallpaper only after navigation, audio, reload and cleanup tests pass.
- [ ] Do not expose a web-wallpaper feature flag or UI action while the renderer is absent.

## 8. Eco Engine and performance

- [ ] Feed platform-neutral window/session/power events into the existing Eco policy.
- [ ] Verify Active, Idle, Occluded and Suspended transitions per monitor.
- [ ] Verify Quality, Balanced and Eco profiles, battery pause and decoder parking.
- [ ] Unify CPU/RAM/process/GPU metrics with explicit unsupported values.
- [ ] Benchmark one, two and multiple monitors on representative hardware.
- [ ] Record idle CPU, active CPU, memory, decoder count and wakeups.

## 9. Community, catalogue and creator workflows

- [ ] Revalidate catalogue API v1, pagination, filtering and download integrity.
- [ ] Revalidate declarative plugins and archive extraction limits.
- [ ] Restore package workflows in Iced: import, export, sign, verify and trust.
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
