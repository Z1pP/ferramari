# Ferramari 2.3 — performance results

**Date:** 2026-11-20  
**Base:** 2.2.5, commit `742b49d`  
**Rule applied:** a runtime change is kept only when a measurement shows a
benefit and playback remains correct. “Measured, no change” is a result too.

The full measurement tables and environment are in [`BENCHMARKS.md`](BENCHMARKS.md);
the findings are in [`PERFORMANCE_AUDIT_2_3.md`](PERFORMANCE_AUDIT_2_3.md).

## Summary

| Change | Status | Measured effect |
| --- | --- | --- |
| Windows mpv IPC reader: 2 ms → 20 ms poll | **kept** | Paused 1080p30, 60 s: server **1.82% → 0.65%** of one core; total **2.03% → 0.96%**. FPS 30, dropped 0. |
| UI hidden to tray: 30 s health poll, no metrics/history/redraw | **kept** | Home hidden: **1.25% → 0.55%** of one core. Visible UI unchanged at 1.25–1.5%. |
| Eco mute guard + snapshot-on-change only | **kept** | Ten geometry covers: repeated mute fades removed; snapshot rebuilds only on a plan change. |
| Eco occlusion hysteresis (2 s hide confirmation) | **kept** | Ten one-second covers: **20 transitions → 0**. Uncover remains immediate. |
| Windows window-state sampler cost | **measured, no change** | 0.146 ms mean / 0.166 ms p95 at 1.33 Hz (~0.02% of one core). |
| Windows metrics sample cost | **measured, no change** | 3.843 ms mean / 4.561 ms p95; process table is 3.271 ms, GPU 0.331 ms. Paid only while the UI is visible on Home/Performance. |
| Linux `/proc` sampler | **deferred** | No Linux host for before/after. Safe stat-only/descendant design documented below; not implemented. |
| Decoder parking | **measured, no change** | mpv RSS 133.9 → 63.1 MiB, threads 42 → 26, VRAM 25.05 → 0; resume at the captured position. |
| mpv process merging | **not attempted** | One-display evidence only; the brief forbids a merge without proof. |
| 24-hour soak | **implemented, not run** | `--scenario long-run --duration 86400`; needs a dedicated machine or consent. |

## 1. Windows IPC reader

### Problem

`crates/ferramari-media/src/mpv.rs` uses a synchronous named-pipe handle and
implements a deadline by peeking and sleeping. The loop slept for 2 ms and was
woken until the 100 ms `READER_POLL` deadline, then `BufReader::read_line`
retried. A paused wallpaper has no events, so the reader was awake with no work
to do. This is the dominant server cost while a player is paused.

### Change

The sleep is 20 ms instead of 2 ms. The deadline, the 1 s command timeout and
the fatal-error handling are unchanged. Commands are tiny; the app's fades run
at 50–100 ms intervals (`fade_interval` default 0.1 s), so a reply delayed by
at most 20 ms remains inside one fade step. An intermediate 10 ms run measured
0.99% server / 1.28% total; 20 ms measured 0.65% / 0.96% and was kept.

### Before/after

60 s paused, 1080p30 H.264, same machine, no UI:

| Metric | 2 ms | 10 ms | 20 ms |
| --- | ---: | ---: | ---: |
| server CPU % one core (phase) | 1.82 | 0.99 | **0.65** |
| mpv CPU % one core (phase) | 0.21 | 0.29 | 0.31 |
| total CPU % one core (phase) | 2.03 | 1.28 | **0.96** |
| RSS total MiB | 154.4 | 154.3 | 154.3 |
| FPS / dropped | 30 / 0 | 30 / 0 | 30 / 0 |

### Verification

- Steady 1080p30: 30 fps, 0 dropped, mpv working set ~135 MiB and ~40 threads
  before and after.
- `cargo test --workspace --locked`, playback smoke, pause/resume and the
  parked/unparked sequence are run in the quality gates.
- A future regression can be caught by the paused scenario: server phase CPU
  should stay well below the old 1.8% on this class of machine.

## 2. Hidden control surface

### Problem

`ferramari-ui` returns to the tray on Windows close. Its subscription still
fired once a second; `Message::Tick` sampled the server's process tree (3.8 ms
on Windows), advanced the sparklines and rebuilt/re-rendered the hidden page.
The live 2.2.5 process used 1.4% of one core while hidden with a ~150 MiB
working set.

### Change

`subscription` uses a 30 s period while `hidden`, and `Message::Tick` returns
after a server snapshot without sampling metrics, recording history or probing
the Library. The `Focused` event, which is what the tray raise produces, sets
`hidden = false` and restores the one-second period. The 30 s snapshot still
detects a dead server and closes the orphaned surface.

### Before/after

Iced UI on Home, 20 s visible and 20 s hidden:

| Phase | UI CPU % one core before | after | UI RSS MiB before | after | Threads before | after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Home visible | 1.79 | 1.25 | 347.7 | 350.5 | 58 | 58 |
| Hidden tray | 1.25 | **0.55** | 347.9 | 348.1 | 57 | 56 |

The residual hidden cost is the once-per-30-seconds snapshot and redraw; the
one-second metrics sample is gone. The visible UI cost is not claimed as
improved; the 3.8 ms metrics sample remains.

## 3. Eco work on the hot path

### Problem

`Command::WindowState` is produced whenever any visible window moves. It called:

- `apply_playback_states` — correct when a plan changed, but it also rebuilt
  the snapshot on every geometry-only sample;
- `apply_mute_states` — which started a volume fade and sent `set_mute` even
  when the target `(muted, volume)` had not changed.

A short cover (a window crossing the monitor, a transient fullscreen prompt)
therefore started a pause/resume fade. Ten one-second covers produced 20
`[Eco]` transitions.

### Change

- `hidden_since` and `last_mute` maps were added to `App`.
- A hidden monitor is paused only after `OCCLUSION_CONFIRM = 2 s`. The
  one-second supervision tick applies the pending decision; unhide is
  immediate. A real fullscreen game pauses within 2–3 s while the wallpaper is
  already invisible.
- `last_mute` stores the desired `(muted, primary volume)` per `WindowKey`; a
  geometry sample that does not change it sends no fade and no mute IPC.
- `apply_playback_states` publishes the snapshot only when a plan changed.
  Callers that change a setting without a plan change (`SetPauseWhenMaximized`,
  `SetMuteWhenMaximized`) publish explicitly.
- `playback_facts` reports the applied plan, so `is_playing` remains true during
  the two-second confirmation window.

### Before/after

Ten one-second covers, with a real window that covers the monitor:

| Metric | Before | After |
| --- | ---: | ---: |
| `[Eco]` transitions / 10 cycles | 20 | **0** |
| server CPU % one core during churn | 0.68 (22.9 s) | 0.54 (20.1 s) |

The server CPU numbers are short-run and not the primary claim; the
transition count is. A one-second cover no longer starts a fade. A cover that
stays longer than two seconds still pauses; the occluded/parked smoke runs
confirm this.

## 4. Metrics sampler

### Windows

`Monitor::sample_of` costs 3.843 ms mean / 4.561 ms p95 on this host. The
breakdown:

| Component | mean |
| --- | ---: |
| `read_tree` (Toolhelp + per-process counters) | 3.271 ms |
| Windows GPU PDH sampler | 0.331 ms |
| total physical memory | 0.001 ms |

It is called once a second only while the control surface is visible on Home or
Performance. The process-table walk dominates; the GPU sampler is cheap. The
hidden-tray path no longer calls it. Replacing Toolhelp with an undocumented
`NtQuerySystemInformation` layout would add risk for a visible-page cost below
half a percent of one core, so **no runtime change** was made.

### Linux — deferred

The Linux branch of `read_tree` reads `/proc/<pid>/stat` and
`/proc/<pid>/status` for every process, then keeps only the descendants. The
proposed, **not implemented**, safe sequence is:

1. Read only `/proc/<pid>/stat` for all numeric entries. It already contains
   PPID, utime+stime, `num_threads` and RSS in pages, so `status` is not needed
   for the tree at all. This halves file opens and removes the full status
   parse.
2. Measure again. If the remaining cost is still material, walk only
   `/proc/<pid>/task/<tid>/children` from the root, with the full stat scan as
   a fallback when that interface is unavailable or the tree is inconsistent.
3. Test stat field indices, page size, reparenting and a missing root; run the
   ignored `metrics_sample_cost` test and `strace -c` before/after on a Linux
   host.

This is deliberately not committed as an "optimisation": the only available
host is Windows, and a before/after without the Linux run would violate the
phase rule. The ignored microbenchmark is in `ferramari-core/src/metrics.rs`
so the Linux run is one command when a host is available.

## 5. Window-state sampler — no change

The hypothesis was that `sample()` rebuilding the full monitor list every
750 ms was expensive. The measured cost is 0.146 ms mean, 0.166 ms p95, about
0.02% of one core at 1.33 Hz. The `monitors::list()` call was left alone; the
cost is below noise. The ignored test remains for future hardware.

## 6. Decoder parking — no change

Five minutes hidden, 1080p30, same mpv PID:

| Phase | mpv RSS MiB | mpv threads | mpv VRAM MiB | total RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| Occluded, loaded | 133.9 | 42 | 25.05 | 155.1 |
| Parked | **63.1** | **26** | **0** | **84.5** |
| Resumed | 135.9 | 41 | 25.05 | 157.3 |

The parked position was 8.533 s; the resumed playback continued from there.
The five-minute threshold was not changed because the measurement supports the
current behaviour and a shorter threshold would trade a real reload for a
benefit that is already captured after five minutes.

## 7. Media process model

One mpv per monitor is the current model. A 1080p player uses about 135 MiB and
40 threads; a 4K30 player uses about 201 MiB and 49 threads. The one-display
measurement cannot show whether merging processes would help on two or three
displays, and the brief explicitly forbids a merge without proof. No merge was
attempted.

`ProbeStats` remains primary-only. The eleven sequential properties cost less
than the reader/supervision work while paused; adding an eleven-property probe
per player would multiply traffic for a value the UI does not display. The
benchmark records the primary FPS/dropped and marks other players as not
measured.

## 8. Conhost

A `conhost.exe` child (about 8.6 MiB, two threads) is present when the server is
launched from a console. The process inventory shows its parent is the server,
and adding `CREATE_NO_WINDOW` to the mpv spawn did **not** remove it because the
console belongs to the server, not to each player. That change was reverted;
the single console host is documented rather than treated as a per-monitor
leak.

## 9. Long-running scenario

`scripts/benchmark.py --scenario long-run --duration 86400 --event-interval 3600`
samples the process tree, kills one player, covers/un-covers and reloads on the
event cadence. It writes JSONL and a summary like the other scenarios. The
24-hour run was **not executed** on this working desktop; it needs a dedicated
machine or explicit consent. The planned assertions are:

- RSS/PSS slope and thread/process counts over 24 h;
- no growth after warm-up beyond a calibrated threshold;
- every scheduled mpv kill recovers to a new PID and a first frame;
- IPC errors (`[mpv] IPC connection failed`, `[mpv] command failed`) and player
  restarts are counted;
- Eco transitions and any stuck state are visible in the per-sample
  `monitor_states`;
- no orphan mpv/conhost after quit.

## 10. Quality gates and rollback

Each change is a separate commit with the measured before/after above. Rollback
is a revert of that commit; no config or data migration is involved. The final
gate is:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

The Windows live smoke covers steady video, pause/resume, occluded/resume,
parked/resume, churn, UI visible/hidden, video switch and mpv recovery. Linux
CI compiles the workspace and examples; Linux runtime numbers remain a
documented gap.
