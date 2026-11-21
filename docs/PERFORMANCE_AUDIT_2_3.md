# Ferramari 2.3 — performance audit

**Audit date:** 2026-11-19/20  
**Base:** Ferramari 2.2.5, commit `742b49d` (`origin/main`)  
**Scope:** measurement harness, metrics sampling, media runtime, Eco Engine,
long-running behaviour. This is the audit and the decisions; the measured
before/after tables are in [`BENCHMARKS.md`](BENCHMARKS.md) and the change log
is in [`PERFORMANCE_RESULTS.md`](PERFORMANCE_RESULTS.md).

## Environment and baseline availability

The only available machine is a Windows 11 build 26200 laptop with one
1920×1080 display at 125% scale (1536×864 logical), an AMD Ryzen 5 5600H, an
AMD Radeon iGPU and an NVIDIA RTX 3050 Laptop GPU. A Linux distribution is not
installed and there is no second or third display.

Consequences, recorded rather than extrapolated:

- The 2.2.5 baseline is **Windows, one display**. Linux, two/three monitors,
  lock/suspend and the 24-hour run are **not measured** on this host.
- The old 1.8.0 table is from a different Linux machine and a different build.
  Its web rows refer to the WebKit engine that 2.x removed. It is not a 2.2.5
  baseline.
- The harness refuses `--monitors N` when the host reports a different count.
  It does not clamp or invent a display.
- A synthetic 1080p30 and a synthetic 2160p30 H.264 `testsrc2` clip make the
  workload reproducible. Film content, 4K60 and HDR are not represented.

`docs/REVIEW_2_2.md` (R-01…R-15) covers correctness and reliability; none of
those fixes is repeated here. The `cached_playback_stats` and `is_decoding`
work from that review is the baseline this audit builds on.

## What the harness used to do

`scripts/benchmark.py` was Linux/X11-only: `os.sysconf`, `/proc`, `gdbus` and
`dbus-daemon` were imported at module load. It had a `web` mode that called a
D-Bus method whose engine deliberately returns “the GTK-free web engine is not
ready”; the `Makefile` and `docs/TESTING.md` still advertised that scenario.
It reported one aggregate CPU/RSS/process count, the number of mpv processes,
and an optional AMDGPU percentage. It had no FPS, dropped-frame, VRAM,
per-process split, idle/occluded/parked/resume scenario, monitor-count check or
environment block, and its CSV had no missing-value representation.

The rewrite keeps the original Linux isolation but adds a Windows backend
(throwaway `APPDATA`, loopback control endpoint, Toolhelp, PDH) and removes the
web scenario instead of measuring an error path.

## Findings

| ID | Finding | Evidence | Decision |
| --- | --- | --- | --- |
| P1 | Windows mpv IPC reader polled the named pipe every 2 ms inside a 100 ms timeout. A paused player therefore woke the process tens of times per second for no data. | `ferramari-media/src/mpv.rs`, `transport::PipeStream::read`. Paused 1080p30, 60 s: server **1.82% → 0.65%** of one core, total **2.03% → 0.96%** after a 20 ms poll; FPS 30 and dropped 0 unchanged. | **Fixed.** 20 ms poll; the fade interval is 50–100 ms, so reply latency stays inside the app's own command budget. |
| P2 | The Iced control surface ticked once a second while hidden to the tray: it sampled the process tree, advanced the sparklines and rebuilt the page. | `ferramari-ui/src/main.rs`, `Message::Tick`/`subscription`. Live 2.2.5 UI hidden: 1.4% of one core, 55 threads, ~150 MiB. Harness Home: hidden **1.25% → 0.55%** phase CPU after a 30 s health-only tick. | **Fixed.** Hidden windows poll the server every 30 s and skip metrics/history. The tray-show focus event restores the 1 s tick. |
| P3 | Every `WindowState` (any window move/resize) called `apply_mute_states`, which restarted a volume fade and sent `set_mute` even when the mute target was identical, and `apply_playback_states` always rebuilt the snapshot. | `src/app.rs`, `Command::WindowState`, `apply_mute_states`, `apply_playback_states`. Ten one-second covers produced **20 Eco transitions** and repeated IPC before the guards. | **Fixed.** Desired (muted, volume) is remembered per window and the snapshot is published only when a plan changed. |
| P4 | Short covers produced a pause/resume fade. A window crossing the monitor edge or a transient fullscreen prompt could start the fade, then immediately reverse it. | Same churn scenario. After an asymmetric two-second confirmation: **0 transitions / 10 cycles**. Uncovering still applies immediately. | **Fixed.** `OCCLUSION_CONFIRM = 2 s` for hidden→paused only; the one-second supervision tick applies the pending decision. |
| P5 | Windows `metrics::Monitor::sample_of` costs 3.84 ms (p95 4.56 ms) per call. The process table is 3.27 ms of that; the GPU counter is 0.33 ms and total memory 0.001 ms. | Ignored benchmark `metrics_sample_cost`/`metrics_sample_breakdown`. Paid once a second only while the UI is visible on Home/Performance. | **Measured, no change.** The hidden path no longer pays it; replacing Toolhelp with an undocumented NtQuery structure is not justified for a visible-page cost below half a percent of one core. |
| P6 | Linux `metrics::read_tree` reads `/proc/<pid>/stat` **and** `/proc/<pid>/status` for every PID in the system, then keeps only the descendants. | `ferramari-core/src/metrics.rs`. `stat` already contains PPID, CPU ticks, thread count and RSS in pages. | **Fixed (stat-only), synthetic before/after.** A 500-PID fake `/proc` tree: 45.46 ms → 27.30 ms (1.66×), file reads 2N → N. Fake-tree unit tests prove no `status` file is needed. Real Linux wall-clock and UI CPU remain unmeasured without a Linux host; the descendant-only walk was not added. |
| P7 | The Windows window-state sampler calls the full `monitors::list()` (DC, DPI, EDID, display settings) every 750 ms although it only needs monitor rectangles. | `crates/ferramari-platform/src/windows/window_state.rs`, `sample`. Measured **0.146 ms mean / 0.166 ms p95**, about 0.02% of one core at 1.33 Hz. | **Measured, no change.** The hypothesis was reasonable but the cost is below the noise floor. |
| P8 | `ProbeStats` sends 11 sequential `get_property` round trips once a second, and only for the primary player. Per-monitor FPS/dropped is therefore absent from the snapshot. | `ferramari-media/src/mpv.rs`, `probe_stats`; `src/app.rs`, `poll_hwdec`. Server CPU while paused is dominated by the reader/supervision, not by 11 small replies. | **No change.** Adding a probe per player would multiply IPC for a value the UI does not show. The limitation is documented in `BENCHMARKS.md`. |
| P9 | Decoder parking (five minutes hidden) was implemented but its RAM/VRAM/resume benefit was unmeasured. | Parked 1080p30 run: same mpv PID, mpv RSS **133.9 → 63.1 MiB**, threads **42 → 26**, VRAM **25.05 → 0 MiB**, total RSS **155.1 → 84.5 MiB**, resumed position continues from 8.53 s. | **Measured, no change.** The existing threshold is effective; no new policy was invented. |
| P10 | One mpv process and two IPC threads per monitor. Merging players would require a different renderer and is outside this phase. | One-display measurements only; 1080p mpv 135 MiB / 40 threads, 4K 201 MiB / 49 threads. | **No merge.** No 2/3-display evidence exists, and the brief forbids a merge without it. |
| P11 | The old harness had no 24-hour scenario and no leak/recovery assertions. | `scripts/benchmark.py` `long-run` now samples for `--duration`, kills mpv, covers/uncover and reloads on an event interval. | **Implemented, 24 h not run.** A 90 s dry run was blocked by a real fullscreen Chrome window; the isolated player correctly reported `Occluded`, which is the intended Eco behaviour but invalidates a long-run on this desktop. The 24-hour run needs a dedicated machine or exclusive desktop use. |
| P12 | A `conhost.exe` child (about 8.6 MiB, two threads) appears in the server tree when the server is launched from a console. It is one per server, not one per mpv. | Process inventory from the harness and the live launch. Adding `CREATE_NO_WINDOW` to mpv did not remove it because the console belongs to the server; that change was reverted. | **Documented, no code change.** The launcher path and a detached server are launch-specific; a single console host is not a per-monitor leak. |

## Baseline and result summary

- 1080p30 Balanced, 2.2.5: server 1.07% of one core, mpv 5.03%, total 6.10%,
  total RSS 156.0 MiB, 54 threads, 3 processes, mpv GPU 11.3%, VRAM 25.1 MiB,
  30 fps, 0 dropped.
- 2160p30 Balanced, 2.2.5: total 8.88% of one core (phase), total RSS
  222.4 MiB, 61 threads, mpv GPU 39.4%, VRAM 142.5 MiB, 30 fps, 0 dropped.
- After the reader/UI/Eco changes, 1080p30 steady: total 6.48% phase median,
  RSS 156.6 MiB, 53 threads, 30 fps, 0 dropped. The total is dominated by mpv
  and is statistically unchanged; the server reader and the hidden UI are the
  completed wins.
- Parked 1080p30: mpv RSS 63.1 MiB and 26 threads while unloaded; resumes at
  the captured position.

The full tables, per-group p95 values, UI visible/hidden rows, churn counts and
microbenchmarks are in [`BENCHMARKS.md`](BENCHMARKS.md).

## Limitations and unsupported scenarios

| Scenario | Status | Reason |
| --- | --- | --- |
| Linux playback, Linux metrics before/after | not measured | No Linux distribution on the host; the CI job compiles the Linux target. |
| Two and three monitors | not measured | One physical display. The harness refuses an explicit mismatch. |
| 4K60, HDR, film content | not measured | Synthetic 4K30 H.264 is the only 4K source used. |
| Lock, sleep/resume, charger transition, real idle | not measured here | Smoke checklist items; a test instance must not lock the user's session. |
| 24-hour soak | implemented, not run | Requires a dedicated machine or a day of exclusive desktop use. |
| Power in watts | not measured | CPU/GPU/VRAM and park/resume costs are the proxies. |
| Native Wayland | unsupported | X11/XWayland is the backend; unchanged by this phase. |
| Web wallpapers | not a benchmark scenario | The engine is absent in 2.x; the harness has no web mode. |

## Method notes

- CPU is reported both as percent of one logical core and percent of the
  machine (twelve logical processors here). The old table's percentages were
  also one-core percentages; that is now stated explicitly.
- Windows one-second process counters are quantised by the scheduler tick. A
  whole-phase delta (`phase_cpu`) is the primary number for 60 s runs; the
  per-sample mean is kept for compatibility with the original 2.2.5 table.
- RSS on Windows is the working set; PSS is Linux-only and stays empty on
  Windows. The global GPU/VRAM counters include other desktop applications;
  per-process rows are the attributable ones.
- The source clips are generated with the commands in `BENCHMARKS.md`; the
  baseline 1080p hash is recorded in the JSON. Each run uses a throwaway config
  with static wallpaper off, mute on and smart pause off for `steady`.
- The desktop had to be temporarily uncovered for `occluded`/`parked`/`churn`.
  The harness minimizes a named covering window with `--minimize-window PID`
  and restores it in `finally`, including on failure.
