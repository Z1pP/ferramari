# Testing matrix

Ferramari is a desktop application, so CI can only cover the parts that do not
need a live session. This document contains historical live-session results
for the 1.9.x GTK build. They are not evidence that the 2.0.0 Iced migration
has passed equivalent smoke tests. The 2.0 workspace must be tested separately
using the checklist below; do not carry forward old “tested” labels as 2.0
certification.

## Environment matrix

| Environment | Status | Notes |
| --- | --- | --- |
| GNOME 50, Wayland session, running through XWayland | **tested on 1.9.2 only** 2026-11-02 | Historical GTK build; 2.0 Iced UI not yet smoke-tested |
| GNOME 50, X11 session | not tested | The same X11 backend is used, but the session has not been exercised |
| KDE Plasma, X11 | not tested | Desktop-window placement and tray integration differ |
| KDE Plasma, Wayland | not tested | XWayland path only |
| GNOME with the Flatpak bundle | **tested** 2026-10-31, 0.7.0 | Built and launched from a local bundle; host mpv/ffmpeg bridge, XWayland, WebKitGTK; 0.8 uses the same manifest; tray/autostart not verified |
| Native Wayland (no XWayland) | not supported | No wallpaper surface or `--wid` embedding yet |

Do not mark an environment as supported until the checklist below has been run
there and the result is recorded in this table.

## Smoke checklist

Build the complete 2.0 workspace with `cargo build --workspace --release --locked`, then run from the checkout or an installed build. Several UI-specific checklist items below describe the former GTK UI and need explicit re-verification against Iced.

### Startup and shutdown
- [ ] App starts with no existing config and shows the Home page.
- [ ] A second launch only asks the first instance to show its window.
- [ ] Quit from Settings and from the tray; no `mpv`, `ffmpeg` or `ffprobe`
      processes remain.
- [ ] SIGTERM during playback shuts down cleanly.

### Theme and appearance
- [ ] Settings → System → Theme switches between System default, Light and Dark.
- [ ] The window, backdrop, popovers and dialogs all follow the change immediately.
- [ ] Restart the app; the chosen theme is still applied.
- [ ] Choose System default with a light and with a dark desktop setting; the
      app follows the desktop.
- [ ] Put an invalid value in `config.json`; the app falls back to System default.

### Local video
- [ ] Pick a 1080p video from Local Video and apply it to one monitor.
- [ ] Apply another video to another monitor; the first keeps playing.
- [ ] Reconnect/disconnect a monitor; its source is restored on reconnect.
- [ ] Change the resolution or scale factor; the window follows the monitor.
- [ ] Kill `mpv` with `SIGKILL`; playback is recreated after the backoff.
- [ ] Repeat the kill until the give-up limit; no endless restart loop and a
      visible error is shown.

### Iced library and favourites
- [ ] Add a recently used video to favourites; it appears in the Favorites
      section without restarting the UI.
- [ ] Remove it from favourites; it disappears from the section.
- [ ] Restart Ferramari; the favourites list is preserved in library.json.
- [ ] Play a favourite; the selected video is applied through the D-Bus server.

### Static wallpaper and desktop background
- [ ] Enable static wallpaper; a blurred frame appears and
      `background-original.json` records the previous background.
- [ ] Disable it; the recorded original is restored by name in the log.
- [ ] Delete the recorded original file, restart, disable the static wallpaper:
      the app must not point GNOME at a missing file.
- [ ] Point `picture-uri` at a frame in `$XDG_CACHE_HOME/ferramari/static`,
      restart: the app treats it as its own and heals it with a real frame.
- [ ] Restart twice with the static wallpaper on; the background URI must name
      a file that exists both times.

### Static wallpaper
- [ ] Enable static wallpaper in Settings; a blurred frame appears in GNOME.
- [ ] Change the blur radius; the frame is regenerated off the UI thread.
- [ ] Disable it; the original background is restored.
- [ ] Quit; the original background is restored and stale frames are removed.

### Web and streaming
- [ ] Web-page wallpapers are currently unavailable in 2.0.0; confirm the UI and server show the explicit unsupported-engine error without crashing.
- [ ] When a replacement web engine is integrated, restore the web-page rendering, security-policy and reload checks from the 1.9.x test baseline.
- [ ] Resolve a streaming URL with `yt-dlp`; play/pause and mute work.
- [ ] Press Reload in stream mode twice within 15 minutes: the wallpaper comes
      back in well under a second, `[Stream] Reusing the cached resolution`
      appears in the log, and no new `yt-dlp` process starts.
- [ ] Kill the player until the restart policy gives up, then press Reload:
      `yt-dlp` runs again (`[yt-dlp] Resolving`) instead of replaying the old
      link.

### Performance and idle
- [ ] A cold local-video grid shows cards immediately; only two thumbnail
      workers run at once and stale results do not replace a newer grid.
- [ ] Opening Local Video a second time does not start ffmpeg for cached
      thumbnails.
- [ ] The static wallpaper appears within ~1 s on the first run and almost
      instantly on the next start (frame cache).
- [ ] Drag the volume slider: playback follows immediately, and
      `[Config] Saving` appears once after the drag.
- [ ] Quit during the debounce window; the last volume is still saved.
- [ ] Idle voluntary context switches are ~1–2/s, not ~25/s.
- [ ] Switch between two local videos on one monitor; the mpv PID stays the
      same and the new file plays.
- [ ] Switch to a missing or corrupt file; an error is shown or the old file
      keeps playing, and the app stays responsive.

### Layout and localization
- [ ] With the interface language set to Russian, every page fits the window:
      no caption, switch or combo is cut off at the right edge.
- [ ] Resize the window from its minimum to a wide size; the Local Video header
      moves its buttons to a second row and back without jitter.
- [ ] Repeat in English. Japanese was not exercised: the `ja` locale is not
      installed on the test machine, so `setlocale` falls back to `C` and the
      app renders English.

### Package integrity and signing
- [ ] `ferramari package key` creates `signing-key.json` (mode 600) once and
      reuses it on the next call.
- [ ] `ferramari package sign` writes `checksums.json` and `signature.json`.
- [ ] `ferramari package verify` exits 0 for a trusted package, 1 for unsigned
      or unknown author, 2 for a tampered file or a broken signature.
- [ ] Editing any file after signing makes verify report the SHA-256 mismatch.
- [ ] Editing `checksums.json` makes the signature invalid.
- [ ] Replacing the embedded `public_key` is reported as invalid, not trusted.
- [ ] `ferramari package trust <package>` and `… trust <key.json>` both add the
      key; `ferramari package keys` lists it.
- [ ] A second user sees "not trusted" until they trust the key, then "trusted".
- [ ] Export signs automatically when a key exists; unsigned export still works.
- [ ] Local Video → Select a package video → Verify shows the report dialog;
      an unknown author offers to trust the key.
- [ ] Settings → Package signing shows the key id, trusted keys, and can export
      a public key and trust a key file.
- [ ] A package without `checksums.json` still imports and plays.

### Catalogue
- [ ] Switch categories, search and Show more.
- [ ] Open a card, download, inspect the six frames, install and apply.
- [ ] Cancel a download; no partial cache file remains.
- [ ] Sort by title/category/size/duration.
- [ ] Filter by resolution; unknown items disappear rather than being guessed.
- [ ] Remove a download; the cache entry is gone.
- [ ] Go offline; local library still works and the catalogue shows an error.

### Library
- [ ] Mark/unmark a favourite; the Favorites view follows immediately.
- [ ] Apply videos; Recent shows the last used first.
- [ ] Create a collection, rename it, add/remove items and delete it.
- [ ] Switch between All, Favorites, Recent and collections.
- [ ] Assign a collection to a monitor on Home.
- [ ] Set rotation to In order and a short interval; the monitor advances.
- [ ] Set rotation to Random; it does not immediately repeat the same item.
- [ ] Restart the app; library.json survives and is valid.

### Power profiles and Eco Engine
- [ ] Select Quality, Balanced and Eco; each restarts playback to apply the mpv flags.
- [ ] Eco adds frame dropping, single decoder thread and bilinear scaling.
- [ ] With "Pause on battery" enabled on battery, playback pauses; connecting AC resumes it.
- [ ] Battery percentage/AC state appears in Settings.
- [ ] On two monitors, open a fullscreen app on monitor 1: monitor 1 pauses and monitor 2 keeps playing.
- [ ] Close the fullscreen app: monitor 1 resumes without restarting monitor 2.
- [ ] Set the Eco idle delay to 1 minute; leave Balanced idle: playback continues at the 30 fps cap.
- [ ] With Eco and a 1-minute delay, leave the desktop idle: playback pauses; moving the mouse resumes it.
- [ ] Home monitor cards show Playing / Idle / Paused (covered) / Paused for the matching state.
- [ ] In `-d` logs, check `[Eco] <monitor>: Active/Occluded/Idle/Suspended` transitions and the fps value.
- [ ] `gdbus ... Properties.Get ... monitor_states` returns one entry per monitor.
- [ ] Cover the monitor (or lock it) with a local video paused for five minutes: the log shows `parked the decoder`, the mpv PID stays the same and its RSS drops.
- [ ] Uncover/unlock: playback resumes at the saved position and the log shows `decoder unparked`.
- [ ] Pause playback explicitly (tray/D-Bus) with the desktop visible for five minutes: no parking happens and the last frame remains.
- [ ] Change the video while the monitor is parked: the new file starts normally, not at the old position.

Recorded 2026-11-01 on GNOME/XWayland: with the threshold temporarily set to
10 s, a fullscreen X11 window produced `Occluded`, the decoder parked at
`time-pos=12.766667`, the mpv PID stayed the same, closing the window produced
`Active` + `decoder unparked`, and a visible user pause did not park again.
The shipped threshold is five minutes; the test only shortened the timer.

### `.ferramari` packages
- [ ] Export a selected video; a `<name>.ferramari/` directory appears with `manifest.json` and `wallpaper.<ext>`.
- [ ] Import that package; it is copied to the data directory and applied to all monitors.
- [ ] Import a web package; it becomes the current web wallpaper.
- [ ] Edit `entry` to `../x.mp4` or add a symlink; import fails with a clear message and no partial directory remains.
- [ ] Set `engine_min` to a future version; import fails.
- [ ] Import a package with a 3 GiB file; it is rejected (do not commit the fixture).

### Ecosystem
- [ ] Place a valid `plugin.json` under `$XDG_DATA_HOME/ferramari/plugins/<id>/`; `--doctor` and Settings list it.
- [ ] Use an invalid id, a `file://` API or `engine_min: 99`; the plugin is skipped and the app still starts.
- [ ] `ferramari --catalog <base>` prints page/next/items for a compatible server and exits non-zero on invalid JSON or HTTP errors.
- [ ] A catalogue item with a `file://` download is rejected.

### Performance and hardening
- [ ] Open Performance: CPU, RSS, threads, processes update every ~2 s; GPU shows a value on AMDGPU and "Unavailable" otherwise.
- [ ] Close the control window: no resource sampling runs (check CPU in `top`).
- [ ] Select Off for hardware decoding; mpv starts with `--hwdec=no` (visible in `-d`).
- [ ] Disable network for web wallpapers and apply a remote URL: a clear error appears instead of a blank page.
- [ ] Apply a local HTML file with network disabled: it loads, and no pop-up window opens.
- [ ] `make bench VIDEO=... SECONDS=5` prints startup/CPU/RAM/process/GPU rows.
- [ ] `make bench MODE=web WEB=... SECONDS=5` measures a web wallpaper with no mpv process.
- [ ] `make bench VIDEO=/path/to/4k.mp4 LABEL=4k SECONDS=5` records the 4K row.

### Cross-desktop diagnostics
- [ ] `ferramari --doctor` prints the session, backend, desktop and tool versions without starting the UI.
- [ ] On a Wayland (XWayland) session it reports `per-monitor occlusion: yes`.
- [ ] On X11 it reports `per-monitor occlusion: yes`.
- [ ] Settings → System → Desktop integration shows the same backend summary.
- [ ] Lock via logind and via the screensaver in either order; playback resumes only after both report unlocked.

### System integration
- [ ] Screen lock pauses and unlock resumes.
- [ ] Maximized/fullscreen window pauses only the covered monitor (X11).
- [ ] Autostart toggle creates/removes the XDG autostart entry.
- [ ] Tray menu (Show, mute, pause, reload, quit) works.
- [ ] D-Bus `video`, `volume`, `reload_config` and `quit` work.
- [ ] Language switch applies after restart.
- [ ] Corrupt `config.json`; the app starts from `config.json.bak`.

## Reproducible local run

```sh
# Keep GSettings writes away from the real desktop
GSETTINGS_BACKEND=memory cargo run -- -d

# Isolated config/cache for repeatable tests
XDG_CONFIG_HOME=/tmp/ferramari-test/config \
XDG_CACHE_HOME=/tmp/ferramari-test/cache \
GSETTINGS_BACKEND=memory cargo run -- -d
```

The X11/XWayland path is the supported one in 1.0. A GNOME Wayland session is
tested through XWayland (`GDK_BACKEND=x11` is set by the application).

## Flatpak status

The manifest is in `flatpak/io.github.ferramari.Ferramari.yml`. It is a
self-hosted-bundle manifest, not a Flathub submission yet. The build needs the
GNOME SDK and Rust extension; see `flatpak/README.md`. Until the checklist has
been run inside the sandbox, the Flatpak row stays **not tested**.