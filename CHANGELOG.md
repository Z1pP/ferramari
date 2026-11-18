# Changelog

## Unreleased

**Stability pass after the 2.2.0 audit.** The Windows-hosted suite now has
**471 tests** (17 binary + 363 core + 18 media + 59 platform + 8 UI library +
6 UI binary).

- **Windows playback with static wallpaper off** no longer enters a restart
  loop. Restoring the original image is now a no-op when Explorer already
  shows it.
- **Configuration and display-id recovery** no longer overwrite a valid
  `.bak` with a corrupt primary; UTF-8 BOMs are accepted, and out-of-range
  numeric settings are clamped.
- **Tracked commands** return application errors to the Iced control surface
  over D-Bus and the Windows loopback transport while the legacy
  fire-and-forget methods remain available.
- **Live playback state** requires a live surface and IPC worker, and the
  status/pending-play UI state expires instead of sticking. A separate
  `is_decoding` fact covers the first-frame wait, so the control surface shows
  `Starting` instead of a false `Idle`.
- **Windows mixed-DPI geometry** is marked physical and is no longer scaled
  twice for occlusion checks.
- **Control transport** limits requests to 64 KiB and 16 concurrent clients.
- **Catalogue cleanup** now actually runs at startup, removes expired sheets
  with their videos, and has a fake-root test.
- **Linux session watchers** now stop at shutdown: each watcher owns a
  connection that is closed to wake its blocking signal iterator, and every
  watcher thread is joined under one two-second deadline. GNOME registers a
  single session client for both end-session signals.
- **Release tags** repeat formatting, clippy, tests and the release build.

## 2.2.4 - 2026-11-18

- **Settings copy reviewed.** Every Settings section now describes what the
  switch or slider actually does. The garbled, duplicated Russian and Japanese
  strings are gone; the profile, hardware-decoding, idle, rotation, occlusion,
  static-wallpaper and background-tray descriptions were rewritten in all
  three catalogs. Windows no longer offers the Linux-only `VA-API` decoder,
  and the dead web-wallpaper networking switch was removed.
- **Settings fit narrow windows.** The package actions wrap onto a second line
  instead of clipping, and minute counts use the interface language.

## 2.2.3 - 2026-11-18

- **Start works after Stop.** `quit_player` destroys the players; the Home
  Start button now clears the user pause and rebuilds the windows from the
  config instead of applying a plan to an empty list. Previously only a full
  app restart brought the wallpaper back.
- **Video changes reuse the player and skip the static-frame refresh.** The
  switch no longer tears down every window or applies a new system wallpaper
  frame while the video is running. Explorer therefore does not replace its
  wallpaper worker and kill mpv on every change; the log shows a reused player
  in microseconds, and the static frame is refreshed when playback stops.
- **Next/previous wallpaper wraps.** The Home step buttons now step through
  the monitor's rotation collection, the library history and all videos in the
  Ferramari folder (in that order). The last item steps to the first and the
  first steps back to the last, instead of reloading the current file.

## 2.2.2 - 2026-11-17

- **No console window from a shortcut.** Windows shortcuts, the desktop
  shortcut and autostart now go through `ferramari-launcher.exe`, a tiny
  GUI-subsystem binary that starts the server with `CREATE_NO_WINDOW` and
  forwards its arguments. The server keeps its console for `--doctor`,
  `--version`, package commands and debug logs when it is run from a terminal;
  the normal shortcut launch shows only the Iced UI.

## 2.2.0 - 2026-11-17

**Ferramari runs on Windows 10/11.** The same two binaries, the same core
policy and the same Iced control surface now have a Win32 backend: a wallpaper
window behind Explorer's desktop icons, physical-pixel monitor discovery,
foreground/occlusion tracking, lock and power events, a tray icon, autostart
and static-wallpaper handling. **452 tests** pass (16 binary + 350 core + 17
media + 55 platform + 8 UI library + 6 UI binary).

- **Wallpaper hosting.** A child window is parented to the `WorkerW` behind
  `SHELLDLL_DefView` (with `Progman` as fallback) and embedded into mpv through
  `--wid`. On Windows 11 Explorer keeps the icon view directly under `Progman`
  and paints the desktop image in a visible `WorkerW` below it; the surface is
  parented into that worker, taking the bottom-most visible one because the
  dummy created by `0x052C` sits above the real painter. Explorer restarts and
  wallpaper changes destroy or hide that host; Windows leaves a cross-process
  child alive, so `WallpaperSurface::is_alive` checks the parent link, the
  host's owning process and the host's visibility rather than only `IsWindow`.
  The media supervisor drops the stale engine and rebuilds against the new
  host, retrying every second while Explorer is gone; a lost host is not
  counted as a player failure.
- **Wallpaper changes no longer freeze playback.** `SPI_SETDESKWALLPAPER`
  makes Explorer replace the wallpaper `WorkerW` and send synchronous messages
  to the surface. The surface window used to belong to the command loop, which
  was inside that very call, so the call waited about 105 seconds for a thread
  that could not answer; mpv died with its host and the generated frame looked
  as if it had never been applied. Each surface now owns a dedicated
  message-pumping thread: the call returns in about half a second, the expected
  host loss rebuilds the player in about two seconds, and a cross-process
  `WM_NULL` or `SetWindowPos` to the surface returns immediately. The frame
  freshness check also compares against the primary monitor's own source
  instead of `Default`, which had discarded every frame after a per-monitor
  switch and left the previous video's image on the desktop.
- **A wallpaper set while Ferramari runs is not rolled back.** The desktop
  image is read again before quitting or toggling static wallpaper. If it is a
  real file other than the frame Ferramari installed — or, with static
  wallpaper off, other than the image recorded at startup — it is the user's
  choice: it stays, `background-original.json` is rewritten to point at it, and
  a later run restores that image instead of the one from the earlier session.
  A new frame records such an image before covering it, so enabling or
  switching static wallpaper cannot lose it either. Path comparison is case-
  and separator-insensitive, and generated frames are never mistaken for the
  user's wallpaper. Covered by four unit tests and verified live with static
  wallpaper on and off.
- **Notification area.** The tray owns a hidden top-level window and re-adds
  its `Shell_NotifyIconW` entry when the shell broadcasts `TaskbarCreated`, so
  the icon comes back after an Explorer restart. The registration is cached in
  a `OnceLock` and covered by a unit test. The icon itself is read from
  resource ID 1 of the running executable, so the tray, Explorer and the
  installer show the same crab.
- **Closing the control window keeps Ferramari running.** On Windows the
  title-bar X and the native close request hide the window instead of ending
  the process: playback continues, "Run in background" is switched on
  automatically so the tray icon exists, and the tray's Show command or a
  second launch raises the same hidden window. `focus_existing_window` no
  longer requires `IsWindowVisible`, so a hidden window is found and restored;
  a hidden control surface closes itself if the server disappears, which
  avoids an invisible orphan after tray Quit. Linux keeps close-and-exit.
- **Performance metrics work on Windows.** The sampler used to read `/proc`
  only, so every tile on the Performance page reported `N/A`. The Windows
  backend now walks the Toolhelp process table for parent links and thread
  counts, reads kernel+user time with `GetProcessTimes`, working set with
  `GetProcessMemoryInfo` and total RAM with `GlobalMemoryStatusEx`. GPU usage
  comes from the Windows `GPU Engine` performance counters through PDH:
  instances are grouped by engine (after dropping the per-process `pid_`
  prefix) and the busiest engine is shown, matching Task Manager. `N/A` remains
  only when the counter set is missing. Verified live at 1080p: CPU 8%, RAM
  0.2–0.3 GB, GPU 50–55%, 62–70 threads and 3 processes while the UI, mpv and
  the video decoder were running.
- **The application has an icon.** `ferramari.ico` and a 256 px PNG are
  generated from the existing SVG and committed under `data/icons/windows`.
  `winresource` embeds the ICO plus version information in both executables,
  the Iced window takes the PNG as its window/taskbar icon, and the tray loads
  resource ID 1. The Inno Setup script uses the same ICO for the setup program
  and its shortcuts. Verified by extracting the icon from both `.exe` files,
  reading the window's `WM_GETICON` and confirming the tray registration with
  `Shell_NotifyIconGetRect`.
- **No D-Bus on Windows.** The server listens on `127.0.0.1` at an ephemeral
  port and publishes a per-process token in `control-endpoint.json`. The Iced
  UI and the second-launch handshake use newline-delimited JSON over that
  endpoint; named mutexes provide single-instance semantics for both binaries.
  A stale endpoint is probed and replaced, and `show_gui` restores and raises
  an already-open control window instead of leaving it minimized behind the
  new launch.
- **Native event sources.** WTS notifications cover lock/unlock, power
  broadcasts cover suspend/resume, `RegisterPowerSettingNotification` covers
  charger changes, `GetLastInputInfo` covers idle, and `SetConsoleCtrlHandler`
  turns Ctrl/Ctrl-Break/close/logoff/shutdown into the same `Command::Quit`
  path with a 1500 ms grace period. Occlusion is sampled from `EnumWindows`
  every 750 ms and only changes are emitted.
- **Media and helpers.** mpv IPC uses a Windows named pipe instead of a Unix
  socket; timed-out yt-dlp/ffmpeg trees are terminated with
  `taskkill /T /F` so a wrapper cannot leave its child holding the pipe.
  `paths::tool` also checks the standard Windows install locations
  (`Program Files\MPV Player`, the winget `Links` folder and per-user
  `Programs`) before falling back to a bare `PATH` lookup, because mpv's
  installer does not add itself to `PATH`.
- **Playback no longer wedges on Windows.** mpv's IPC server writes events and
  reads commands on one thread. A client that only read between commands let
  the 4 KiB pipe buffer fill, so mpv blocked in `WriteFile` and never read the
  next command: playback started but switching, pause, mute and statistics
  stopped answering. A dedicated reader thread now drains the stream
  continuously, replies are matched by `request_id` with a timeout, and `Drop`
  joins the reader before the pipe closes. A separate `worker_alive` flag makes
  a stopped or panicked worker report the player as dead instead of healthy.
  Verified live: switching `Power-Dancing-1080p.mp4` → `arcane-jinx.mp4`,
  pause/resume, master mute and advancing `time-pos` all answered while
  playback continued.
- **Occlusion ignores the shell.** `Progman`, `WorkerW`, taskbars and
  Explorer's XAML island are excluded from the `EnumWindows` sample; otherwise
  the desktop's own host windows counted as covering the wallpaper and Eco
  paused it.
- **Occlusion pauses the covered monitor.** With "Pause on maximized window"
  enabled, a maximized, fullscreen or otherwise fully covering window pauses
  playback on that monitor only; the watcher samples every 750 ms and ignores
  shell, tool and own-process windows. Verified live on Windows by maximizing
  the control window (`[Eco] DISPLAY1: Occluded (play=false)`) and restoring
  it (`Active (play=true)`).
- **The icon font is embedded.** Font Awesome 4.7 ships as
  `crates/ferramari-ui/assets/fontawesome-webfont.ttf` (SIL OFL 1.1, licence
  beside it) and is registered under the family name `FontAwesome`. The rail
  and every page draw their glyphs on a machine with no icon font installed;
  `scripts/glyph-check.py` reads the bundled face instead of a system one.
- **Counted nouns translate again.** GNU MO stores a plural entry under
  `singular\0plural`, and the pure-Rust reader keyed the whole msgid, so
  `tr_n` never matched and "1 display" stayed English in every catalog. The
  parser now keys by the singular and a regression test covers it; Russian
  renders `1 монитор` and Japanese `1 台`.
- **Rotation picker width.** The monitor card's rotation control sizes to its
  widest option with a 220 px cap instead of a fixed 150 px, which had put the
  Japanese `ローテーションなし` label under the arrow. Checked in light and dark,
  Russian and Japanese, at 1180 and 900 px wide.
- **Restart policy.** A player that ran longer than `STABLE_AFTER` and then
  exited had its scheduled retry erased by the stability reset on the next
  tick, so the supervisor never recreated it. The reset now only runs while no
  retry is pending; killing mpv during playback starts a fresh process after
  the one-second backoff and resumes on the same surface.
- **Translations without libintl.** `gettext-rs` and `gettext-sys` are gone.
  `ferramari-core::i18n` reads GNU MO files and evaluates each catalog's own
  `Plural-Forms` expression, so both binaries build on Windows without a C
  gettext dependency. Startup no longer deadlocks on the translator lock while
  resolving the initial language.
- **Packaging.** `installer/build.ps1` builds an Inno Setup installer and a
  portable ZIP with both binaries and their locale catalogs. It finds Inno
  Setup and `msgfmt` on `PATH`, in `Program Files` or under the per-user
  `%LOCALAPPDATA%\Programs` tree where winget installs them. CI now lints,
  tests, builds and runs `--doctor` on `windows-latest`, then uploads both
  artifacts; tagged releases attach them to the GitHub release. Locally, a
  silent per-user install placed both binaries, docs and ru/ja catalogs and
  created Start-menu shortcuts; the shortcut started the server, the control
  surface and mpv, and a control `quit` stopped them. Uninstall removed the
  installation and shortcuts while leaving `%APPDATA%\ferramari` and
  `%LOCALAPPDATA%\ferramari` untouched, and the portable ZIP ran from a path
  containing a space. The control surface
  showed the Russian and Japanese catalogs without clipped labels.
- **Docs.** `docs/BACKEND_CONTRACT.md` gains the Windows backend notes and
  checklist status, `docs/MIGRATION_TODO.md` records what is done and what
  still needs hardware, and `docs/TESTING.md` has a Windows smoke checklist.
  Live playback on one Explorer-hosted display was verified with mpv 0.42,
  ffmpeg 9.0.2 and yt-dlp 2026.10.26 (`d3d11va`, playback behind the icons,
  automatic recreation after killing mpv). Killing Explorer twice rebuilt the
  surface through the `Progman` fallback and kept the icons clickable, and a
  `TaskbarCreated` broadcast re-added the tray icon. A second launch restored
  and raised a minimized control window; tray mute, pause, reload, lucky, Show
  and quit acted on the running server; autostart wrote and removed its `Run`
  key; static wallpaper applied a frame and restored the recorded original.
  A second live pass on 2026-11-15 switched between two videos, paused,
  resumed and muted through the control API while `time-pos` advanced, and
  checked the embedded icons and ru/ja counted nouns in light and dark themes
  at 1180 and 900 px. A third pass on 2026-11-16 switched
  `Power-Dancing-1080p.mp4` ↔ `arcane-jinx.mp4` through the same `video`
  command the control surface sends (with and without a monitor name): the
  matching static frame was installed in 0.34–0.51 s instead of about 105 s,
  Explorer replaced its worker as expected, the player rebuilt in about 2.5 s
  and two desktop captures three seconds apart differed at 30k+ sample points
  after every switch. A `WM_NULL` and a `SetWindowPos` sent from another
  process to the surface returned in under a millisecond. On 2026-11-16/17 an
  external wallpaper change while static wallpaper was on left playback
  running, and quitting kept the chosen image and rewrote the sidecar; with
  static wallpaper off the same change was also kept, and a change made before
  a video switch was remembered when the new frame was installed. Synthetic
  lock/unlock and suspend/resume messages moved the monitor state between
  `suspended` and `active`, `WM_QUERYENDSESSION` answered 1, and
  `WM_ENDSESSION` or closing the console window stopped the server, removed
  the endpoint and restored the wallpaper. A clean launch kept the control
  window above the desktop through a video switch and hit-testable. The
  installer was rebuilt and re-exercised (install, upgrade in place, uninstall,
  portable ZIP); config stayed byte-identical. Multi-monitor and mixed-DPI
  scenarios stay unchecked until they are run on suitable hardware.

## 2.1.0 — 2026-11-10

**The control surface redrawn to the reference design, and the shutdown path
made real.** **436 tests** pass (337 core + 46 platform + 20 media + 19 UI +
14 binary).

- **`Install` installs.** The button put the wallpaper on the screen and left
  the file in `~/.cache/ferramari/catalogue`, where the seven-day and 2 GB
  pruning eventually deletes it — leaving a Library card and a config entry
  pointing at a path that no longer exists. `catalogue::install` had lost its
  caller during the GTK removal, so the download now copies into
  `$XDG_VIDEOS_DIR/Ferramari` under the source's own name before playing.
- **The paused engine no longer reports a frame rate.** mpv answers
  `container-fps` for a loaded-but-paused file, so the FPS tile showed `30`
  next to an `Idle` badge in the same window. The tile and its history now
  read nothing while the engine is not playing.
- **Thread and process counts returned to the Performance page**, dropped by
  the redesign, in a card of their own: they describe the process tree, not
  mpv, and did not belong under *MPV Statistics*.
- **Category tiles answer in the source's language.** The catalogue publishes
  its categories in Russian whatever the interface language is, so an
  English-only keyword table left nearly every tile on the fallback globe;
  `Зима` and `Огонь` now get a snowflake and a flame instead of a collapse
  arrow and a duplicate bolt.
- **CI is green again.** One lint test asserted what ffprobe said about a
  fabricated `.mp4`, so it passed on a developer machine and failed on every
  runner without ffmpeg — red since 2.0.0. The probe's interpretation now has
  pure tests that need no tool, and the manifest test only tests the
  manifest.

- **The control surface was rebuilt around the reference design.** A gradient
  backdrop instead of a flat fill, a title bar the app draws itself, a rail,
  and five pages assembled from one material vocabulary. The styles moved to a
  new `crates/ferramari-ui/src/style.rs`, which owns every closure, so
  `main.rs` holds no colour literal and no radius arithmetic of its own.
  Home's hero is now the wallpaper's own frame with transport and volume over
  a scrim, Library is a grid of measured cards, Performance gained the
  decoder's numbers and a frame-time chart with its axis.
- **Two shutdown bugs, both real, both only reachable now.** `sigprocmask` was
  called inside the signal-waiter thread, where it blocks the signal for that
  thread alone; the main thread kept the default action, so `kill -TERM`
  stopped the process mid-fade and left its mpv children re-parented to init.
  Signals are now blocked in the main thread before anything spawns. Fixing
  that made `shutdown()` reachable for the first time, and it hung: the
  monitors watcher's `Drop` joined a thread parked in `wait_for_event`, which
  cannot observe the stop flag without an X event, so the daemon kept holding
  the D-Bus name and no new one could start.
- **`Run in background` changes the tray at runtime.** `is_show_systray` is now
  writable over D-Bus and registers or withdraws the StatusNotifierItem
  without a restart. Measured on the bus: the SNI name goes 0 → 1 → 0.
- **The frame rate shown is the frame rate produced.** `estimated-vf-fps` and
  `display-fps` join the probe next to `container-fps`, because a 60 FPS file
  capped to 30 by the Eco Engine is running at 30 and the tile is about what
  is on screen. The FPS tile's ceiling is the display's own refresh rate, read
  from RandR (`MonitorInfo::refresh_hz`) rather than assumed; on the test panel
  RandR reports 120 Hz and mpv's `display-fps` independently answers 119.93.
- **`Check for updates` is a real check.** `ferramari_core::update` asks
  GitHub's releases API, caches the answer in `update.json` for a week, and is
  gated by the user's switch, so opening the window to change a volume costs
  no request. Verified against the live API: it fetched `v2.0.0`, compared it
  with this build and reported "up to date".
- **Translations are generated, not remembered.** 116 strings had been added
  to the UI without ever entering the catalogues. GNU gettext has no Rust
  parser, so `scripts/extract-messages.py` generates the template,
  `make po-update` folds it into each language and `make po-fill` applies
  reviewed wording to what msgmerge could only guess — its guesses were unsafe,
  `Nothing is playing` having arrived as `Сейчас воспроизводится`, the
  opposite. Both catalogues are at 180/180, including Russian's three plural
  forms, which msgmerge flattens silently. `make po-check` fails when the
  template and the sources drift.
- **Icon names are checked against the installed font.** A name missing from
  the table, or a codepoint borrowed from Font Awesome 5, renders as a wrong
  glyph with no error; `scripts/glyph-check.py` reads the font's own charset
  and reports both cases.
- **`FERRAMARI_UI_BOOT="page=… size=… section=…"`** opens the window at one
  page, size and settings section, which is how every page was verified in
  dark and light, at 1180, 940 and 1600 wide, in English, Russian and
  Japanese.

- **A wallpaper now follows the physical monitor.** A config key was only as
  durable as the enumeration that produced it, so a replugged screen that came
  back labelled differently, or two identical panels reported in the opposite
  order, silently swapped their wallpapers. Each display carries the connector,
  EDID when the server exposes it, physical size and any backend-native id;
  their digest is remembered in `~/.config/ferramari/display-ids.json` as the
  key it owns. A screen seen for the first time keeps the name the backend
  reported, so configs from 1.9.x are not rewritten, and a known screen reclaims
  its key across replugs, reboots and reordered layouts. Two cloned monitors
  over one output no longer share a key.
- **Hardware decoding is measured, not assumed.** mpv accepts
  `--hwdec=vaapi` on hardware without a VA-API path and decodes on the CPU
  without a word about it. The decoder actually running is now read from the
  player over IPC (mpv 0.41 reports it as `hwdec-current`), published as the
  `hwdec_running` D-Bus property, compared against the setting, shown under it
  when they disagree, and included in `--doctor`. Measured on the test machine:
  the real wallpaper at `hwdec=auto` decodes as `vaapi`, which nothing reported
  before.
- **Resource metrics name their reason.** One `available` flag could not tell a
  platform with no process sampler apart from a driver without
  `gpu_busy_percent`, a server process that is gone, or the first sample of a
  rate — all four showed as zeroes or as one generic "Unavailable". Each
  reading is now `Value`, `Unsupported`, `Unavailable` or `Pending`, so an
  unmeasurable thing no longer looks idle. A dead process tree also clears the
  CPU interval instead of reporting a spike across the gap.
- **Capability reporting decides per feature.** One X11 level was applied to
  the surface, enumeration, hotplug and window tracking together, which
  downgraded the two capabilities that genuinely work under XWayland. Each
  level is now derived from its own evidence and carries the reason, printed as
  an indented note by `--doctor`.
- **CI audit job fixed.** `rustsec/audit-check` publishes its findings as a
  check run; with only `contents: read` the Checks API call was rejected and
  the job failed even though no critical advisory existed. The action swallows
  that failure when `GITHUB_HEAD_REF` is set, which is why pull-request runs
  stayed green while every push to `main` was red.
- **The backend contract is written down.** `docs/BACKEND_CONTRACT.md` states
  what a platform backend must provide, the threading and ownership model, the
  event-ordering guarantees core relies on, the shutdown order and the display
  identity rule, and records two gaps honestly: the D-Bus watchers cannot be
  stopped before process exit, and under XWayland display identity is
  connector-based because the compositor hides EDID.
- **Session signals are typed models, not booleans.** Lock state used to be
  three booleans whose aggregation rule lived twice. `session::LockState` now
  keeps one bit per source (screensaver service, session manager) and states
  the rule once, so a logind unlock cannot cancel a lock the screensaver still
  holds. Idle gained a third state: `IdleState::Unknown` says the idle facility
  did not answer, which is not the same as the user being active. `pauses_playback`
  resolves it conservatively, so a screensaver service that vanished during a
  shell restart cannot leave the wallpaper paused forever. Idle polling logic
  moved out of the thread closure into a testable `IdlePoller`.
- **`ferramari package lint` for creators.** `package::load` answers "may this
  be applied"; a creator preparing a release needs "what will a recipient see".
  Import happily accepts a package with no description, no preview and no
  resolution, and it renders as a blank card. lint splits findings into errors
  (do not publish: import refuses, or the signature does not verify — import
  itself would still copy a tampered package) and warnings (publishable but
  degraded). Findings carry stable codes so CI can filter (`--json`,
  `--ignore preview-missing`); a typo in the ignore list fails with exit 3
  rather than silently filtering nothing. Warnings exit 0 by default —
  `--strict` gates on them. Missing ffmpeg and a non-decodable video are
  reported as different things, so a machine without the tool does not blame
  the creator for it.

## 2.0.0 — 2026-11-07

**Major release: GTK removal complete, Iced UI, native event loop.**

- **GTK/GDK/WebKit fully removed** from the server binary. The binary now links
  zero GTK, GDK, Pango, Cairo, or WebKit libraries (153 MB → 153 MB, but
  runtime GTK dependencies are gone).
- **Iced UI** replaces the GTK control surface. The UI is a separate binary
  (`ferramari-ui`) that connects to the server over D-Bus, so the server has
  no GUI dependency at all.
- **Native event loop** replaces `glib::main_loop`. The server runs on a plain
  `recv_timeout` loop with deadline bookkeeping for periodic tasks (player
  watch, rotation, power). No GLib main loop, no `glib::timeout_add_local`.
- **Native signal handling** replaces `glib::unix_signal_add`. SIGINT/SIGTERM
  are blocked process-wide and delivered via `sigwait` on a dedicated thread.
- **Native fade engine** replaces `glib::timeout_add_local` for volume fades.
  Each fade runs on a dedicated `std::thread` with `std::thread::sleep`.
- **Native wallpaper surfaces** replace GTK `ApplicationWindow`. Windows are
  created via `x11rb` with `_NET_WM_WINDOW_TYPE_DESKTOP`, no decorations,
  non-focusable, lowered below desktop icons.
- **Native monitor enumeration** replaces `gdk::Monitor`. Uses `x11rb` RandR
  with hotplug via `randr_select_input` on a dedicated thread.
- **Native tray** via `ksni` (already in place, no GTK).
- **Web wallpapers** gated behind `web-wallpaper` feature flag. Enabling it
  pulls WebKitGTK 2.x back in (the only toolkit with a maintained Rust
  binding for WebKitGTK 4.1). See `docs/archive/GTK_REMOVAL_AUDIT.md` §4 for
  alternatives (CEF, external browser process).
- **160 tests** pass (113 core + 19 platform + 19 media + 8 UI + 1 binary).
- **Workspace split** into 4 crates: `ferramari-core`, `ferramari-media`,
  `ferramari-platform`, `ferramari-ui`.

### Breaking changes

- `ferramari` no longer links GTK. Build with `--features web-wallpaper` to
  enable WebKitGTK web wallpapers (requires GTK dev packages).
- The control surface is now a separate binary (`ferramari-ui`). The server
  launches it via `std::process::Command` when `show_gui` is called.
- `Command::SetTheme` now accepts `system`, `light`, or `dark`. The old GTK
  theme system is gone.
- `Command::SetLanguage` now accepts `system`, `en`, `ru`, or `ja`. The old
  GTK language system is gone.

### Migration guide

See `docs/archive/GTK_REMOVAL_AUDIT.md` for the full audit and migration plan.

## 1.9.4 — 2026-11-04

Fullscreen finally pauses the wallpaper — including on Wayland sessions.

- The EWMH watcher was disabled wholesale on a Wayland session. It now runs
  there too: the app is an XWayland client, and X11 windows do appear in
  `_NET_CLIENT_LIST` (measured: a fullscreen mpv window is listed with
  `_NET_WM_STATE_FULLSCREEN` and produces `Occluded` within ~0.5 s). Native
  Wayland windows remain undetectable through EWMH; `--doctor` and the
  README say so.
- A live test showed the pause never reached mpv. `pause_fade` applied the
  pause as the completion callback of the volume fade, and the mute rule
  started its own fade on the same single slot in the same command,
  cancelling it: the player decoded at volume 0. The pause now owns a second
  fade slot, so occlusion actually stops decoding: mpv 3.8% → 0.0% CPU while
  a fullscreen window covers the monitor, position frozen.
- The "Pause/Mute when maximized" switches are no longer hidden on Wayland.
- Web wallpaper crashes are detected (audit B14): WebKit's
  `web-process-terminated` signal flips a shared latch, the supervisor
  treats the window as dead and recreates it through the same restart
  policy mpv uses. Before this a crashed web process left a blank desktop
  forever.

## 1.9.3 — 2026-11-03

Streaming no longer re-runs yt-dlp on every reload.

- A resolved stream is cached per page URL and screen height for 15 minutes.
  `Reload` and `Apply` adopt it on the GTK thread, so the wallpaper comes back
  in tens of milliseconds instead of 2.6 s, and the desktop is not blanked
  while yt-dlp works.
- A playback error drops the cached entry and the resolved stream, so the next
  `Apply` or `Reload` asks the site for a fresh link instead of replaying a
  dead one.
- `scripts/benchmark.py --mode stream --url … --reloads N` reports reload
  latency and the number of yt-dlp runs, so the numbers below are reproducible.

Measured on one 1080p YouTube URL: reload 2.6 s → under 0.3 s (usually 30–50 ms
after the window rebuild), with one yt-dlp run per URL per 15 minutes instead
of one per reload.

## 1.9.2 — 2026-11-02

Layout regression from 1.9.1: content was clipped at the right edge.

- The content stack reserved the widest page's minimum width; long Russian
  captions (Settings rows, the Local Video header and action bar) made that
  minimum larger than the window, so every page lost its right edge.
- Text now shrinks instead of pushing the layout: titles, button captions and
  descriptions ellipsize or wrap, and the page header stacks its action row
  below the title when the two no longer fit side by side.
- Verified by measuring every widget against the window's client edge in
  Russian and English at 680–1300 px: zero overflowing widgets.

## 1.9.1 — 2026-11-01

### Fixed

- Gray desktop after a restart with the static wallpaper enabled. Frames moved
  to `$XDG_CACHE_HOME/ferramari/static/` in 1.0.1, but the "original" background
  captured at startup could itself be one of those frames. The cache is capped,
  so pruning deleted the file GNOME was still pointing at and the desktop went
  gray. The original is now persisted to `background-original.json` outside the
  cache, cache files still referenced by GNOME or by that record are never
  pruned, and a background we cannot restore is never overwritten blindly.
  Applying a frame also repairs a dangling URI left by an older build.
- `ferramari --doctor` now reports the desktop background as `user background`,
  `Ferramari frame` or `DANGLING … (this is a gray desktop)`, so this state is
  visible without reading a log from the last restart.
- `prune_static_cache` off-by-one: protected and kept files were not counted
  against the limit, so the cache could exceed it.
- Local Video action bar: the two new package buttons made the bar wider than
  the window, so it clipped the grid and the label. Replaced with a single
  **Package…** menu that appears only for installed packages.

## 1.9.0 — 2026-11-01

Package integrity and authorship.

- `checksums.json` records the SHA-256 of every package file. Loading a package
  that carries it verifies each entry, so a modified or extra file is rejected
  with a message naming the file. Packages without checksums still load.
- `signature.json` is an Ed25519 signature over those checksums, so one
  signature covers the whole package. Verification reports **Trusted**,
  **Unknown author**, **Unsigned** or **Invalid**.
- Ferramari creates a local signing key on first use
  (`~/.config/ferramari/signing-key.json`, mode `0600`) and signs exports
  automatically. Trusted keys live in `trusted-keys.json`; trusting is always
  an explicit choice, and there is no network validation.
- New headless CLI: `ferramari package key|sign|verify|trust|keys`, with exit
  codes for scripting (0 trusted, 1 untrusted, 2 invalid).
- Local Video gains **Sign** and **Verify** for package entries, with a report
  dialog that offers to trust an unknown author. Settings gains a
  **Package signing** section showing the key, the trust store and an export
  button.
- `ring` and `base64` become direct dependencies; `ring` was already in the
  tree through `ureq`, so no new native library is introduced.
- Tests: 140 passing.

## 1.8.0 — 2026-11-01

Eco Engine completion: decoder parking and broader measurements.

- Release the mpv decoder after a local video has been paused out of sight for
  five minutes. The worker captures `time-pos`, switches mpv to idle and sends
  `stop`; the process and IPC socket stay alive. Resuming reloads the same
  source at the saved position, so the cold-start cost is not paid again.
- Parking is deliberately conservative: only local video windows, only while
  the monitor is covered by a fullscreen/maximised window, locked or suspended,
  and never for an explicit user pause or a visible Eco idle. A black frame
  cannot be left in front of the user.
- The park/unpark sequence runs on the existing mpv IPC worker; the GTK main
  loop never waits for `get_property` or `loadfile`.
- Benchmark harness gains web-wallpaper mode (`--mode web --web`), `--label`
  and `--app-log`; `make bench` accepts `MODE`, `WEB` and `LABEL`.
- Measure and document 1080p, synthetic 4K and local-canvas web wallpapers.
  The Power profile reduced GPU load but not CPU on the tested clip; the
  documentation no longer claims a CPU saving.
- Tests: 126 passing.

## 1.7.0 — 2026-11-01

Ecosystem foundation.

- Add declarative catalogue-source plugins (`$XDG_DATA_HOME/ferramari/plugins/
  <id>/plugin.json`). Plugins are never executed; they provide only an HTTP
  API base URL. Invalid ids, non-http URLs and future `engine_min` versions are
  skipped with a log line. See `docs/PLUGIN_API.md`.
- Add the Ferramari Catalog API v1 client (`src/catalog_api.rs`): pagination,
  search/category query encoding, 15 s timeout, 2 MiB body limit, up to 200
  items, HTTP-only preview/download URLs. See `docs/CATALOG_API.md`.
- Add `ferramari --catalog <base>` to inspect a deployment and list accepted
  plugins in `--doctor` and Settings → Community sources.
- Tests: 123 passing.

## 1.6.0 — 2026-11-01

Creator Engine: `.ferramari` packages.

- Add `src/package.rs`: manifest parsing, strict validation, import, export and
  installed-video discovery.
- Validation rejects newer formats/engine versions, path traversal, symlinks,
  wrong extensions, more than 10,000 files and packages over 2 GiB. Import
  revalidates after copying and cleans up on failure.
- Local Video gains **Import…** and **Export .ferramari**; imported video
  packages join the library grid, imported web packages are applied as the
  wallpaper.
- Export writes `wallpaper.<ext>`, a manifest and an optional thumbnail preview,
  then reloads the package before reporting success.
- Add `docs/FERRAMARI_FORMAT.md` and package tests.
- Tests: 117 passing.

## 1.5.0 — 2026-11-01

Cross-desktop diagnostics and session integration.

- Add `ferramari --doctor`: prints session type (X11 / Wayland-XWayland / native
  Wayland), desktop, X11 screen count, backend, tool versions (mpv, ffmpeg,
  ffprobe, yt-dlp, vainfo), DRM render nodes and their drivers, idle service and
  lock services. It never initialises GTK or claims a D-Bus name.
- Show the desktop integration summary in Settings and state explicitly that
  native Wayland surfaces are not implemented.
- Watch `org.freedesktop.login1.Session` `Lock`/`Unlock` as a fallback. Locks
  from the screensaver and logind are aggregated, so a logind unlock cannot
  resume playback while a screensaver still reports locked.
- Add pure tests for session/desktop classification and the “native Wayland is
  never claimed” invariant.
- Tests: 110 passing.

## 1.4.0 — 2026-11-01

Performance and hardening.

- Replace the 300 ms X11 polling loop with a `PropertyNotify`/`StructureNotify`
  watcher. An idle desktop produces no wakeups; fullscreen/maximise changes are
  processed immediately. Transient query errors still keep the last good state.
- Add a Performance page with live CPU, RSS, thread, process and GPU metrics
  (`src/metrics.rs`), sampled every 2 s only while the control window is
  visible.
- Add `scripts/benchmark.py` and `make bench`: isolated config/cache, private
  D-Bus, startup/CPU/RSS/threads/processes/GPU sampling and optional mpv
  recovery. See `docs/BENCHMARKS.md`.
- Add the hardware decoding setting (Auto / VA-API / Off) and pass it as mpv
  `--hwdec`.
- Harden WebKit wallpapers: explicit file-access settings, no developer
  extras, denied permissions, no pop-up windows, and an optional network switch
  that blocks non-local navigations.
- Fix a deadlock when the mpv child exited before `Mpv::Drop`: the IPC worker
  waited in `recv()` while `join()` waited for it. Always wake the worker.
- Report `loadfile` failures from the worker as a user-visible error.
- Remove abandoned `.part` thumbnail/static files older than an hour on start.
- Tests: 107 passing.

## 1.3.0 — 2026-11-01

Eco Engine completion.

- Configurable `idle_timeout_sec` (1–1440 minutes in Settings, 30 s–24 h
  accepted from config) shared with the idle worker through an atomic; changing
  it takes effect on the next 15 s poll.
- Balanced applies a 30 fps `vf` cap through mpv IPC while idle and restores the
  profile filter on activity. Eco pauses on idle. The runtime `vf` property was
  verified against mpv 0.41.
- Per-monitor Eco state (`active`, `idle`, `occluded`, `suspended`) is shown on
  Home monitor cards and exported as the D-Bus `monitor_states` property.
- Fix Mutter idle method name (`GetIdletime`), which made idle detection dead on
  GNOME. Add a probe test that is a no-op on headless CI.
- Coalesce added/removed/size monitor events into one 250 ms rebuild.
- Surface a `loadfile` failure from the mpv worker as a user-visible playback
  error instead of a log-only message.
- Cache the Home video-folder scan for one second so volume-slider commands do
  not re-read the whole directory.
- Tests: 100 passing.

## 1.2.0 — 2026-11-01

Eco Engine core and Core Stability (Phase 0 audit + Phase 1/2 first slice).

- Add a per-monitor Eco Engine (`src/eco.rs`): `Active`, `Idle`, `Occluded`,
  `Suspended`; Quality, Balanced and Eco policies; idle pause only in Eco.
- Pause and mute decisions are evaluated per monitor. A fullscreen game on one
  screen no longer stops the wallpaper on the other screens.
- X11 now reports visible window rectangles; a maximized window with a panel
  inset still counts as covering its monitor. Window state is sorted to avoid
  spurious updates on focus changes.
- Account for GDK logical vs X11 device pixels (`MonitorInfo::scale_factor`),
  so occlusion works on HiDPI/XWayland layouts.
- Move mpv JSON IPC to a dedicated worker thread. `Mpv::spawn` returns without
  waiting for the socket; volume, pause, mute and `loadfile` are queued. A
  wedged mpv can no longer block the GTK main loop for the IPC timeout.
- Move the system-bus/session-bus setup for suspend and end-session/Idle
  watchers off the GTK thread.
- Add idle detection through GNOME Mutter, the freedesktop screensaver, KDE or
  XFCE idle counters (15 s poll on a worker, commands only on threshold
  crossing).
- Flush a pending debounced config write before a monitor hotplug reloads
  `config.json`; a pending volume/blur edit is no longer lost.
- Repair config fields individually: one wrongly typed field no longer resets
  every other setting to its default.
- Keep the previous X11 window sample on a transient query error instead of
  assuming “nothing is maximized”.
- Accept local `file://` web wallpapers (including percent escapes) and reject
  remote `file://host/...` authorities.
- Mute hidden web wallpapers (WebKitGTK has no public freeze API, so JS
  timers/animations are only throttled).
- Read battery state once per construction; power polling only sends a command
  when the state changes.
- Add `docs/AUDIT.md` with the Phase 0 findings and the remaining work.
- Tests: 98 passing (up from 81).

## 1.1.0 — 2026-10-31

- Light, dark and system colour schemes in Settings → System → Theme.
- The selected theme is stored in `config.json` and exposed as the `theme`
  D-Bus property; changes apply immediately, without a restart.
- A dark-theme screenshot is included in the README.

## 1.0.1 — 2026-10-31

Performance and idle-power improvements.

- Local thumbnails: input seek, no `thumbnail` filter, two-worker pool and
  cancellation of stale grids (2.2× on 1080p, 4.5× on 4K).
- Static wallpaper frames cached by path, mtime and blur radius; bounded to 12
  files. A restart reuses the frame in ~55 ms instead of ~860 ms.
- Event-driven command loop driven by a wakeup pipe; idle voluntary context
  switches fall from ~26/s to ~1.2/s.
- Volume and blur changes are debounced into one atomic config write, flushed
  on quit and shutdown.
- The GNOME desktop-icon workaround runs only when a wallpaper window exists.
- Video-to-video switches on one monitor reuse the existing mpv and window;
  `loadfile` is sent asynchronously so the GTK thread never waits for a decode.

## 1.0.0 — 2026-10-31

First stable release.

- Reliable mpv supervision, crash-safe config and library data.
- Multi-monitor sources, hotplug handling and per-monitor rotation.
- Local library: favorites, recent history, collections and rotation.
- Catalogue: search, categories, sorting, resolution filters and frame previews.
- Web pages, streaming, static wallpaper and power profiles.
- Suspend/resume, screen lock, battery pause and autostart integration.
- English, Russian and Japanese UI.
- Flatpak manifest, tagged releases, CI and testing documentation.

## 0.9.0 — 2026-10-31

- Performance, Balanced and Power saving playback profiles.
- Power saving uses frame dropping, one decoder thread, bilinear scaling and a 30 fps filter.
- Optional pause when running on battery; playback resumes when AC returns.
- Battery percentage and AC/battery state shown in Settings.

## 0.8.0 — 2026-10-31

- Assign a library collection to an individual monitor from the Home page.
- Rotate that collection in order or randomly on a configurable interval.
- Settings page controls rotation mode and interval (minutes).
- Rotation runs on the GTK loop with a five-second tick and never blocks the UI.

## 0.7.0 — 2026-10-31

Library, catalogue filters and Flatpak.

- Local library: favorites, recent wallpapers and collections.
- Collection create/rename/delete, add/remove videos and filtering.
- Catalogue sorting by title, category, size and duration.
- Resolution filters and metadata (size/duration/resolution) from previews and downloads.
- Flatpak manifest and host-tool bridge for mpv, ffmpeg, ffprobe and yt-dlp.
- Testing matrix and smoke checklist in `docs/TESTING.md`.

## 0.6.0 — 2026-10-31

- Dedicated Settings page with playback, wallpaper and system sections.
- Settings is available from the sidebar, the header gear and the Home page.
- Configuration file, About and Quit actions live on the Settings page.
- The old quick-toggles popover was removed to avoid duplicating controls.

## 0.5.0 — 2026-10-31

Home screen and per-monitor wallpapers.

- New Home page with the current wallpaper, playback state and quick links to
  the library, catalogue and settings.
- Monitor cards show name, resolution, current source and playback state.
- Assign a local video to one monitor directly from its card; the rest of the
  monitors keep playing.
- Russian and Japanese strings for the new screen.

## 0.4.0 — 2026-10-30

Reliability and recovery.

- Detect an unexpected mpv exit and recreate the player with a capped
  exponential backoff. After five retries the automatic loop stops instead of
  spawning processes forever.
- Kill and reap wedged external commands (`mpv`, `ffprobe`, `ffmpeg`,
  `xdg-user-dir`, Flatpak `gsettings`) at a hard deadline.
- Report WebKit load failures, retry the page once and show one clear error.
- Generate static-wallpaper frames on a worker thread so ffmpeg no longer
  blocks the GTK main loop.
- Write `config.json` atomically and keep `config.json.bak`; recover from a
  truncated or invalid primary file.
- Add monitor reattachment and scaling tests, structured user errors, locked
  CI builds, metadata and translation validation, a RustSec audit and a
  tag-driven release workflow.
- Requires Rust 1.88 (security update of the ZIP parser).

## 0.3.0 — 2026-10-29

- Russian interface and a language selector (System, English, Russian,
  Japanese).
- Russian plural forms through gettext.
- Translated tray menu.

## 0.2.0 — 2026-10-29

- Live wallpaper catalogue with categories, search and pagination.
- Still preview, six-frame sheet, download progress, cache management and
  install through the monitor menu.

## 0.1.0 — 2026-10-26

- First release: local video, streaming, web-page wallpapers, multi-monitor,
  GNOME static wallpaper, tray, D-Bus and autostart.
