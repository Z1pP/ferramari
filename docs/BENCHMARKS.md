# Benchmarks

`scripts/benchmark.py` starts an isolated Ferramari instance on a private
D-Bus session, applies a local video or web wallpaper, samples the whole
process tree for a number of seconds, and optionally kills mpv once to measure
recovery. It does not touch the user's config, cache, or D-Bus name.

```sh
cargo build --release
make bench VIDEO="$HOME/Videos/Ferramari/sample.mp4" PROFILE=balanced SECONDS=10
make bench MODE=web WEB="$PWD/docs/demo.html" SECONDS=10
```

Or directly:

```sh
GDK_BACKEND=x11 python3 scripts/benchmark.py \
  --binary target/release/ferramari \
  --mode video \
  --video ~/Videos/Ferramari/sample.mp4 \
  --label 1080p --profile balanced --seconds 10 --recover

GDK_BACKEND=x11 python3 scripts/benchmark.py \
  --binary target/release/ferramari \
  --mode web --web /tmp/page.html --label canvas --seconds 10 \
  --app-log /tmp/ferramari-bench.log
```

The CSV option (`--csv results.csv`) writes one row per one-second sample. The
script reports:

- startup: spawn → the server claims its D-Bus name;
- CPU: process-tree ticks divided by elapsed wall time;
- RSS: sum of `VmRSS` for Ferramari, mpv and WebKit children;
- threads and process count from `/proc`;
- GPU: maximum `gpu_busy_percent` under `/sys/class/drm` (AMDGPU; otherwise
  blank);
- mpv recovery: time from `SIGKILL` to a new mpv pid, if `--recover` is used.

`--label` records the source resolution or scenario. A 4K run is an ordinary
video file with 3840×2160 frames; the harness cannot generate content. For a
local synthetic clip:

```sh
ffmpeg -f lavfi -i testsrc2=size=3840x2160:rate=30:duration=20 \
  -c:v libx264 -preset ultrafast -crf 24 -pix_fmt yuv420p /tmp/4k.mp4
```

## 2026-11-01 — 1.8.0, single 1080p display

Environment: Ubuntu 26.04, GNOME Shell 50.1, Wayland session through XWayland,
release build, one 1920×1080 display, AMDGPU. Ten one-second samples after a
3 s warm-up. These numbers are relative indicators, not a lab measurement.

| Scenario | Startup | Avg CPU | Peak RSS | Threads | Processes | GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Local 1080p H.264, Balanced | 120 ms | 4.40 % | 251.0 MiB | 61 | 2 | 25 % |
| Local 1080p H.264, Power/Eco base | 61 ms | 6.41 %* | 257.4 MiB | 74 | 2 | 6 % |
| Local 4K H.264 (synthetic), Balanced | 65 ms | 6.01 % | 317.6 MiB | 59 | 2 | 49–69 % |
| Local HTML canvas, Balanced | 121 ms | 37.35 % | 473.9 MiB | 112 | 3 | 72–78 % |

\* Two Power-profile runs averaged 5.07 % and 6.41 % CPU. In this build the
profile consistently reduced GPU utilisation (about 25 % → 6 %) but did not
reduce CPU on this clip. The earlier 1.4.0 table showed the opposite CPU
ordering, so treat CPU differences below roughly one percentage point as noise
and do not claim a CPU saving from the Power profile.

### What the scenarios contain

- **1080p video**: `Power-Dancing-1080p.mp4`, hardware decoding selected by
  mpv (`--hwdec=auto-safe`), one monitor.
- **4K video**: a 20 s `testsrc2` H.264 clip at 3840×2160/30. This is synthetic
  and compression-heavy; GPU load rose from 49 % to 69 % across the run while
  the clip changed. It is not representative of film content, 4K/60 or HDR.
- **Web**: a local HTML page with a 2D canvas, 600 moving particles and a
  requestAnimationFrame loop, 1920×1080. This is a deliberately busy page:
  WebKitGTK spent about 37 % of one core and kept the GPU at roughly three
  quarters utilisation. A static page or CSS animation will be much cheaper.

### Notes and anomalies

- Startup includes the isolated D-Bus session and the app's own readiness
  handshake; 60–120 ms is the observed range, not a regression signal.
- One earlier web run showed RSS fall from ~460 MiB to ~38 MiB while the
  process count stayed at three. It did not reproduce in two later runs and
  the app log contained no error. A WebKit process replacement is the likely
  explanation, but it was not confirmed; do not treat WebKit RSS as flat.
- Web wallpapers have no freeze API in WebKitGTK, so these numbers measure a
  visible web wallpaper. A hidden web wallpaper is muted but its timers are
  only throttled by WebKit.
- Decoder parking is not represented in the table: it needs five minutes of
  hidden playback. The end-to-end behaviour was verified with a temporary 10 s
  threshold (occlusion → parked at `time-pos=12.77`, same mpv PID → reveal →
  unparked), and the mpv protocol/worker order is unit-tested, but the
  wall-clock energy effect is unmeasured.

### Not measured

- Two and three monitors (the test machine has one display);
- 4K/60, HDR and real film content;
- WebGL/Unity/Godot pages and long-running WebKit memory;
- screen lock, fullscreen games and long idle on a real session;
- the idle FPS cap on a live session (the private benchmark bus has no Mutter
  idle service, so it cannot exercise idle detection);
- power draw in watts.

Record these before publishing energy claims. The manual matrix in
`docs/TESTING.md` covers the functional cases that the benchmark cannot.

## 2026-11-01 — 1.4.0, single 1080p display

Older baseline, kept for comparison within its own build. Environment was the
same except that this run used a 2.5 s warm-up. Video:
`Power-Dancing-1080p.mp4`. Eight one-second samples.

| Scenario | Startup | Avg CPU | Peak RSS | Threads | Processes | GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Local 1080p, Balanced | 63 ms | 5.04 % | 258.4 MiB | 59 | 2 | 25 % |
| Local 1080p, Power/Eco base | 61 ms | 4.20 % | 257.5 MiB | 71 | 2 | 21 % |
| 1080p, mpv killed after 1 s | 61 ms | 6.85 %* | 258.6 MiB | 59 | 2 | 25 % |

\* The recovery run includes the 2 s window in which mpv is dead (0 % CPU) and
the restart spikes. `mpv_recovery_ms` was **2081 ms** (1 s backoff + process
start + IPC connect). No mpv child was left after the tested instance quit.

## Historical baseline

The 1.0.0/1.0.1 baseline and optimisation notes are in
[`docs/PERFORMANCE.md`](PERFORMANCE.md). Those numbers were taken on a
different build and a different source; compare only within one table.
