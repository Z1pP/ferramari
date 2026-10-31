# Performance baseline and optimisation paths

Measured 2026-10-31 on Ubuntu 26.04, GNOME Shell 50.1, Wayland session
(Ferramari runs through XWayland), release build (`ferramari 1.0.0` for the
baseline, `1.0.1` for the optimisation results),
single 1920×1080 monitor. Numbers are relative indicators, not a benchmark
suite. Each scenario ran in an isolated `dbus-run-session`, with a warm page
cache. RAM/PSS/CPU/threads are for the whole process tree (Ferramari + mpv or
WebKit), sampled over 10 seconds.

Two- and three-monitor scenarios could not be measured on this single-display
machine; they remain part of the release checklist.

## Baseline

| Scenario | RAM | PSS | CPU | Threads | Processes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Null mode | 58.3 MiB | 37.3 MiB | 0.2% | 16 | 1 |
| 1080p30 video, balanced | 266.9 MiB | 220.6 MiB | 2.7% | 53 | 2 |
| 4K30 video, balanced | 323.8 MiB | 277.4 MiB | 3.2% | 52 | 2 |
| 4K60 video, balanced | 317.3 MiB | 269.0 MiB | 9.0% | 55 | 2 |
| 4K60 video, performance | 317.7 MiB | 269.5 MiB | 6.1% | 55 | 2 |
| 4K60 video, power saving | 317.1 MiB | 268.9 MiB | 5.1% | 67 | 2 |
| 4K30 video, paused | 323.6 MiB | 277.2 MiB | 0.2% | 52 | 2 |
| Animated local web page | 446.4 MiB | 247.0 MiB | 0.1%* | 93 | 3 |

\* The web page was in a background wallpaper window; WebKit throttles
`requestAnimationFrame` when the surface is not user-visible, so its CPU
number is not representative. The memory/thread cost is.

Startup to the video mode decision was ~290 ms in this run (including process
start, D-Bus registration and window setup); the server itself logged `Started`
after ~40 ms.

## Power profiles

The 4K60 run is the useful comparison: the power-saving profile reduced the
process-tree CPU from 9.0% to 5.1% by dropping late frames, using one decoder
thread, bilinear scaling and a 30 fps filter. Performance mode uses
display-resample interpolation and measured 6.1% in this sample; treat the
difference from Balanced as workload-dependent rather than a guarantee.

## 1.1 optimisation results (2026-10-31)

These changes keep the RAM/CPU baseline above unchanged; they remove work from
the startup and idle paths.

### Thumbnails

| Video | Old command | New command | Speed-up |
| --- | ---: | ---: | ---: |
| 1080p30 | 432 ms | 197 ms | 2.2× |
| 4K30 | 1594 ms | 353 ms | 4.5× |

`-ss 1` now precedes `-i` and the `thumbnail` filter is gone. A pool of two
workers replaces one thread per video, and refreshing the library cancels
stale work. With the (path, mtime) cache, a warm grid does not start ffmpeg
at all.

### Static frame cache

Frames are cached by `(path, mtime, blur)` in
`$XDG_CACHE_HOME/ferramari/static/`. On a 1080p video, the gap from `[Mode]`
to `[StaticWallpaper] Applied` was 860 ms on the first run and 55 ms on the
next start (file check plus `gsettings set`). The cache is capped at 12 PNGs,
so it cannot grow without bound.

### Command loop and config writes

The 40 ms command poll is gone. Producers now write a byte to a pipe and
`glib::unix_fd_add_local` drains the queue; the remaining periodic sources are
the 1 s player supervision, 5 s rotation and 30 s power poll. Voluntary
context switches fell from ~26/s to 1.2/s in both null and video mode, and a
command no longer waits up to 40 ms.

Nine rapid D-Bus volume changes produced exactly one config write, confirmed
with the `[Config] Saving` debug line. A flush still happens on quit and
shutdown, and the 1 s supervision tick flushes a pending write without adding a
timer.

### Reusing mpv for video → video

A monitor that switches from one local video to another now keeps its mpv
process and X11 window. `loadfile` is written without waiting for the reply:
mpv may answer only after the new file is ready, which blocked the GTK thread
for 845 ms in the first version of this change. Sending it asynchronously
brings the switch to ~0.2 ms of app time, against ~120 ms for a new process
plus the IPC wait. The reuse path was verified by reading mpv's `path`
property after the switch; the PID stays the same.

### DING

`gnome_desktop_icon_workaround()` now runs only when at least one wallpaper
window exists, so null mode no longer pays for an extension reload.

## Resolved: static wallpaper no longer blocks the GTK loop

Before 0.4.0, `apply_static` ran ffmpeg synchronously on the GTK thread
(0.67–1.59 s on the same 1080p file). Static frames are now generated on a
worker thread, with a generation counter for stale results and ffmpeg/probe
timeouts; the UI and playback start immediately. The rest of this document
keeps the historical optimisation notes and marks what is still open.

## Ranked bottlenecks

### 1. Static wallpaper (resolved in 0.4.0)

Historically `App::update_static_wallpaper` called `GnomeBackground::apply_static`,
which ran `ffmpeg` synchronously on the main thread. Journal gaps between
`[Mode]` and `[StaticWallpaper] Applied`: 0.67 s, 0.79 s, 1.59 s on the same
1080p video. The first `ffmpeg` invocation is the slow one (cold start).

Measured directly (ffmpeg alone):

| Work | Time |
| --- | --- |
| 1080p frame seek + `gblur=sigma=5` | 0.66–0.69 s |
| 1080p frame seek, no blur | 0.56–0.60 s |
| 4K frame + full-res blur | 0.96–0.97 s |
| 4K downscale → blur → upscale | 0.90 s |

Blur is cheap; seek/decode dominates. Because the call is synchronous, the GUI
appears late on startup and D-Bus/UI commands are stalled on every mode
switch. This is the largest *perceived* performance problem.

**Paths**

1. Generate the frame and blur on a worker thread. Show the control window and
   start playback immediately; set the GSettings URI when the file is ready.
   The video window already covers the screen, so a 0.7 s delay is invisible.
2. Cache the result by `(video path, mtime, blur radius)` in
   `$XDG_CACHE_HOME/ferramari/static/`. Repeated switches and blur changes
   become a file check plus `gsettings set`.
3. Optional: ask the running mpv for the current frame
   (`screenshot-to-file` via IPC) instead of re-decoding with ffmpeg. This
   avoids a second decoder and the seek; fall back to ffmpeg when mpv is not
   ready.
4. Do not regenerate at all while the video window is fullscreen and the
   static wallpaper is not visible. Generate lazily when the video is hidden.

### 2. Thumbnails (resolved in 1.1)

`thumbnails::generate` runs:

```
ffmpeg -y -v error -i VIDEO -ss 1 -vf thumbnail,scale=-1:96 -frames:v 1 OUT
```

`-ss` is an output seek and `thumbnail` scans frames. Measured per video:
0.47–0.51 s. The cheaper equivalent (`-ss 1` **before** `-i`, no `thumbnail`
filter) takes 0.20 s. `fill_store` spawns one thread/ffmpeg per video, so a
cold GUI open runs N decoders at once.

Six videos, cold cache:

| Strategy | Wall time |
| --- | --- |
| Current: all six in parallel | 3.26 s |
| Input seek + pool of two | 1.21 s |

**Paths**

1. Move `-ss` before `-i`; drop the `thumbnail` filter (or use it only as a
   fallback for videos where seek lands on a black frame).
2. Bounded worker pool (2–4) instead of one thread per file.
3. Cancel stale generation when the list is refreshed; add a generation
   counter and ignore results from old runs.
4. Keep the existing (path, mtime) cache; it already avoids repeat work.

### 3. Idle wakeups from the command loop (resolved in 1.1)

The app drains `Command` with
`glib::timeout_add_local(Duration::from_millis(40))`. Measured 25
voluntary context switches/s and 0.2 % CPU in both null and video mode. This
is small CPU, but it prevents deep idle and adds 40 ms latency to every
D-Bus/UI command.

**Paths**

1. Replace polling with an event-driven channel: `async-channel` (or
   `glib::MainContext::channel`) + `glib::spawn_local` / `MainContext::channel`.
   All producers already own a `Sender<Command>`; only the receiver changes.
2. If a channel refactor is too large, raise the interval to 100–200 ms as a
   stop-gap (10–5 wakeups/s), at the cost of latency.
3. The X11 window watcher polls every 300 ms on X11 sessions (disabled on
   Wayland). Move it to `PropertyNotify`/`_NET_CLIENT_LIST` events later.

### 4. Unnecessary DING work (resolved in 1.1)

`App::rebuild_windows` calls `util::gnome_desktop_icon_workaround()`
unconditionally, including `MODE_NULL`. It queries
`org.gnome.Shell.Extensions` and can disable/enable DING (a ~1.4 s extension
reload in a background thread) even when no Ferramari window exists.

**Paths**

1. Call the workaround only when `!self.windows.is_empty()`.
2. If lowering the wallpaper window below the icon layer works, remove the
   DING toggle entirely. It is the expensive, fragile half of the workaround.
3. Keep the lowering itself: it is two `ConfigureWindow` calls per window.

### 5. Config writes on every volume-slider tick (resolved in 1.1)

`App::set_volume` calls `save_config()`, `sync_snapshot()` and `refresh_ui()`
for every `value-changed` emission. A slider drag can write the JSON file
dozens of times. The write is small, but it is avoidable disk I/O on a hot UI
path.

**Paths**

1. Debounce `save_config` by 300–500 ms while applying
   volume to mpv immediately.
2. Optionally coalesce volume commands: store the latest value and apply it on
   a timer instead of per pixel.

### 6. mpv is killed and restarted on every video switch

`WallpaperWindow` spawns a new mpv per mode/video switch and waits up to 3 s
for its IPC socket on the main thread. Normal cold start is ~120 ms; a failure
blocks the UI for the full timeout.

**Paths**

1. For video → video switches, reuse the existing mpv: `loadfile` via IPC,
   reset volume/position. Keep per-window state in `WallpaperWindow`.
2. Spawn mpv asynchronously or reduce the socket timeout; never block the GTK
   loop for 3 s on a broken binary.
3. For playlists/rotation this is required anyway.

### 7. Streaming re-resolves on every reload; mpv memory is untuned

`ytdlp::resolve` starts `yt-dlp -J` (~1.5–2.4 s, 592 KiB JSON) on every
stream start and every `Reload` in stream mode. mpv PSS is 159 MiB for one
1080p video (gpu-next + libplacebo + decoders).

**Paths**

1. Cache `StreamInfo` per URL for a short TTL (e.g. 10–30 min); invalidate on
   playback error. Restart/Reload then skips yt-dlp.
2. Expose a low-power profile: 30 fps cap, `--demuxer-max-bytes` /
   `--cache=no` for local files, hardware-decoder selection. Measure PSS/CPU
   per profile before making it the default.
3. Add `hwdec-current` to the planned `ferramari doctor` output; software
   decode at 4K is the real CPU cliff.

### 8. WebKit is a hard dependency

WebKitGTK is linked even in video/null mode. Runtime cost is modest (~7 MiB
PSS, no web process), but it is a heavy build dependency and maps a large
library at startup.

**Paths**

1. Make `webpage` a Cargo feature (`webkit2gtk` optional). Video-only builds
   lose the Web page tab and the dependency.
2. This is primarily a packaging/build win, not a runtime optimisation; do
   not expect a large RSS drop.
3. In web mode, pause/suspend the WebView on lock or maximize where WebKit
   allows it; not measured yet.

## Priority

| Priority | Change | Expected effect | Effort |
| --- | --- | --- | --- |
| Done | Async + cached static wallpaper | 0.4.0: worker thread and generation counter; UI no longer freezes | — |
| Done | Thumbnail seek/pool/cancel | 1.1: 2.2× on 1080p, 4.5× on 4K; two workers, stale results cancelled | — |
| Done | Debounce config writes | 1.1: nine slider changes → one write, flushed on quit | — |
| Done | Event-driven command loop | 1.1: 26/s → 1.2/s idle wakeups; no 40 ms command latency | — |
| Done | Conditional DING toggle | 1.1: skipped when there is no wallpaper window | — |
| Done | Reuse mpv for video→video | 1.1: `loadfile` keeps the PID; ~0.2 ms app time instead of ~120 ms | — |
| P2 | Stream resolution cache | 1.5–2.4 s saved per reload | S |
| P2 | mpv low-power/memory profile | lower CPU/GPU on 4K/60 and idle | M |
| P3 | X11 event watcher | 3.3 wakeups/s on X11 sessions | M |
| P3 | Optional WebKit feature | lighter builds/packaging | S–M |

## What not to optimise (measured)

- **Startup**: 48 ms to the mode decision; already excellent.
- **App memory**: 33 MiB PSS in video mode, 22 MiB null; do not chase.
- **`xdg-user-dir` subprocess**: 10 calls in 0.01 s; caching it is noise.
- **Blur itself**: ~0.06 s at 1080p; seek/decode is the cost.
- **Language/runtime**: both video paths are GPU/C; Rust is not the limit.
- **WebKit RSS in video mode**: ~7 MiB PSS; only relevant for build deps.
- **Single-monitor baseline**: two/three-monitor scenarios are still unmeasured;
  do not extrapolate this table to them.

## Re-measuring

```sh
# app/mpv RSS, PSS, threads
grep -E '^(Rss|Pss|Anonymous):' /proc/$(pgrep -x ferramari)/smaps_rollup

# CPU + voluntary context switches over 5 s
# (see the /proc/PID/stat + status delta used for the baseline)

# static wallpaper, 1080p
ffmpeg -y -v error -ss 00:00:05 -i VIDEO -frames:v 1 -vf gblur=sigma=5 /tmp/static.png

# thumbnails, old vs new (card size)
ffmpeg -y -v error -i VIDEO -ss 1 -vf 'thumbnail,scale=400:225:force_original_aspect_ratio=increase,crop=400:225' -frames:v 1 old.jpg
ffmpeg -y -v error -ss 1 -i VIDEO -vf 'scale=400:225:force_original_aspect_ratio=increase,crop=400:225' -frames:v 1 new.jpg
```