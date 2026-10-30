# Performance baseline and optimisation paths

Measured 2026-10-26 on Ubuntu 26.04, GNOME Shell 50.1, Wayland session
(Ferramari runs through XWayland), release build (`ferramari 0.6.0`),
single 1920×1080 monitor. Numbers are relative indicators, not a benchmark
suite: warm page cache, one local 1080p H.264 video, one synthetic 4K clip.

The `diagnosing-bugs` perf branch was used: establish a baseline first, then
rank hypotheses and measure one variable at a time.

## Baseline

| Metric | Value | How |
| --- | --- | --- |
| Start → mode decision | 48 ms | journal timestamps |
| Start → mpv IPC ready | ~120 ms | poll for mpv socket |
| App RSS / PSS, video mode | 82 MiB / 33 MiB | `/proc/PID/smaps_rollup` |
| App RSS / PSS, null mode | 59 MiB / 22 MiB | isolated `dbus-run-session` instance |
| App CPU, video mode | 0.2 % | `/proc/PID/stat` delta, 5 s |
| App wakeups | 25/s | voluntary context switches, 5 s |
| mpv RSS / PSS | 258 MiB / 159 MiB | `/proc/PID/smaps_rollup` |
| mpv CPU, 1080p | 3.8 % | `/proc/PID/stat` delta, 5 s |
| WebKit PSS in video mode | ~7 MiB (linked, not initialised) | smaps per mapped file |
| Threads: video / null | 22 / 15 | `/proc/PID/task` |

## Ranked bottlenecks

### 1. Static wallpaper blocks the GTK main loop (0.7–1.6 s)

`App::update_static_wallpaper` calls `GnomeBackground::apply_static`, which
runs `ffmpeg` synchronously on the main thread. Journal gaps between
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

### 2. Thumbnails are 2.4× slower than necessary and unbounded

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

### 3. 25 idle wakeups/s from the command loop

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

### 4. Unnecessary DING work in modes without a wallpaper

`App::rebuild_windows` calls `util::gnome_desktop_icon_workaround()`
unconditionally, including `MODE_NULL`. It queries
`org.gnome.Shell.Extensions` and can disable/enable DING (a ~1.4 s extension
reload in a background thread) even when no Ferramari window exists.

**Paths**

1. Call the workaround only when `!self.windows.is_empty()`.
2. If lowering the wallpaper window below the icon layer works, remove the
   DING toggle entirely. It is the expensive, fragile half of the workaround.
3. Keep the lowering itself: it is two `ConfigureWindow` calls per window.

### 5. Config is written on every volume-slider tick

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
| P0 | Async + cached static wallpaper | removes 0.7–1.6 s main-loop freeze; GUI appears immediately | S–M |
| P0 | Thumbnail seek/pool/cancel | 2.7× faster cold GUI; no CPU spike on large libraries | S |
| P0 | Debounce config writes | fewer disk writes on volume drag | XS |
| P1 | Event-driven command loop | 25 wakeups/s → ~0; lower latency | S–M |
| P1 | Conditional/removed DING toggle | ~1.4 s background reload and flicker gone | XS–S |
| P1 | Reuse mpv for video→video | ~120 ms per switch; less memory churn | M |
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

## Re-measuring

```sh
# app/mpv RSS, PSS, threads
grep -E '^(Rss|Pss|Anonymous):' /proc/$(pgrep -x ferramari)/smaps_rollup

# CPU + voluntary context switches over 5 s
# (see the /proc/PID/stat + status delta used for the baseline)

# static wallpaper, 1080p
ffmpeg -y -v error -ss 00:00:05 -i VIDEO -frames:v 1 -vf gblur=sigma=5 /tmp/static.png

# thumbnails, old vs new
ffmpeg -y -v error -i VIDEO -ss 1 -vf thumbnail,scale=-1:96 -frames:v 1 old.jpg
ffmpeg -y -v error -ss 1 -i VIDEO -frames:v 1 -vf scale=-1:96 new.jpg
```