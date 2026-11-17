# Testing matrix

Ferramari is a desktop application, so CI can only cover the parts that do not
need a live session. This document contains historical live-session results
for the 1.9.x GTK build. They are not evidence that the 2.0.0 Iced migration
has passed equivalent smoke tests. The 2.0 workspace must be tested separately
using the checklist below; do not carry forward old “tested” labels as 2.0
certification. Windows has its own checklist at the end; a green Windows CI
job proves the build and unit tests, not wallpaper playback.

## Environment matrix

| Environment | Status | Notes |
| --- | --- | --- |
| GNOME 50, Wayland session, running through XWayland | **tested on 1.9.2 only** 2026-11-02 | Historical GTK build; 2.0 Iced UI not yet smoke-tested |
| GNOME 50, X11 session | not tested | The same X11 backend is used, but the session has not been exercised |
| KDE Plasma, X11 | not tested | Desktop-window placement and tray integration differ |
| KDE Plasma, Wayland | not tested | XWayland path only |
| GNOME with the Flatpak bundle | **tested** 2026-10-31, 0.7.0 | Built and launched from a local bundle; host mpv/ffmpeg bridge, XWayland, WebKitGTK; 0.8 uses the same manifest; tray/autostart not verified |
| Windows 11 23H2, Explorer desktop | **partial** 2026-11-17 | Release build, full unit suite, `--doctor`, live display/desktop/session probes and one Explorer-hosted surface passed; live 1080p playback behind the icons with mpv 0.42, ffmpeg 9.0.2 and yt-dlp 2026.10.26 (`d3d11va`), plus automatic mpv recreation after a kill, Explorer-restart recovery, tray re-registration on `TaskbarCreated`, second-launch focus, tray commands, autostart and static-wallpaper toggles. A later pass fixed the IPC reader stall and verified live switching, pause/resume, mute and advancing playback, the embedded icon font, and ru/ja counted nouns in light/dark themes at 1180 and 900 px. The 2026-11-16 pass moved each surface onto its own message-pumping thread, which cut a wallpaper change from a 105 s command-loop stall to about 0.5 s, and verified bidirectional video switching through the control surface's own `video` command. The 2026-11-17 pass added live Windows CPU/RAM/thread/process sampling (GPU stays `N/A`), close-to-tray with automatic tray enablement and hidden-window raise, and the embedded crab icon in both executables, the window, the tray and the installer. Multi-monitor and mixed-DPI scenarios are unchecked |
| Windows 10/11 with the installer | **partial** 2026-11-14 | Per-user silent install, Start-menu launch of both binaries and mpv, `--doctor`, ru/ja catalogs, portable ZIP from a path with a space and silent uninstall preserving user data passed on one Windows 11 account; not yet run on a clean machine or through an upgrade |
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

### Iced library, favourites and collections
- [ ] Add a recently used video to favourites; it appears in the Favorites
      section without restarting the UI.
- [ ] Remove it from favourites; it disappears from the section.
- [ ] Restart Ferramari; favourites are preserved in library.json.
- [ ] Create, rename and delete a collection; each change persists after restart.
- [ ] Add a recent video to a collection, play it from the collection, then
      remove it from that collection.
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

## Windows smoke checklist

Run on a Windows 10/11 desktop with Explorer, at least one monitor, and mpv,
ffmpeg/ffprobe and yt-dlp installed. Start from the release build or the
installer; use a disposable Windows account for the first run so the original
wallpaper sidecar and autostart key can be removed without touching a real
profile.

### Startup, control and shutdown
- [x] `ferramari --doctor` prints `session: Windows desktop`, the Explorer
      host, display identities, tool versions and capability levels without
      starting a window.
- [x] `ferramari` starts the server and opens `ferramari-ui`; both `.exe`
      files are in the same folder and the UI finds the endpoint file.
- [x] A second `ferramari` launch asks the first instance to show the UI and
      exits; only one server process remains. Verified with the control window
      minimized: the second launch restored and raised it (the log shows
      `[GUI] Raised the existing control window`) and exited in about one
      second.
- [x] Closing the control window on Windows hides it instead of ending the
      process: the title-bar X, the native close request, a second launch and
      the tray's Show all reach the same window. Verified 2026-11-17: closing
      with the "Run in background" switch off turned the switch on, registered
      the tray icon and left `ferramari-ui` alive; a tray left-click restored
      the hidden window, and the UI process exited when the hidden window's
      server went away.
- [x] Quit from the tray; no `mpv`, `ffmpeg` or `ffprobe` child remains and the
      original wallpaper is restored.
- [ ] Quit from Settings; no `mpv`, `ffmpeg` or `ffprobe` child remains.
- [ ] Ctrl-C, Ctrl-Break, console close, logoff and shutdown each end the
      server cleanly; the original wallpaper is restored.
- [x] A stale `control-endpoint.json` from a killed server is replaced on the
      next start and does not block the UI. A file pointing at a closed port
      was replaced with the new port and token, and the UI connected.
- [x] Both executables expose the crab icon through `ExtractAssociatedIcon`;
      the control window answers `WM_GETICON` with a small icon and the tray
      icon loads resource ID 1 from the running module.

### Wallpaper host and Explorer restarts
- [x] Apply a local video to one monitor; it appears behind the desktop icons
      and the icons stay clickable.
- [x] Switch videos through the control surface's own `video` command, with
      and without a monitor name; the new file plays and the desktop frame is
      replaced. Verified 2026-11-16 both ways (`Power-Dancing-1080p.mp4` ↔
      `arcane-jinx.mp4`): the matching static frame was installed in
      0.34–0.51 s, Explorer replaced its worker, a fresh mpv started in about
      2.5 s, and two desktop captures three seconds apart differed at 30k+
      sample points after every switch.
- [x] Send `WM_NULL` and `SetWindowPos` to the surface from another process;
      both return in under a millisecond because the surface owns a
      message-pumping thread. Before the thread, the same calls hung until
      their timeout.
- [x] Restart Explorer (`taskkill /F /IM explorer.exe` then start it again):
      playback is recreated within the restart backoff and the new surface is
      attached to the new `WorkerW` or to `Progman` when Explorer keeps the
      desktop there. Verified twice: the host loss was reported one second
      after the kill, failed rebuilds kept retrying while the shell was down,
      and a fresh mpv with a new `--wid` resumed playback. Icons stayed on top
      and clickable after they finished loading.
- [x] Kill `mpv`; the supervisor recreates it and the surface stays alive.
- [x] Change the desktop wallpaper in Windows Settings while Ferramari is
      playing; the live wallpaper is unaffected and the original record is not
      overwritten by a generated frame. Verified 2026-11-16: changing to
      `img24.jpg` during playback left the video running (32k+ differing
      sample points) and did not touch the sidecar.
- [x] Change the wallpaper while static wallpaper is off, then quit; the chosen
      image stays and `background-original.json` records it. Verified
      2026-11-17 with `img24.jpg`.
- [x] Change the wallpaper while a frame is installed, then switch videos; the
      new frame is installed and the chosen image becomes the recorded
      original. Verified 2026-11-17: `[StaticWallpaper] Remembering ...` ran
      before `Applied`, and quitting restored `img25.jpg`.
- [x] Close Ferramari and confirm the user's original desktop image is back.

### Displays and DPI
- [ ] Two monitors at different scales: each surface covers its own monitor
      with no offset, stretching or gap.
- [ ] Move the primary-display setting and change resolution; both surfaces
      follow without a restart.
- [ ] Disconnect and reconnect a monitor; its configured source is restored
      and the other monitor keeps playing.
- [ ] Rotate a display; the surface geometry follows the rotation.
- [ ] Unplug the monitor whose video is playing; no error dialog storm and the
      remaining monitor continues.

### Static wallpaper, tray and autostart
- [x] Enable static wallpaper; a blurred frame appears and
      `%APPDATA%\ferramari\background-original.json` records the previous path.
- [x] Disable it; the recorded original is restored. Verified with the real
      theme image (`c:\windows\web\wallpaper\themeb\img25.jpg`), then enabled
      again to apply the generated frame.
- [x] Delete the recorded original file and disable again; no missing-file
      background is installed. Verified 2026-11-16: the server logged
      `[StaticWallpaper] Keeping the last frame` and left the generated frame
      on the desktop; the missing path was not installed.
- [x] Tray menu: Show, mute, pause, reload, lucky and quit all act on the
      running server; the icon disappears when “Run in background” is off.
      Verified by posting the menu command ids to the tray window: mute and
      pause toggled, reload recreated the player, lucky logged `[Lucky]`, Show
      restored and raised the minimized control window, and quit stopped the
      server and restored the wallpaper. The popup itself was not clicked.
- [x] Restart Explorer while the tray is enabled; the icon is re-added when
      the shell broadcasts `TaskbarCreated`. The Explorer started manually in
      this test session did not broadcast that message (the shell also did not
      auto-restart), so the handler was verified by sending the registered
      `TaskbarCreated` id to `HWND_BROADCAST`; the icon was re-added and the
      server logged `[Systray] Restored the icon after an Explorer restart`.
- [x] Autostart creates `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
      value `Ferramari` with a quoted `-b` command; disabling removes it.
- [ ] Sign out and back in; the wallpaper starts in background mode without
      opening the control window.

### Performance and Eco events
- [x] Open Performance: CPU, RAM, threads and processes show live values for
      the server tree; GPU reports `N/A` on Windows because no portable counter
      is wired up. Verified 2026-11-17 with a 1080p VP9 video: CPU 6%, RAM
      0.2 GB of 16 GB (1%), 48 threads and 3 processes; the sparkline advanced
      once per second.
- [ ] Lock the session (Win+L); playback pauses, and unlock resumes it.
- [ ] Sleep/resume; playback resumes without restarting the process.
- [ ] Unplug/plug the charger on a laptop; the AC/battery state changes and
      “Pause on battery” acts without waiting for the 30 s poll.
- [ ] Cover a monitor with a maximized or fullscreen window; only that monitor
      pauses. Close it; that monitor resumes.
- [ ] Leave the desktop idle past the Eco delay; playback pauses and resumes
      on input.
- [ ] A UWP window on another virtual desktop does not count as occlusion.

### Installer and packaging
- [x] `installer\build.ps1` produces a setup `.exe` and a portable `.zip` in
      `installer\Output`.
- [x] The setup program and its shortcuts use
      `data/icons/windows/ferramari.ico`; the embedded icon is present in both
      installed executables and the uninstall entry points at `ferramari.exe`.
      Verified 2026-11-17 by a silent per-user install: the setup, Start-menu
      shortcut and installed `ferramari-ui.exe` all returned the crab through
      `ExtractAssociatedIcon`; silent uninstall removed the install directory
      and shortcuts while `%APPDATA%\ferramari` and `%LOCALAPPDATA%\ferramari`
      stayed intact.
- [x] Install per-user without elevation; Start-menu shortcut starts both
      binaries and the UI is translated (ru/ja) when the locale is selected.
- [x] Uninstall removes binaries, shortcuts and locale files but leaves
      `%APPDATA%\ferramari` and `%LOCALAPPDATA%\ferramari` intact.
- [x] Extract the portable ZIP to a folder with a space in its path; both
      binaries start and find their locale catalogs.

Record the Windows build number, GPU, monitor layout and mpv/ffmpeg versions
next to the results. Do not mark the Windows row in the environment matrix as
`tested` until every section above has been run.

Recorded 2026-11-12 on Windows 11 23H2, one 1920×1080 display at 125%,
Explorer, with mpv 0.42.0, ffmpeg/ffprobe 9.0.2 and yt-dlp 2026.10.26:
`cargo test --workspace` passed (14 binary + 344 core + 17 media + 49 platform
+ 8 UI library + 6 UI binary tests), the release workspace produced both `.exe`
files, and `ferramari --doctor` reported the Win32 backend, one `DISPLAY1` with its
monitor device interface, Explorer wallpaper host = yes, every capability
supported, and all four tool versions. A live background server answered
`ping`, returned a complete `snapshot`, accepted `quit`, exited cleanly and
removed `control-endpoint.json`; ten immediate snapshot requests during
startup all succeeded. A local 1080p video then played behind the desktop
icons (`is_playing=true`, `d3d11va`); `WindowFromPoint` over an icon returned
`SysListView32`, so the icons stay on top and clickable. `paths::tool` found
mpv at `C:\Program Files\MPV Player\mpv.exe` while `mpv` was absent from
`PATH`. Killing that process made the supervisor spawn a new mpv after the
one-second backoff, attach a fresh surface and resume playback. Killing
Explorer twice produced the expected sequence: `[Player] DISPLAY1 lost its
wallpaper host; rebuilding in 1s`, one or two rebuild attempts rejected while
`Progman` was still missing, then a new mpv with a new `--wid` and
`is_playing=true`. The surface uses the `Progman` fallback in this session
because Explorer keeps `SHELLDLL_DefView` there; desktop icons finished
loading and stayed clickable over the video. The tray is disabled in the
profile used for the run; enabling it spawned the icon (`[Systray] Ready`),
and a `TaskbarCreated` broadcast re-added it after the notification area was
rebuilt. Quitting the server restored the recorded original wallpaper. The
installer was not run during this first pass; it was built and exercised on
2026-11-14 (below). This is a partial result, not the full smoke matrix.

Recorded 2026-11-13 on the same machine: a stale `control-endpoint.json`
pointing at a closed port was replaced on startup and the UI connected. A
second `ferramari` launch exited in about one second and restored and raised
the minimized control window (`[GUI] Raised the existing control window`).
Tray command ids toggled mute and pause, reload recreated the player, lucky
logged `[Lucky]`, Show raised the minimized window, and quit stopped the
server, removed the endpoint and restored the wallpaper. Autostart wrote the
quoted `-b` command to the `Run` key and removed it again. Enabling static
wallpaper applied the generated frame and disabling restored
`c:\windows\web\wallpaper\themeb\img25.jpg`. Synthetic `WM_WTSSESSION_CHANGE`
and `WM_POWERBROADCAST` messages produced `[ScreenLock] active=true/false` and
`[Power] Suspend=true/false`; a real Win+L lock, sleep/resume and sign-out were
not exercised in this session.

Recorded 2026-11-14 on the same machine, with Inno Setup 6.7.3 and gettext
1.0+iconv 1.19 installed per user through winget: `installer\build.ps1 -NoBuild`
found both tools under `%LOCALAPPDATA%\Programs`, compiled the ru/ja catalogs
with `msgfmt --check` and produced a 9.1 MB setup executable and a 10.5 MB
portable ZIP. A silent per-user install into `%TEMP%\ferramari-install-test`
placed both executables, README, LICENSE and the locale catalogs and created
the Start-menu shortcuts; the installed `ferramari --doctor` reported the Win32
backend and every capability supported. Launching the Start-menu shortcut
started `ferramari`, `ferramari-ui` and mpv; a `quit` request over the control
endpoint stopped the server and mpv, and closing the control window ended the
UI. The portable ZIP extracted to a path containing a space and its
`ferramari.exe --doctor` ran. Silent uninstall removed the installation
directory and shortcuts and left `%APPDATA%\ferramari\config.json`
byte-identical. The UI was opened with `LANGUAGE=ru` and `LANGUAGE=ja` against
the compiled catalogs: the rail, page titles, Settings sections and their
descriptions were Russian and Japanese with no clipped labels. The offline
banner was the only English text in those runs; it now uses the translated
“Daemon offline” string.

Recorded 2026-11-15 on the same machine: the playback stall was traced to
mpv's IPC server blocking in `WriteFile` when the client stopped draining
events between commands. After the reader thread and liveness flag landed, a
live control session switched `Power-Dancing-1080p.mp4` → `arcane-jinx.mp4`,
reported the new `path` and a `duration` of 29.77 s, paused and resumed
(`pause=true`/`false`), set `mute=true` through `is_mute`, and showed
`time-pos` advancing (3.67 → 5.70 s) with snapshot `position` moving. The
same run confirmed the embedded Font Awesome face draws every rail, page and
action glyph with no system font, `1 монитор` and `1 台` in the compiled
ru/ja catalogs, and the monitor-card rotation picker fitting
`ローテーションなし`. Screenshots were checked in dark and light themes in
English, Russian and Japanese at 1180×780 and at the 900×620 minimum width.
Occlusion still reports `Occluded` when a maximized window covers the
display; with `is_pause_when_maximized=false` playback stays active. The
earlier application-loop freeze did not recur; trace markers around
`watch_players`, rotation, Eco and snapshot publishing are in place for the
next occurrence. Real lock, sleep/resume, sign-out and multi-monitor scenarios
remain unchecked.

Recorded 2026-11-16 on the same machine: the static-frame path was traced to
`SPI_SETDESKWALLPAPER` waiting about 105 s for the surface window's thread,
which was the command loop itself and was inside the call. Each surface now
has a dedicated message-pumping thread. A debug server then switched
`arcane-jinx.mp4` → `Power-Dancing-1080p.mp4` → `arcane-jinx.mp4` →
`Power-Dancing-1080p.mp4` through the control endpoint's `video` command, both
with an explicit `DISPLAY1` argument and with the empty monitor string the
control surface sends. Every switch logged `[StaticWallpaper] Applied`
0.34–0.51 s after `[Mode]`, the expected `[Player] DISPLAY1 lost its wallpaper
host` / `stopped` rebuild followed, and a new mpv with a new `--wid` started
about 2.5 s after the command. Two desktop captures three seconds apart
differed at 30k+ sample points after each switch, and the registry wallpaper
named the frame for the selected file. `SendMessageTimeoutW(WM_NULL)` and
`SetWindowPos` from a PowerShell process to the surface returned in 0 ms.
`cargo test --workspace` passed 439 tests (15 binary + 344 core + 17 media + 49
platform + 8 UI library + 6 UI binary), including the new
`a_static_frame_matches_the_primary_monitors_source`. The original source and
wallpaper were restored at the end.

Recorded 2026-11-16/17 on the same machine: the wallpaper-preservation path was
verified in both static modes. With static wallpaper on, changing the desktop
to `img25.jpg` during playback was remembered when the next video switch
installed a frame (`[StaticWallpaper] Remembering the wallpaper set while
running: ...img25.jpg` before `Applied`), and quitting restored `img25.jpg`.
With static wallpaper off, changing to `img24.jpg` and quitting kept
`img24.jpg` and rewrote `background-original.json`; before the fix the same
sequence restored `img25.jpg`. Deleting the recorded original and disabling
static wallpaper logged `[StaticWallpaper] Keeping the last frame` and
installed no missing path. Synthetic `WM_WTSSESSION_CHANGE` and
`WM_POWERBROADCAST` messages moved `monitor_states` between `suspended` and
`active`; a synthetic `WM_QUERYENDSESSION` answered 1, `WM_ENDSESSION` quit
cleanly with no server, mpv or endpoint left, and closing the console window
did the same. A clean launch then switched `Power-Dancing-1080p.mp4` →
`arcane-jinx.mp4`; the control window stayed visible, foreground and
hit-testable at its centre and both top corners after the change, so the
earlier z-order observation did not reproduce. The installer and portable ZIP
were rebuilt after the fix: silent install, upgrade in place (README and both
binaries replaced, two shortcuts, one uninstall entry), silent uninstall
(install directory and shortcuts gone, config byte-identical), and the
portable ZIP from a path with a space all passed. `cargo test --workspace`
passed 443 tests (15 binary + 344 core + 17 media + 53 platform + 8 UI library
+ 6 UI binary).

Recorded 2026-11-17 on the same machine: the Performance page showed live CPU
(6%), RAM (0.2 GB of 16 GB, 1%), 48 threads and 3 processes for the server
tree while a 1080p VP9 video played; GPU stayed `N/A` as designed. Closing the
control window with `WM_CLOSE` left `ferramari-ui` alive and enabled the tray;
the tray's Show command and a second launch raised the same hidden window, and
a hidden window exited after the server was quit. `ExtractAssociatedIcon`
returned the crab from both executables, `WM_GETICON` returned the small icon
for the control window, and the tray registration answered
`Shell_NotifyIconGetRect` with `S_OK`. `cargo test --workspace` passed 447
tests (15 binary + 347 core + 17 media + 54 platform + 8 UI library + 6 UI
binary).

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
self-hosted-bundle manifest, not a Flathub submission yet. The build now targets
the 2.0 workspace (both binaries, GTK-free) but requires `org.gnome.Sdk//50`
with rust-stable `25.08` (older minor than the latest available); the runtime
rejects mismatched extension versions. Building also needs FUSE or
`--disable-rofiles-fuse`. All these traps are documented in `flatpak/README.md`.

Flatpak **build and install validated**: the sandboxed bundle runs `ferramari --doctor`,
both binaries present, neither links GTK/GDK/WebkitGTK, translations installed, gsettings
via `flatpak-spawn --host` works. Remaining items not yet exercised inside sandbox:
tray icon appearance/action, portal-based file picker interaction, autostart path,
live wallpaper playback loop. Last verified non-Flatpak run was 0.7.0 with WebKitGTK.