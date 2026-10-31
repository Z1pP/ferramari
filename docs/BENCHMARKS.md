# Benchmarks

`scripts/benchmark.py` starts an isolated Ferramari instance on a private
D-Bus session, applies a local video, samples the whole process tree for a
number of seconds, and optionally kills mpv once to measure recovery. It does
not touch the user's config, cache, or D-Bus name.

```sh
cargo build --release
make bench VIDEO="$HOME/Videos/Ferramari/sample.mp4" PROFILE=balanced SECONDS=10
```

Or directly:

```sh
DISPLAY=:0 python3 scripts/benchmark.py \
  --binary target/release/ferramari \
  --video ~/Videos/Ferramari/sample.mp4 \
  --profile balanced --seconds 10 --recover
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

## 2026-11-01 — 1.4.0, single 1080p display

Environment: Ubuntu 26.04, GNOME Shell 50.1, Wayland session through XWayland,
release build, one 1920×1080 display. Video: `Power-Dancing-1080p.mp4`. Eight
one-second samples after a 2.5 s warm-up. These numbers are relative
indicators, not a lab measurement.

| Scenario | Startup | Avg CPU | Peak RSS | Threads | Processes | GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Local 1080p, Balanced | 63 ms | 5.04 % | 258.4 MiB | 59 | 2 | 25 % |
| Local 1080p, Power/Eco base | 61 ms | 4.20 % | 257.5 MiB | 71 | 2 | 21 % |
| 1080p, mpv killed after 1 s | 61 ms | 6.85 %* | 258.6 MiB | 59 | 2 | 25 % |

\* The recovery run includes the 2 s window in which mpv is dead (0 % CPU) and
the restart spikes. `mpv_recovery_ms` was **2081 ms** (1 s backoff + process
start + IPC connect). No mpv child was left after the tested instance quit.

In this run the Power/Eco base reduced average CPU from 5.04 % to 4.20 % and
GPU from 25 % to 21 % on the same clip. An earlier run showed no difference, so
treat the gain as workload-dependent rather than a guarantee. The important
verified behaviour is that the idle cap is applied through the runtime `vf`
property (mpv 0.41 accepted `fps=5`/`fps=30` and cleared it with an empty
value) and that idle Eco pauses instead of decoding.

### Not measured

- 4K and 4K/60 sources;
- two and three monitors (the test machine has one display);
- Web wallpapers and WebKit memory;
- screen lock, fullscreen games and long idle on a real session;
- idle FPS cap on a live session (the private benchmark bus has no Mutter
  idle service, so it cannot exercise idle detection).

Record these before publishing energy claims. The manual matrix in
`docs/TESTING.md` covers the functional cases that the benchmark cannot.

## Historical baseline

The 1.0.0/1.0.1 baseline and optimisation notes are in
[`docs/PERFORMANCE.md`](PERFORMANCE.md). Those numbers were taken on a
different build and a different source; compare only within one table.