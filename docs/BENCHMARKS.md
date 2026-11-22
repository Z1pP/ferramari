# Benchmarks

The harness measures an **isolated** Ferramari instance. Linux keeps the
original private D-Bus session; Windows uses a throwaway `%APPDATA%` and the
loopback control endpoint. It never writes the user's config, cache or library.
A value the platform cannot measure is `null` in JSON and an empty `CSV` cell;
it is never reported as zero.

The 2026-11-20 numbers below are the 2.2.5 baseline and the results of the 2.3
performance work. The machine is a single-display Windows laptop, so Linux and
two/three-monitor rows are explicitly **not measured** here. See
[`PERFORMANCE_AUDIT_2_3.md`](PERFORMANCE_AUDIT_2_3.md) for the audit and
[`PERFORMANCE_RESULTS.md`](PERFORMANCE_RESULTS.md) for the change-by-change
results. The compact JSON summaries are committed under
[`docs/benchmarks/`](benchmarks/README.md).

## Running the harness

Windows (PowerShell, from the repository root):

```powershell
cargo build --workspace --release --locked

python scripts/benchmark.py `
  --platform windows `
  --mode video --video C:\clips\1080p30-testsrc2.mp4 `
  --scenario steady --seconds 60 --warmup 5 --repetitions 3 `
  --json-out bench-results\steady.json `
  --jsonl-out bench-results\steady.jsonl `
  --csv-out bench-results\steady.csv
```

Linux:

```sh
cargo build --workspace --release --locked
python3 scripts/benchmark.py \
  --platform linux \
  --mode video --video ~/Videos/1080p30-testsrc2.mp4 \
  --scenario steady --seconds 60 --warmup 5 --repetitions 3 \
  --json-out bench-results/steady.json
```

The harness itself has dependency-free unit tests:

```sh
python -m unittest discover -s scripts/bench -p 'test_*.py'   # or make test-bench
```

A long run uses the same binary and writes a compact soak report in addition
to JSONL: RSS slope per hour, thread/process first/last/max, mpv PID changes,
dropped-frame delta, monitor-state counts, `[Player]`/`[mpv]`/`[Eco]` log
counters and each scheduled recovery latency.

```sh
make soak VIDEO="$HOME/Videos/1080p30-testsrc2.mp4" DURATION=86400
```

The cover event is fullscreen; run the soak on a dedicated display or with the
desktop otherwise clear. The 2.2.5 host did not have one, so the 24-hour row is
not in this document.

The Windows single-instance mutex is session-wide. Close the running Ferramari
before a measurement; the harness refuses to start when another server owns
the endpoint. A window that already covers the wallpaper on a one-display
desktop can be minimized for the run and restored afterwards:

```powershell
python scripts/benchmark.py ... --minimize-window <PID>
```

This is required for `occluded`, `parked` and `churn` on a desktop where the
editor/browser window covers the monitor. The harness restores the window even
when the run fails.

### Scenarios

| Scenario | What it does |
| --- | --- |
| `steady` | Apply a local video or stream and sample playback. |
| `paused` | Pause playback but keep the process and IPC alive. This isolates the IPC reader and supervision cost. |
| `idle` | Set the idle timeout low and wait for the window state to become `idle`. On Windows the real `GetLastInputInfo` is used, so the machine must stay idle. Linux accepts `--idle-stub`, a test helper that owns `org.gnome.Mutter.IdleMonitor` on the private bus. |
| `occluded` | Start a fullscreen cover on the target monitor, wait for `Occluded`, then remove it and measure the resume. |
| `parked` | Cover the monitor, wait for the five-minute decoder park, measure mpv RSS/VRAM/threads, then uncover and measure the resume and position continuity. |
| `recover` | Kill one mpv process and measure the time to a new PID and to the first frame. |
| `switch` | Apply a second local video and verify that mpv was reused rather than restarted. |
| `ui` | Start the Iced control surface on a chosen page, measure it visible, then hide it to the tray and measure it again. Windows-only hidden phase. |
| `churn` | Minimize/restore a real covering window ten times. It measures how many Eco pause/resume transitions a short cover still causes. |
| `long-run` | Sample for `--duration` and periodically kill mpv, cover/uncover or reload. The 24-hour command is `--duration 86400`. |

### What is measured

- **CPU** — process-tree ticks over wall time, split into `ferramari` (server),
  `mpv`, other children and `total`. The report shows percent of one logical
  core and percent of the machine. On Windows, one-second process counters are
  quantised by the scheduler tick; the summary also records a whole-phase CPU
  (`phase_cpu`) computed from the first and last `GetProcessTimes` values, which
  is the number to use for a 60 s run. The original 2.2.5 table below uses the
  mean of one-second samples, as the old harness did; the optimisation runs use
  `phase_cpu`.
- **RSS / PSS / working set** — `VmRSS` per process on Linux, optional PSS from
  `smaps_rollup`, and `WorkingSetSize` on Windows. Windows has no PSS; the
  column stays empty.
- **GPU** — per-process utilization from the Windows `GPU Engine` PDH instances
  and the busiest engine globally; AMDGPU `gpu_busy_percent` or a cached
  `nvidia-smi` sample on Linux. The global number includes every application
  on the desktop and is context only; the `mpv` row is the attributable one.
- **VRAM** — Windows `GPU Adapter Memory` and `GPU Process Memory`
  (`Dedicated Usage`); AMDGPU `mem_info_vram_used` or `nvidia-smi` on Linux.
  Per-process VRAM is Windows-only.
- **FPS / dropped frames** — the primary player's `estimated-vf-fps` and
  `frame-drop-count` from the snapshot. Non-primary players are not probed
  every second: that would multiply IPC traffic for a value the UI does not
  display. A 2/3-monitor row therefore reports the primary FPS and marks the
  other players as not measured, rather than inventing a number.
- **Threads and processes** — from Toolhelp on Windows and `/proc` on Linux.
- **Recovery / park / resume** — time to a new mpv PID, time to the first frame,
  and the park/unpark markers from the server log.

## Environment

| Item | Value |
| --- | --- |
| OS | Windows 11, build 26200 (Windows 10.0.26200) |
| Machine | AMD Ryzen 5 5600H, 6 cores / 12 threads, 14.2 GiB usable RAM |
| GPU | AMD Radeon Graphics (31.0.21912.3005) + NVIDIA GeForce RTX 3050 Laptop (32.0.16.2101) |
| Display | one 1920×1080 physical panel at 125% scale (1536×864 logical), 120 Hz |
| Build | Ferramari 2.2.5, commit `742b49d` |
| mpv | 0.42.0 |
| ffmpeg / ffprobe | 9.0.2 |
| yt-dlp | 2026.10.26 |
| Source | 1080p30 `testsrc2` H.264, 20 s loop, 1920×1080 (`sha256 e0236f…b6e3`) |
| 4K source | 2160p30 `testsrc2` H.264, 20 s loop, 3840×2160 |
| Profile | Balanced, `hwdec=auto` (`d3d11va` reported) |
| Isolation | throwaway `APPDATA`/`LOCALAPPDATA`, static wallpaper off, muted |
| Samples | 3 runs × 60 s at 1 s after a 5 s warm-up; AC power |

The generated clips are reproducible:

```sh
ffmpeg -f lavfi -i testsrc2=size=1920x1080:rate=30:duration=20 \
  -c:v libx264 -preset ultrafast -crf 24 -pix_fmt yuv420p 1080p30-testsrc2.mp4
ffmpeg -f lavfi -i testsrc2=size=3840x2160:rate=30:duration=20 \
  -c:v libx264 -preset ultrafast -crf 24 -pix_fmt yuv420p 2160p30-testsrc2.mp4
```

## 2.2.5 baseline — 1080p30, one display

Three 60 s runs, 178 one-second samples. The `ferramari` row is the server
process only, not the whole tree. `mpv` is one process with forty-ish threads.
`children` is the launch-dependent `conhost.exe`; it is kept separate so it
does not look like Ferramari's own allocation.

| Group | CPU % one core (mean) | p95 | RSS MiB | Threads | Processes | GPU % | VRAM MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ferramari | 1.07 | 4.55 | 12.5 | 11 | 1 | — | — |
| mpv | 5.03 | 10.97 | 134.9 | 40 | 1 | 11.3 | 25.1 |
| children (`conhost`) | 0.01 | 0.00 | 8.6 | 2 | 1 | — | — |
| **total** | **6.10** | **12.11** | **156.0** | **54** | **3** | **11.3** | **25.1** |

FPS 30.0, dropped frames 0, display 120 Hz. Machine CPU is 0.51% of twelve
logical processors. The global GPU counter showed 12.3% median during these
runs; it includes other desktop applications and is not used as the app's GPU
number. Table medians are per group; the **total** row is its own median and
can differ by a thread from the sum of the group medians.

### 4K30 baseline

One 60 s run, same machine and source generator at 3840×2160:

| Group | CPU % one core (phase) | RSS MiB | Threads | GPU % | VRAM MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| ferramari | 0.75 | 12.5 | 10 | — | — |
| mpv | 8.12 | 201.4 | 49 | 39.4 | 142.5 |
| children (`conhost`) | 0.00 | 8.6 | 2 | — | — |
| **total** | **8.88** | **222.4** | **61** | **39.4** | **142.5** |

FPS 30.0, dropped 0. 4K nearly doubles mpv working set and pushes the
attributable GPU load to about 39%. This is a synthetic H.264 clip; film
content, 4K60 and HDR are not represented.

## 2.3 optimisation result — steady 1080p30

Same workload and three-run protocol, final code (reader poll, UI hidden tick,
Eco hysteresis and mute guards):

| Group | CPU % one core (phase median) | phase mean | RSS MiB | Threads | Processes | GPU % | VRAM MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ferramari | 0.65 | 0.71 | 12.5 | 11 | 1 | — | — |
| mpv | 5.88 | 6.21 | 135.5 | 40 | 1 | 11.6 | 25.1 |
| children (`conhost`) | 0.00 | 0.00 | 8.6 | 2 | 1 | — | — |
| **total** | **6.48** | **6.91** | **156.6** | **53** | **3** | **11.6** | **25.1** |

FPS 30.0, dropped 0, RSS and thread counts unchanged. The total is dominated
by mpv and is statistically the same as the baseline; the server row and the
paused scenario below are where the reader optimisation is visible. A steady
scene cannot claim a playback CPU saving.

### Paused — the reader result

60 s paused, same clip, one run each:

| Metric | Before (2 ms poll) | After (20 ms poll) |
| --- | ---: | ---: |
| ferramari CPU % one core (phase) | 1.82 | 0.65 |
| mpv CPU % one core (phase) | 0.21 | 0.31 |
| total CPU % one core (phase) | 2.03 | 0.96 |
| RSS total MiB | 154.4 | 154.3 |
| FPS / dropped | 30 / 0 | 30 / 0 |

The server was waiting on mpv with no events, which is exactly the case the
Windows named-pipe reader used to poll. The 10 ms intermediate run measured
0.99% server / 1.28% total; 20 ms was kept because it cut the wake-ups further
without approaching the 50–100 ms fade interval.

### UI visible and hidden

Iced control surface on Home, 20 s visible and 20 s hidden to the tray. The
hidden phase on Windows is a real close-to-tray; the process and its 30 s
server-health poll remain.

| Phase | UI CPU % one core (phase) before | after | UI RSS MiB before | after | UI threads before | after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Home visible | 1.79 | 1.25 | 347.7 | 350.5 | 58 | 58 |
| Hidden tray | 1.25 | 0.55 | 347.9 | 348.1 | 57 | 56 |

The remaining hidden cost is the one 30 s health poll; its snapshot update
shows as a short spike every thirty seconds. The visible UI cost includes the
3.8 ms metrics sample once a second; it was measured but left unchanged because
the Windows process-table walk, not GPU sampling, dominates and the hidden
case is the one that runs for hours.

### Eco churn

Ten one-second covers separated by one second of visibility, using a real
window that covers the monitor. A two-second confirmation means a short cover
is never treated as an occlusion.

| Metric | Before | After |
| --- | ---: | ---: |
| `[Eco]` transitions / 10 cycles | 20 | **0** |
| server CPU % one core during churn | 0.68 (22.9 s) | 0.54 (20.1 s) |

The transition count is the meaningful result: after the change the wallpaper
never pauses for a cover shorter than two seconds, so no pause/resume fade is
started. A cover that stays for more than two seconds still pauses; a real
fullscreen game is unaffected because the wallpaper is already hidden.

### Decoder parking — 1080p30

Cover for five minutes on the only display, then uncover. Same mpv PID
(12592) throughout; the log contains `parked the decoder at Some(8.533333)`
and `decoder unparked`.

| Phase | mpv CPU % one core | mpv RSS MiB | mpv threads | mpv VRAM MiB | total RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Occluded, still loaded | 0.78 | 133.9 | 42 | 25.05 | 155.1 |
| Parked (file unloaded) | 0.39 | **63.1** | **26** | **0** | **84.5** |
| Resumed | 6.01 | 135.9 | 41 | 25.05 | 157.3 |

Parking frees about 70.8 MiB (53%) of mpv RSS, 16 threads and all of the
measured VRAM for this clip, and the resumed position continues from the
captured 8.53 s. The five-minute threshold was not changed.

## Recovery, switch and idle

These are single live runs of the final worktree, not part of the three-run
steady table. They close the scenarios the old harness could not express.

### mpv recovery

`--scenario recover`: one SIGKILL/`taskkill` of the 1080p30 player.

| Metric | Value |
| --- | ---: |
| Time to a new mpv PID | 2016 ms |
| Time to the first frame after the kill | 4063 ms |
| FPS after recovery | 30.0 |
| Dropped frames after recovery | 0 |
| mpv working set before → after | 134.3 → 133.2 MiB |
| mpv threads before → after | 42 → 41 |

The restart policy's one-second backoff accounts for half the PID latency; the
new IPC connection and first decode account for the rest. No orphan process
remained after quit.

### Video switch reuses the player

`--scenario switch`: 1080p30 `testsrc2` → the local `Power-Dancing-1080p.mp4`.

| Metric | Value |
| --- | ---: |
| mpv PID reused | **yes** |
| Reuse confirmation in the server log | 109 ms |
| First frame of the new file | 2031 ms |
| mpv working set after switch | 154.0 MiB |
| FPS / dropped | 30 / 0 |

The server logged `reused player in 2.4 µs`; the measured 109 ms is dominated by
the harness's 100 ms log poll, not by the app. An earlier version of this run
reported 15 ms by reading the previous file's still-active stats; the
log-confirmed number above is the honest one.

### Idle — Balanced

`--scenario idle --idle-timeout 30`, 20 s after the server reported
`[Eco] DISPLAY1: Idle (play=true, fps=Some(30))`.

| Group | CPU % one core (phase) | RSS MiB | Threads | GPU % | VRAM MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| ferramari | 1.17 | 12.5 | 13 | — | — |
| mpv | 6.17 | 136.1 | 52 | 11.4 | 9.0 |
| children (`conhost`) | 0.00 | 8.6 | 3 | — | — |
| **total** | **7.34** | **157.2** | **68** | **11.4** | **9.0** |

FPS 30, dropped 0. The idle policy applied the 30 fps filter, but on this
hardware it did **not** reduce CPU or GPU versus steady playback; mpv's thread
count rose from about 40 to 52 because of the filter chain, and VRAM fell from
25.1 to 9.0 MiB. Do not claim an idle CPU saving from this row. The cap remains
because the policy is profile-driven and the measurement is one clip on one
GPU; a different decoder path may respond differently.

### 24-hour dry run — blocked by a real fullscreen app

The attempt with `--scenario long-run --duration 90 --event-interval 60`
stopped at the first-frame gate because a real Google Chrome window was
fullscreen on the only display. The isolated server correctly reported
`Occluded` and refused to call the player "playing". That is the intended Eco
behaviour, but it means a long-run on a working desktop is not a valid
measurement. A 12 s null-mode run validated the sampling loop and the deadline
bound: an event past the deadline is not started, and a 60 s cover is skipped
when fewer than 65 s remain. No 24-hour number was produced; the full run still
needs a dedicated machine or a desktop with no fullscreen app.

## Microbenchmarks

Ignored tests, run only on a benchmark host:

```sh
cargo test --release -p ferramari-core metrics_sample_cost -- --ignored --nocapture
cargo test --release -p ferramari-core metrics_sample_breakdown -- --ignored --nocapture
cargo test --release -p ferramari-core linux_read_tree_fake_before_after -- --ignored --nocapture
cargo test --release -p ferramari-platform window_state_sample_cost -- --ignored --nocapture
```

Windows results (200–1000 iterations, release):

| Function | mean | p50 | p95 |
| --- | ---: | ---: | ---: |
| `metrics::Monitor::sample_of` | 3.843 ms | 3.773 ms | 4.561 ms |
| `read_tree` (Windows branch) | 3.271 ms | 3.242 ms | 3.826 ms |
| Windows GPU sampler | 0.331 ms | 0.331 ms | 0.359 ms |
| `total_memory` | 0.001 ms | — | — |
| `window_state::sample` | 0.146 ms | 0.141 ms | 0.166 ms |
| Linux fake `/proc`, 500 PIDs, old stat+status | 45.46 ms | — | — |
| Linux fake `/proc`, 500 PIDs, stat-only | **27.30 ms** | — | — |

The Linux row is a synthetic 500-PID `/proc`-shaped tree (50 iterations each),
not a Linux kernel run; it demonstrates the halved file opens and the removed
`status` parse (1.66×). A real Linux `strace -c`/UI-CPU run remains pending.

The Windows process-table walk is the cost; the GPU counter and the monitor
rectangles are not. `window_state::sample` runs at 1.33 Hz and costs about
0.02% of one core, so its internal `monitors::list()` was left alone. The
Windows metrics sample is paid only while the control surface is visible on
Home or Performance; the hidden-tick change removes it from the tray path.

## Not measured on this host

- **Linux**: no Linux distribution is installed on this machine. The harness
  Linux backend, `cover_window` and `idle_stub` examples are present, and the
  stat-only `/proc` sampler is implemented and covered by fake-tree tests plus
  a synthetic 500-PID before/after (45.46 ms → 27.30 ms, 1.66×). No **real**
  Linux playback, `strace`, UI-CPU or `metrics_sample_cost` number was taken;
  the Linux CI job compiles the workspace and examples.
- **Two and three physical displays**: the host has one. The harness refuses a
  `--monitors N` mismatch rather than extrapolating. The numbers above must not
  be multiplied by display count.
- **4K60, HDR, film content**: the 4K row is synthetic 30 fps H.264.
- **Screen lock, suspend/resume, real battery**: Windows lock/sleep and a
  charger transition are covered by the smoke checklist, not by this table.
- **Power draw in watts**: not measured. CPU, GPU, VRAM and the park/resume
  costs are the available proxies.
- **Per-monitor FPS/dropped on 2/3 displays**: only the primary player is
  probed; see the note above. This is a documented limitation, not a zero.

## Historical data

The 1.8.0 table (2026-11-01, Ubuntu, one 1080p display, AMDGPU) remains in the
git history and in the older [`PERFORMANCE.md`](PERFORMANCE.md) notes. It was
taken on a different build and machine, and its **web rows refer to the WebKit
engine that 2.x removed**. Do not compare it with the 2.2.5 table. The 1.4.0
table is historical for the same reason.
