# Benchmark summaries

These are the compact JSON summaries behind the tables in
[`../BENCHMARKS.md`](../BENCHMARKS.md). They contain the environment block,
configuration, per-phase group metrics and the process inventory. The raw
one-second JSONL streams, CSV files, server logs and the 1080p/4K source clips
were intentionally not committed; regenerate them with the commands in
`BENCHMARKS.md`.

| File | Measurement |
| --- | --- |
| `baseline-2.2.5-1080p.json` | Original 2.2.5, Windows 1×1080p, 3×60 s steady |
| `baseline-2.2.5-2160p.json` | Original 2.2.5, Windows 1×2160p30, 60 s |
| `optimised-steady-1080p.json` | Final worktree, 3×60 s steady |
| `paused-before.json` / `paused-after.json` | 60 s paused, 2 ms vs 20 ms reader poll |
| `ui-before.json` / `ui-after.json` | Iced Home visible + hidden, before/after the hidden tick |
| `parked-1080p.json` | Occluded → parked → resumed, 1080p30 |
| `churn-before.json` / `churn-after.json` | 10 one-second covers, before/after hysteresis |
| `recover-1080p.json` | mpv kill and supervised recovery |
| `switch-1080p.json` | testsrc2 → Power-Dancing, log-confirmed reuse |
| `idle-balanced-1080p.json` | Balanced idle, 30 fps filter |
| `longrun-null-loop.json` | 12 s null-mode loop/deadline validation |

Caveats:

- **Windows only, one physical display.** The host has no Linux distribution
  and no second/third monitor. Do not infer Linux or multi-monitor numbers from
  these files.
- `parked-1080p.json` was taken after the Windows reader change but **before**
  the two-second occlusion confirmation. That confirmation only delays the
  pause that starts the five-minute park timer; the RSS/thread/VRAM drop is
  unaffected.
- `churn-before.json` and `churn-after.json` use a real window that covered the
  monitor. The before run used 1.5 s holds with a state wait; the after run used
  fixed 1.0 s holds. The transition count is the result; the elapsed times are
  not a CPU comparison.
- `longrun-null-loop.json` is a loop/deadline validation in `--mode null`, not
  a playback soak. A 24-hour run requires a dedicated machine.
- The `environment.binary` path is the build used for the run. The final
  optimized binaries are in the worktree's `target/release`; the summaries are
  not installation artifacts.
