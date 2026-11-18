# Ferramari 2.2.0 — Phase 0 audit

**Audit date:** 2026-11-18  
**Target:** `origin/main` / `fb6071f` (release 2.2.0)  
**Branch:** `work/2.2-audit`  
**Scope:** Phase 0 audit plus the first Phase 1 change set. Each finding states
whether it is fixed, partially fixed, or still open; unverified live scenarios
stay explicitly blocked.

## Environment

| Item | Value |
| --- | --- |
| OS | Windows 11, build 26200 |
| Session | Explorer desktop, one display |
| Display | 1920×1080 physical at 125% scaling (1536×864 logical) |
| GPU | AMD Radeon Graphics + NVIDIA GeForce RTX 3050 Laptop |
| Rust | 1.99.1, cargo 1.99.1 |
| mpv | 0.42.0 (`C:\Program Files\MPV Player\mpv.exe`) |
| ffmpeg / ffprobe | 9.0.2 |
| yt-dlp | 2026.10.26 |
| Inno Setup | 6 |
| gettext / msgfmt | 1.0 |
| Linux / WSL | No Linux distribution installed; Linux live checks were not run |

`ferramari --doctor` on this machine reports:

```text
session: Windows desktop
desktop: Explorer
backend: win32 (WorkerW wallpaper host behind Explorer's icons)
Explorer wallpaper host: yes
desktop window: yes
per-monitor occlusion: yes
static wallpaper: yes
capabilities: wallpaper=supported, displays=supported, hotplug=supported,
              window tracking=supported, lock=supported, power=supported,
              idle=supported
monitors: 1
display identities: DISPLAY1 [device MONITOR\LEN9059\...] (key remembered)
mpv: mpv v0.42.0-3-g902818753
ffmpeg: ffmpeg version 9.0.2
ffprobe: ffprobe version 9.0.2
yt-dlp: 2026.10.26
hardware decode: requested auto -> --hwdec=auto-safe; not decoding
```

## Baseline

All mandatory gates passed on 2.2.0 before the audit:

```text
cargo fmt --all -- --check                                  exit 0
cargo clippy --workspace --all-targets --locked -- -D warnings  exit 0
cargo test --workspace --locked                             exit 0
cargo build --workspace --release --locked                  exit 0
```

Test totals: **450 passed, 0 failed**:

| Target | Passed |
| --- | ---: |
| `ferramari` binary | 15 |
| `ferramari-core` library | 350 |
| `ferramari-media` library | 17 |
| `ferramari-platform` library | 54 |
| `ferramari-ui` library | 8 |
| `ferramari-ui` binary | 6 |
| Doc tests | 0 |

Both release executables exist:

```text
target/release/ferramari.exe       6,585,856 bytes
target/release/ferramari-ui.exe   21,305,344 bytes
```

The GTK-family check was run against a Linux target dependency graph:

```text
cargo tree --workspace --locked --all-features -e normal \
  --target x86_64-unknown-linux-gnu | grep -E '(^| )(gtk|gdk|webkit2gtk|gdk-pixbuf|glib|gio|pango|cairo-rs) v'
```

No match was found. `msgfmt --check` passed for `po/ru.po` and `po/ja.po`.

AppStream and desktop-file validation were not run locally because
`appstreamcli` and `desktop-file-validate` are not installed on this Windows
host; the Linux CI job runs both.

## Severity and status

- **P0** — playback is unusable, user data can be lost, or a local process can
  take the server down.
- **P1** — a core feature is wrong, a failure is invisible to the user, or a
  supported platform scenario is broken with a workaround.
- **P2** — reliability, diagnostics, UX or resource use; not an immediate
  data-loss path.
- **P3** — documentation, CI or release-process drift.

Statuses used below:

- `confirmed-live` — reproduced on this machine with the release binary.
- `confirmed-code` — the code path is unambiguous; a live/unit reproduction is
  still to be added.
- `confirmed-repro` — reproduced with a deterministic file/command scenario.
- `documented-gap` — already stated in the repository's own contract.
- `risk` — plausible, but not yet proven.

## Summary

| ID | Priority | Status | Area | Finding |
| --- | --- | --- | --- | --- |
| R-01 | P0 | fixed-live | Windows static wallpaper | Disabling static wallpaper makes playback restart in a loop |
| R-02 | P1 | fixed-unit | Config durability | Recovery overwrites the valid `.bak` with the corrupt primary |
| R-03 | P1 | fixed-unit | Display identity | A corrupt `display-ids.json` ignores a valid `.bak` and loses assignments |
| R-04 | P1 | fixed-code | Config reload | `reload_config` discards a pending debounced setting |
| R-05 | P1 | partially-fixed | Error reporting | Errors now publish to `operation_status`; per-command result semantics remain open |
| R-06 | P1 | partially-fixed | Playback state | `is_playing` now requires a live player; cold-start decode is still not a separate fact |
| R-07 | P1 | fixed-unit | Mixed DPI | Windows physical geometry is multiplied by `scale_factor` a second time |
| R-08 | P1 | fixed-unit | Windows control transport | Request line and concurrent client threads are bounded |
| R-09 | P2 | partially-fixed | UI status | Status expires, new server operations have a sequence, optimistic play has a timeout |
| R-10 | P2 | fixed-unit | Config validation | Loaded volume/blur/fade values are range-checked and persisted |
| R-11 | P2 | fixed-unit | Config robustness | A UTF-8 BOM makes the whole config fall back to defaults |
| R-12 | P2 | fixed-code | Cache management | `catalogue::cleanup_cache()` has no caller |
| R-13 | P2 | fixed-unit | Single instance | Second launch retries endpoint/interface publication |
| R-14 | P2 | documented-gap | Shutdown | Linux D-Bus watchers and the Windows accept loop live until process exit |
| R-15 | P3 | partially-fixed | Docs / CI | Linux formatting gate fixed; stale docs/release gating remain |

---

## R-01 — Disabling static wallpaper makes Windows playback restart in a loop

**Priority:** P0  
**Status:** `fixed-live` (fixed in this branch; see Resolution)

**Where**

- `src/app.rs:1731-1742` — `update_static_wallpaper()` calls
  `self.background_wallpaper.restore()` whenever the static wallpaper is not
  wanted.
- `crates/ferramari-platform/src/windows/wallpaper.rs:336-383` —
  `StaticBackground::restore()` unconditionally calls `set_wallpaper(&path)`
  for `Background::Original`, even when the desktop already displays that
  exact file.
- `crates/ferramari-platform/src/windows/wallpaper.rs:72-82` —
  `set_wallpaper()` calls `SPI_SETDESKWALLPAPER` with
  `SPIF_UPDATEINIFILE | SPIF_SENDCHANGE`.

**Observed**

An isolated profile with `is_static_wallpaper=false` was started in background
mode, then a local 1080p video was applied through the control endpoint. The
server log showed:

```text
22:17:34.154 [Surface] DISPLAY1 created 0x1660330 rect=1920x1080+0,0
22:17:34.231 [StaticWallpaper] Restored c:\windows\web\wallpaper\themeb\img25.jpg
22:17:34.954 [mpv] IPC connection failed: ...
22:17:35.699 [Player] DISPLAY1 stopped; retrying in 1s
22:17:36.740 [mpv] IPC connected: \\.\pipe\ferramari-mpv-7112-1
22:17:38.220 [mpv] IPC connection failed: ...
22:17:38.718 [Player] DISPLAY1 stopped; retrying in 2s
22:17:40.757 [mpv] IPC connected: \\.\pipe\ferramari-mpv-7112-2
22:17:41.280 [mpv] IPC connection failed: ...
```

The snapshot at that point reported `is_playing=true` while
`player_stats.active=false`. In a counter-run with `is_static_wallpaper=true`,
the static frame caused one expected host rebuild and then playback stabilized:

```text
22:18:53.033 [StaticWallpaper] Applied C:\Users\...\static\e921765e0d8b0d63.png
22:18:53.034 [mpv] IPC connection lost unexpectedly
22:18:54.051 [Player] restarted DISPLAY1 (Video)
... snapshot after 8 s: is_playing=true, player_stats.active=true
```

**User impact**

Users who turn the static wallpaper off get a wallpaper that keeps restarting
after every video switch/hotplug rebuild, and can eventually hit the restart
give-up limit. The desktop may show a static image while the UI claims the
video is playing. This is a normal supported configuration, not an edge case.

**Fix direction**

Make the restore operation idempotent, or call it only when static wallpaper
transitions from enabled to disabled and at shutdown. At minimum,
`restore()` must skip `SPI_SETDESKWALLPAPER` when the current image already
equals the recorded original and no frame is active. Do not use an
`SPI_SETDESKWALLPAPER` call as a cheap no-op: on Windows it replaces Explorer's
wallpaper host and destroys the live surface.

**Regression test**

- A unit test around `StaticBackground::restore()` with a fake current/original
  path that asserts no `set_wallpaper` decision when the two are equal.
- A Windows live smoke item: static wallpaper off, apply video, switch video
  twice, wait 10 s, assert the mpv PID stays stable and no `lost its wallpaper
  host`/restart sequence occurs.

**Resolution (2026-11-18)**

`StaticBackground::restore()` now skips the restore when no frame is active and
the desktop already shows the recorded original. The decision lives in
`restore_can_be_skipped()` and is covered by
`restoring_the_original_that_is_already_shown_is_a_no_op`.

A fresh isolated live run with `is_static_wallpaper=false` then produced:

```text
PLAYING=True
STATS_ACTIVE=true
MPV=18344
RESTART_LINES=0
RESTORE_LINES=0
```

No `lost its wallpaper host`, no restart, and an active decoder after eight
seconds. The earlier failing run had three restarts in seven seconds and
`stats.active=false`.

---

## R-02 — Config recovery overwrites the valid backup with the corrupt primary

**Priority:** P1 (can become P0 on a crash in the copy window)  
**Status:** `confirmed-repro`

**Where**

- `crates/ferramari-core/src/config.rs:44-65` — `write_atomic()` writes the
  temporary file, then `fs::copy(path, backup_path(path))`, then renames the
  temporary file over the primary.
- `crates/ferramari-core/src/config.rs:238-269` — after recovering from the
  backup, `load_from()` immediately saves the recovered config through the same
  helper.

**Observed**

A valid primary was replaced with a truncated JSON document and a valid
`.bak` containing `audio_volume=42` was kept. After startup:

```text
SNAPSHOT_VOLUME=42
PRIMARY_VALID=True
PRIMARY_VOLUME=42
BACKUP_RAW={"version": 4, "audio_volume"
```

The primary recovered correctly, but the backup now contains a copy of the
corrupt primary. The same helper is used by `library.json` and
`display-ids.json`.

**User impact**

The backup is no longer a last-known-good copy after a recovery. If the
process or machine fails between the `fs::copy` and the rename, or if the
rename fails (antivirus lock, permissions), both the primary and the backup
can be corrupt. This directly violates the crash-safe configuration claim.

**Fix direction**

Do not copy the current primary over the backup without validating it. Write
the backup through its own temporary file and rename, and only promote a
primary that parsed successfully. A safe sequence is: write new temp/fsync;
promote the known-good old primary to backup atomically; rename new temp to
primary. If the old primary is known to be corrupt, leave the previous backup
alone.

**Regression test**

- Extend `atomic_save_keeps_a_backup_and_recovers_from_it` to assert that the
  backup is still valid JSON after recovery.
- Add a failure-injection seam between backup promotion and primary rename;
  after the simulated failure, assert that at least one valid copy exists.

**Resolution (2026-11-18)**

`write_atomic()` now promotes only a previous primary that is a valid JSON
object, writes the backup through its own temporary file and rename, and leaves
an already-good backup alone when the primary is corrupt. The existing recovery
test now also asserts that the repaired backup remains valid, and
`a_corrupt_primary_is_not_promoted_to_the_backup` covers the
corrupt-primary-to-new-primary path. The same helper serves `config.json`,
`library.json` and `display-ids.json`.

---

## R-03 — A corrupt display-id registry ignores its valid backup

**Priority:** P1  
**Status:** `confirmed-repro`

**Where**

- `crates/ferramari-core/src/config.rs:35-39` — `read_with_backup()` returns
  the primary whenever it is readable, without validating it.
- `crates/ferramari-core/src/display_ids.rs:54-65` — `load_from()` parses that
  text once and returns `Self::default()` on error. It never tries
  `display-ids.json.bak`.

**Observed**

The backup contained:

```json
{"records":[{"stable_id":"deadbeef","key":"LegacyDisplay"}]}
```

The primary was `{ not json`. After startup the primary contained only the
current display:

```json
{
  "records": [
    {
      "stable_id": "id-35ef37bec59977081ba1b8cda95b470a",
      "key": "DISPLAY1"
    }
  ]
}
```

and the backup had been replaced by `{ not json`.

**User impact**

Stable display identities are lost after a crash or a truncated write. A
wallpaper assignment can then follow the connector/enumeration fallback
instead of the physical screen. The user sees a wallpaper move to the wrong
monitor after a replug or layout change.

**Fix direction**

Use a corruption-aware `read_with_backup()` (validate with `serde_json`) or
parse the primary, then parse the backup on failure. Rewrite both files only
after a successful parse. Keep the `write_atomic` fix from R-02.

**Regression test**

- Write a valid registry to `.bak`, truncate the primary, load, and assert the
  legacy record survives.
- Assert the backup remains valid after the repair.

**Resolution (2026-11-18)**

`DisplayIdRegistry::load_from()` now tries the primary and then
`display-ids.json.bak`, remembering the last parse error for the log. The new
`a_corrupt_primary_falls_back_to_the_valid_backup` test writes a valid backup,
corrupts the primary, and asserts both the recovered record and the intact
backup. `read_with_backup()` also validates JSON before preferring a primary,
so `Config::language_override()` benefits from the same rule.

---

## R-04 — `reload_config` discards a pending debounced setting

**Priority:** P1  
**Status:** `confirmed-live`

**Where**

- `src/app.rs:1164-1184` — `reload_config()` assigns
  `self.config = Config::load(&names)` without first calling
  `self.flush_config()`.
- `src/app.rs:1817-1834` — volume and blur changes only mark
  `config_dirty_at`; the write is delayed by `CONFIG_FLUSH_AFTER`.
- `src/app.rs:786-800` — the hotplug path already flushes before reloading;
  this is the correct precedent.

**Observed**

The config file contained `audio_volume=42`. Through the control endpoint the
audit sent `property_set volume=77` followed immediately by
`reload_config`. Result:

```text
BEFORE=42
AFTER_RELOAD=42
SAVED_VOLUME=42
```

The volume edit was accepted (`SetVolume` runs and updates the players/snapshot
before the reload) but was never written. `reload_config` then re-read the old
file and replaced the in-memory value.

**User impact**

A D-Bus/control client that calls `reload_config` during a slider drag loses
the newest value. This is exactly the concurrent hotplug/reload loss the
specification calls out.

**Fix direction**

Call `flush_config()` before `Config::load()` in `reload_config()` (same as
the hotplug path). Add a test that sets a dirty value and reloads.

**Regression test**

- Unit/integration test with a temporary config: `SetVolume(77)`,
  `ReloadConfig`, assert volume 77 in memory and on disk.
- A live control-endpoint check on Windows, where the ordering is easy to
  reproduce.

**Resolution (2026-11-18)**

`reload_config()` now calls `flush_config()` before `Config::load()`, matching
the established hotplug path. The pre-fix live reproduction is recorded above
(`AFTER_RELOAD=42`). A post-fix live rerun was not possible while another
Ferramari server owned the per-session mutex; the unit/integration regression
test remains to be added with the next app-level test harness.

---

## R-05 — Server-side application errors never reach the control surface

**Priority:** P1  
**Status:** `partially-fixed`

**Where**

- `src/app.rs:1929-1931` — `notify_user_error()` only calls `error.log()`.
- `src/app.rs:1837-1845` — config save failures take this path.
- `src/app.rs:806-825` — a missing video file takes this path.
- `src/app.rs:1127-1150` — playback and stream failures take this path.
- `src/app.rs:1848-1851` and `1853-1922` — `operation_status`/`operation_error`
  exist in the snapshot, but only package operations set them.
- `crates/ferramari-platform/src/windows/dbus.rs:181-194` — a command is
  queued and the reply is `ok` immediately; the D-Bus methods return `()`.
- `crates/ferramari-ui/src/main.rs:1233-1240` — `CommandFinished` only receives
  transport errors.
- `crates/ferramari-ui/src/main.rs:1174-1184` — the UI only reads
  `operation_status`, which is empty for the errors above.

**Observed**

With an isolated profile, the audit sent a `video` command for a path that
does not exist:

```text
COMMAND_REPLY={"ok":true,"result":null,"error":null}
OP_STATUS=
OP_ERROR=False
LOG: [filesystem] File not found: C:\Users\...\does-not-exist.mp4
```

The command reported success; the only evidence was in the log. The same gap
applies to playback failure, mpv exhaustion, config save failure and the
static-wallpaper restart loop in R-01.

**User impact**

The user sees an apparently successful action and no explanation when nothing
plays. The UI cannot distinguish "queued", "applied" and "rejected". This
violates the explicit requirement that apply errors be visible and that the UI
not present optimistic success when the server rejected the command.

**Fix direction**

Add an additive result path: a command id and a `CommandResult`/`last_error`
property (or an error event channel) carrying at least the command id, the
user-facing message, the kind and the timestamp. The existing D-Bus/JSON
method signatures can stay compatible. The UI should wait for the matching
result or a snapshot transition before clearing a pending action. Do not
reuse the package-only `operation_status` string for unrelated commands
without a sequence id.

**Regression test**

- UI state test: queue a command, deliver a server rejection, assert the
  status is visible and the optimistic value rolls back.
- Windows control test: missing file returns a structured error (or an
  observable error snapshot) rather than plain `ok`.
- Server test: `notify_user_error` publishes to the snapshot.

**Resolution (2026-11-18, partial)**

`notify_user_error()` now publishes `error.message()` through the existing
`operation_status`/`operation_error` snapshot fields and refreshes the snapshot,
so the control surface can display an apply/playback failure instead of only
writing a log line. `operation_seq` lets the UI distinguish a repeated message
from a new event. The command API is still fire-and-forget and the status has
no request id; per-command acknowledgement and rollback remain open (R-09).

---

## R-06 — `is_playing` reports policy intent, not a live player

**Priority:** P1  
**Status:** `partially-fixed`

**Where**

- `src/app.rs:1576-1580` — `any_playing()` iterates `windows` and asks
  `eco_plan_for(monitor).play`; it does not ask whether the player or surface
  is alive.
- `src/app.rs:1853-1861` — `Snapshot.is_playing` is assigned from
  `any_playing()`.
- `src/app.rs:1203-1375` — `watch_players()` removes dead engines and rebuilds
  them; a restart may not refresh the snapshot immediately.

**Observed**

In the R-01 static-off run the snapshot reported `is_playing=true` while every
mpv was exiting and `player_stats.active=false`. In the static-on recovery
run, after a kill the supervisor created a new mpv (PID 20376), but the next
snapshot still reported `is_playing=false` for a short period. The value is
therefore unreliable in both directions.

**User impact**

The Home page, tray, D-Bus clients and scripts can show "playing" when no
decoder exists, or "not playing" just after a successful restart. A user
cannot use this value to diagnose a broken wallpaper.

**Fix direction**

Keep two facts: requested/intended play state and actual player state. Derive
`is_playing` from a live engine with a live surface and a live mpv (or define
the property explicitly as intent and add an `is_decoding`/`player_alive`
fact). Refresh the snapshot after any automatic restart or give-up.

**Regression test**

- Snapshot test with a dead engine: `is_playing` must not be true.
- Snapshot test immediately after restart: `is_playing` should become true
  once the new engine is installed.
- Live smoke: kill mpv, assert state transitions and no false "playing" after
  give-up.

**Resolution (2026-11-18, partial)**

`Engine::player_ready()` now requires a live surface and a live IPC worker, and
`any_playing()` requires that before the Eco policy can report playback.
`watch_players()` republishes the snapshot after a player set change. The new
`a_dead_ipc_or_stopped_worker_is_not_alive` test covers the non-blocking worker
flags. `is_playing` still does not distinguish "player ready but no frame has
decoded yet"; that remains a separate `is_decoding` fact if the UI needs it.

---

## R-07 — Windows physical geometry is scaled twice for mixed DPI

**Priority:** P1  
**Status:** `fixed-unit`, live matrix blocked by one display

**Where**

- `crates/ferramari-platform/src/windows/monitors.rs:166-183` — Windows
  reports geometry in physical pixels and stores
  `scale_factor = round(dpi / 96)`.
- `src/app.rs:2051-2059` — `monitor_rect()` multiplies x, y, width and height
  by `scale_factor`.
- `crates/ferramari-core/src/window.rs:38-55` — `visibility_at()` compares
  the resulting rectangle with `WindowState` rectangles.
- `crates/ferramari-platform/src/windows/window_state.rs:232-270` — the
  Windows watcher reports `GetWindowRect`, i.e. physical pixels.

**Reasoning**

At 150% DPI, `GetDpiForMonitor` returns 144; `round(144/96) = 2`. A
1920×1080 physical monitor becomes a 3840×2160 rectangle for occlusion
checks, while a fullscreen window is still 1920×1080. `rect_covers()` and
`center_inside()` then fail, so the wallpaper is not paused. At 125%,
`round(1.25) = 1`, which is why the current test machine masks the bug. At
200% the same double-scaling happens.

**User impact**

On a 150%/175%/200% display, or a mixed-DPI multi-monitor layout, Eco
occlusion/fullscreen pause and parking can stop working. This is one of the
explicitly unchecked Windows scenarios in `docs/TESTING.md`.

**Fix direction**

Make the coordinate space part of the `MonitorInfo`/`DisplayProvider`
contract: Windows providers must either return logical geometry and let
`monitor_rect()` scale, or keep physical geometry and have the policy use it
without scaling. Do not infer this from `scale_factor` alone. Add a unit test
for a Windows-style 1920×1080 physical display with `scale_factor=2`; the
occlusion rectangle must remain 1920×1080. Verify with a real 150%/200%
display before claiming mixed-DPI support.

**Regression test**

- Pure policy test: physical monitor + physical fullscreen window at scale 2
  must produce `covered=true` and `fullscreen=true`.
- Windows live matrix: 100/125/150/200%, two monitors, primary on either side,
  negative coordinates and a fullscreen app.

**Resolution (2026-11-18, unit only)**

`MonitorInfo` now declares `physical_geometry`. The Windows display provider
sets it to `true`; the X11 provider leaves it `false`. `monitor_rect()` returns
the raw rectangle for physical geometry and only applies `scale_factor` to
logical geometry. The new
`physical_monitor_geometry_is_not_scaled_twice` test uses a 1920×1080 physical
display at scale 2 and asserts the occlusion rectangle stays 1920×1080. The
150/200% live check is still required before calling mixed-DPI supported.

---

## R-08 — Windows control endpoint accepts an unbounded line and spawns unbounded client threads

**Priority:** P1  
**Status:** `fixed-unit`

**Where**

- `crates/ferramari-platform/src/windows/dbus.rs:118-143` — the accept loop
  spawns one thread per accepted connection.
- `crates/ferramari-platform/src/windows/dbus.rs:145-171` — `BufReader::read_line`
  reads until a newline with no size cap; the token is checked only after the
  complete line has been read.
- `crates/ferramari-platform/src/windows/control.rs:35-39` — connect and I/O
  timeouts exist, but no request-size limit.

**User impact**

A local process running as the same user can connect many times, or send a
very large line, and force thread/memory growth before authentication. The
token does not help until the line has been allocated. The TZ explicitly
requires an incoming-message size limit and bounded resources.

**Fix direction**

Cap the request line (for example 64 KiB) using `Read::take`, return a
structured error before parsing, and cap concurrent client handlers with a
semaphore or a small fixed worker set. Keep the per-connection timeout. Add
tests with an oversized line, a slow client and a burst of connections.

**Regression test**

- Unit/integration test against a mock listener: a line over the limit gets a
  bounded error and the server stays healthy.
- Test that connection N+1 is rejected/queued once the concurrency cap is
  reached.
- Verify the endpoint ACL remains user-only on a real profile (the audit's
  isolated path inherited SYSTEM/Administrators/user only; no explicit ACL is
  set by the app).

**Resolution (2026-11-18)**

`read_request_line()` reads through a `Read::take` limit of 64 KiB plus one byte
and rejects an oversized line before JSON parsing. The accept loop also admits
at most `MAX_CLIENTS` (16) concurrent handlers; a rejected connection is
dropped without spawning a thread, and a `ClientSlot` releases the slot on every
normal or unwinding exit. The tests cover a normal line, EOF, an oversized line,
slot exhaustion and slot reuse.

---

## R-09 — Sticky UI status and a one-snapshot optimistic play state

**Priority:** P2  
**Status:** `partially-fixed`, UI interaction not exercised in this pass

**Where**

- `crates/ferramari-ui/src/main.rs:1174-1184` — a non-empty
  `operation_status` sets the banner, but an empty value never clears it.
- `crates/ferramari-ui/src/main.rs:1233-1240` — a failed command sets the
  banner; the next successful snapshot does not clear it.
- `crates/ferramari-ui/src/main.rs:1217-1228` — `pending_play` is consumed by
  the first snapshot, even if that snapshot contradicts the action; the UI
  keeps the optimistic value only for that tick.
- `src/app.rs:1848-1851` — package operation status is never cleared after a
  successful operation.

**User impact**

A stale error or "Package request queued" line remains on screen after the
condition ends. A play/pause click can show the requested state while the
server still reports the old one; a rejection is not rolled back as an
explicit failed action. The comment in the UI says the line disappears on the
next successful poll, but the implementation does not do that.

**Fix direction**

Give status entries a sequence/TTL and clear them on the next successful
matching result or snapshot. Keep a pending action until the server confirms
it, the command returns an error, or a short timeout expires; then apply the
server value. Clear the package status when a later operation starts or after
a bounded display time.

**Regression test**

- UI state test: error status, then a successful snapshot; assert the banner
  clears.
- UI state test: pending play, contradictory snapshot; assert the final state
  follows the server after the timeout and an error remains visible.
- Server test: package operation status lifecycle.

**Resolution (2026-11-18, partial)**

The server emits an additive `operation_seq` alongside `operation_status` and
`operation_error`. The UI now treats a repeated status string as the same
event, expires a status line after eight one-second ticks, and keeps a pending
play/pause intent for at most three polls before accepting the server value.
The old error is still not tied to a specific command, so the full ack/rollback
design remains open.

---

## R-10 — Loaded numeric settings are not range-checked

**Priority:** P2  
**Status:** `fixed-unit`

**Where**

- `crates/ferramari-core/src/config.rs:305-332` clamps
  `rotation_interval_sec` and `idle_timeout_sec`, but not `audio_volume`,
  `static_wallpaper_blur_radius`, `fade_duration_sec` or `fade_interval`.
- `src/app.rs:1602-1620` and `1643-1652` clamp volume/blur only when the
  setter runs; a value loaded from disk is passed to the engine and snapshot
  unchanged.

**User impact**

A hand-edited config or a value from an older build can produce a volume
outside 0–100, a huge blur radius, or a fade interval that creates a very
large number of fade steps. The D-Bus snapshot can report a value the UI's
slider cannot represent. This is a robustness problem, not a security
boundary.

**Fix direction**

Validate every numeric field in `Config::parse`, set `changed` when a value
is normalized, and use the same ranges as the setters. Add boundary tests for
0, maximum, negative, fractional and very large values. Keep `fade_steps`
bounded independently.

**Regression test**

- Table-driven parse tests for the four fields.
- A property test over out-of-range integers, asserting the in-memory and
  saved values are in range.

**Resolution (2026-11-18)**

`Config::parse()` now clamps volume to 0–100, blur to 0–20, fade duration to
0–60 seconds and fade interval to 0.001–10 seconds, marking the document for
rewrite when a value changes. `out_of_range_numbers_are_clamped_and_persisted`
covers the negative and oversized cases.

---

## R-11 — A UTF-8 BOM resets the whole config

**Priority:** P2 (manual-edit robustness)  
**Status:** `fixed-unit`

**Where**

- `crates/ferramari-core/src/config.rs:238-262` — `is_json_object()` uses
  `serde_json::from_str`, which rejects a leading UTF-8 BOM; the primary is
  then treated as broken and, without a backup, the template is used.

**Observed**

A valid config written by Windows PowerShell with a UTF-8 BOM (the default of
`Set-Content`/`Out-File` in the test environment) was loaded as the template:
volume reset from 42 to 50, `is_static_wallpaper` reset to true and all other
settings returned to defaults.

**User impact**

A user editing `config.json` with an editor that emits a BOM loses all
settings on the next start. The documentation describes loading as forgiving;
a BOM is a file-level encoding marker, not a corrupted value.

**Fix direction**

Strip a leading `\u{feff}` before parsing in both `Config::load_from` and
`Config::language_override`, or read through a helper that normalizes the
encoding. Add a test with a BOM-prefixed valid document.

**Regression test**

- `Config::load_from` with a BOM and with a valid backup; assert the values
  survive and the file is rewritten without a BOM if the app chooses to
  rewrite.

**Resolution (2026-11-18)**

`strip_bom()` is applied by `Config::parse`, `Config::language_override`,
`Config::is_json_object`, `DisplayIdRegistry`, `Library` and the update record.
The new `a_utf8_bom_is_not_a_corrupt_config` test parses a BOM-prefixed
document and preserves its value.

---

## R-12 — `catalogue::cleanup_cache()` has no caller

**Priority:** P2  
**Status:** `fixed-code` (startup call added; fake-root test open)

**Where**

- `crates/ferramari-core/src/catalogue.rs:1055-1141` defines
  `cleanup_cache()`, including the seven-day age and 2 GiB video budget.
- `src/app.rs:146-151` only cleans `.part` files in the thumbnail and static
  caches on startup.
- A repository-wide grep finds no call to `catalogue::cleanup_cache()`.

**User impact**

The README says cached videos older than seven days or beyond 2 GB are
removed on start. They are not. The catalogue cache, previews and stale
partial downloads can grow without bound; the 2 GiB budget is not enforced.

**Fix direction**

Call `catalogue::cleanup_cache()` from the startup cache-clean task (or a
bounded worker) before or alongside the UI starts, and make it testable with a
fake cache root. Include the mpv socket directory in stale-file cleanup for
Linux if it is safe to identify files from dead PIDs.

**Regression test**

- A test with a fake cache containing an old video, a fresh video and a
  partial file; assert old data is removed and the current limit is applied.
- A start-up test/fake-clock assertion that the cleanup function is invoked.

**Resolution (2026-11-18, partial)**

The startup cache-clean thread now calls `catalogue::cleanup_cache()` alongside
the thumbnail/static partial cleanup. The function itself still reads the
global cache paths; a fake-root test remains open.

---

## R-13 — Second launch can silently lose the single-instance startup race

**Priority:** P2  
**Status:** `fixed-unit` (live startup race still to be exercised)

**Where**

- Windows: `crates/ferramari-platform/src/windows/dbus.rs:52-103` claims the
  mutex, publishes the endpoint, then `main` starts the accept loop. A second
  process sees `AlreadyRunning` and calls `request_show_gui()` immediately.
  If the endpoint is not written yet, or the accept loop is not registered
  (for example during `--pause`), the request fails. `src/main.rs:99-103`
  ignores the error and exits.
- Linux: `crates/ferramari-platform/src/dbus.rs:27-67` claims the bus name
  before `register()` exports the interfaces. A second process can see
  `NameHasOwner` and fail its `show_gui` call before the interface exists.

**User impact**

A second launch during app startup, especially after login with `-b -p N`,
can silently do nothing. The user has to launch again after the first instance
is ready.

**Fix direction**

Make the second-instance path retry `show_gui` for a bounded period, or
register the control surface before exposing the instance name where the
transport allows it. Log a visible failure instead of `let _ =`. Add a
startup-race test with a delayed endpoint/interface.

**Regression test**

- Windows integration test with a deliberately delayed accept loop.
- Linux D-Bus test with a name owner that registers the interface after a
  delay, if a test bus is available; otherwise a documented smoke item.

**Resolution (2026-11-18, unit only)**

`ferramari_core::util::retry_until()` retries a closure until a deadline; both
`request_show_gui()` implementations now use a three-second window with 100 ms
intervals. The second launch logs a failed request instead of discarding it.
The helper has transient-success and deadline tests; the actual two-process
startup race still needs the smoke item in `docs/TESTING.md`.

---

## R-14 — Shutdown contract for watchers remains weaker than documented

**Priority:** P2  
**Status:** `documented-gap`

**Where**

- `docs/BACKEND_CONTRACT.md` §5 already records that the Linux D-Bus
  session/lock/sleep/idle watchers block in `receive_signal` and are released
  only at process exit.
- `crates/ferramari-platform/src/windows/dbus.rs:9-11` states the accept loop
  also lives until process exit.
- `crates/ferramari-platform/src/handlers.rs:84-380` has no stop token or
  join for the Linux watchers.
- The X11/monitor watcher `Drop` implementations deliberately detach threads
  blocked in `wait_for_event` (`crates/ferramari-platform/src/monitors.rs:335-352`,
  `crates/ferramari-platform/src/x11.rs:20-29`).

**User impact**

There is no current in-process restart of these sources, so the user does not
see a leak today. The gap becomes a correctness problem for any future server
restart or backend reinitialization, and it means `PlatformWatchers` promises
less than its name.

**Fix direction**

Give each watcher an owned connection plus a wake/stop mechanism with a
bounded join, or explicitly keep process-lifetime semantics and amend the
contract. The audit recommends fixing the D-Bus watchers before any in-process
backend restart is introduced.

**Regression test**

- Start a watcher, drop the token, assert the thread joins within a deadline.
- Shutdown smoke: start the server, quit, assert no process/thread remains.

---

## R-15 — Documentation and CI drift

**Priority:** P3  
**Status:** `confirmed-code`

**Documentation**

- `README.md` lists web-page wallpapers twice in the feature list (once as
  unavailable, once as a current limitation).
- `ROADMAP.md` still opens with the 1.0/1.9 narrative while the project is
  2.2.0; the 2.2 section exists above older 1.x sections.
- `docs/TESTING.md` starts by saying the 2.0 Iced UI has not been smoke-tested,
  then records substantial 2.2 Windows results. The historical warning is
  useful, but the current status should be explicit.
- `data/io.github.ferramari.Ferramari.desktop` still says "Video wallpaper for
  Linux" even though AppStream and the README describe Linux and Windows.
- The README claim that catalogue cache cleanup runs on start is false (R-12).

**CI**

- The Linux quality job runs `cargo fmt --all` and then `git diff --check`.
  `git diff --check` checks whitespace errors, not whether formatting changed
  the file, so a formatting diff can pass. The Windows job correctly uses
  `cargo fmt --all -- --check`.
- The release workflow builds and packages tagged commits but does not run
  `fmt`, `clippy` or the test suite; it relies on the earlier push CI.
- CI has no live playback, tray, Explorer, multi-monitor or mixed-DPI check.
  The Windows `--doctor` step is useful but is not a playback proof.
- `scripts/glyph-check.py`, AppStream and desktop-file validation are not all
  in the local gate; they run only in the Linux CI job (metadata/tooling).

**User impact**

Release documentation can claim support or cleanup that the code does not
provide, and a formatting regression can merge despite the stated gate. This
is a process finding, not a runtime defect.

**Fix direction**

Reconcile the README/ROADMAP/TESTING/desktop metadata in a dedicated docs
change. Change the Linux formatting step to `cargo fmt --all -- --check`.
Decide whether release tags must repeat the quality gates; if not, state that
the tag commit must have a green CI run. Keep live results in TESTING, not in
the historical audit.

**Resolution (2026-11-18, partial)**

The Linux CI formatting step now runs `cargo fmt --all -- --check`, so a
formatting diff fails the job instead of being rewritten in the runner. The
documentation reconciliation and release-workflow gating remain open.

---

## Verified security/robustness points

These were checked and did **not** produce a finding:

- `--doctor` reports the Win32 backend, Explorer host, display identity and
  tool versions without starting a window.
- The control endpoint is bound to `127.0.0.1`; on the isolated test profile
  the endpoint file inherited only SYSTEM/Administrators/user access. The app
  does not set an explicit ACL, so this depends on the profile; document that
  as a platform assumption.
- `msgfmt --check` passes for both catalogs.
- The Linux dependency graph contains no GTK/GDK/WebKitGTK packages.
- `mpv` recovery after `SIGKILL` did create a new process and a new surface
  with `is_static_wallpaper=true`; the state snapshot lag is R-06.
- With static wallpaper on, applying the frame caused the expected single
  host-loss rebuild followed by stable playback and active decoder stats.
- No orphan `mpv` remained after a clean `quit` in the audit runs.

## Unchecked / blocked scenarios

These are not failures; they were not run on this hardware and must stay
unchecked in `docs/TESTING.md` until a real session is available:

- Linux GNOME X11 and GNOME Wayland/XWayland playback, tray, lock/unlock,
  suspend/resume, autostart and static wallpaper.
- Two or more monitors on Linux or Windows; clone layouts, negative
  coordinates, display rotation and reconnect.
- Mixed DPI at 100/125/150/200%, and changing scale while running.
- Windows Explorer restart recovery, tray re-registration and close-to-tray
  in this audit pass (the repository has older recorded passes, but they were
  not repeated here).
- Windows installer upgrade/uninstall on a clean machine; only the existing
  2.2.0 artifact path was inspected, not rebuilt or installed.
- Flatpak sandbox, portal file dialogs, host media tools and tray/autostart
  inside the sandbox.
- Real screen lock, sleep/resume, sign-out and multi-user session changes.
- Long-running idle, decoder parking at the shipped five-minute threshold and
  energy/power measurements.

## Recommended Phase 1 order

1. **R-01** first: it is a P0 playback failure on a normal Windows setting and
   it feeds the false `is_playing` state.
2. **R-02/R-03/R-04** as the data-durability set: one atomic-write/backup
   helper, one corruption-aware registry loader, one flush-before-reload.
3. **R-05/R-06** as the state-honesty set: command result/error path and
   actual-versus-intended playback state.
4. **R-08** transport hardening (size/concurrency).
5. **R-07** mixed-DPI coordinate-space fix with the Windows live matrix.
6. **R-09–R-15** as separate, smaller changes after the above.

The first three sets are independent enough to be separate commits and
regression tests. None should be bundled into a single rewrite.

## Reproduction notes

The audit used isolated `APPDATA` directories and temporary config files; it
did not modify the repository's tracked data or the user's real config. The
live tests changed the desktop wallpaper only through the app, which restored
`c:\windows\web\wallpaper\themeb\img25.jpg` on quit; `--doctor` confirmed the
original background afterward. The temporary directories are under
`C:\Users\Lenovo\AppData\Local\Temp\ferramari-audit-*`.

The release binary used for the live probes was
`target\release\ferramari.exe` built from `fb6071f`. The test video was
`%USERPROFILE%\Videos\Ferramari\Power-Dancing-1080p.mp4`.

## Phase 1 progress

The first Phase 1 change set addresses R-01, R-02, R-03 and R-04 while keeping
the audit and verification results separate.

| Change | Verification |
| --- | --- |
| R-01: idempotent Windows static-background restore | New platform unit test; live rerun with static wallpaper off: stable mpv PID, `STATS_ACTIVE=true`, zero restarts/restore calls |
| R-02: validated, atomic backup promotion | New config tests; full `ferramari-core` suite |
| R-03: primary-then-backup display registry load | New display-id test; full `ferramari-core` suite |
| R-04: flush before `reload_config` | Pre-fix live reproduction recorded; post-fix app-level regression test still pending |
| R-05: publish errors to the snapshot | Code path now calls `set_operation_status`; per-command ack still open |
| R-06: live-player check for `is_playing` | New media liveness test; decode-before-frame state still separate |
| R-07: physical/logical monitor geometry | New dashboard unit test; live mixed-DPI matrix still open |
| R-08: bounded control transport | New platform tests for line size and client slots |
| R-09: status sequence and optimistic timeout | UI/client compile and existing tests; no new UI-state harness yet |
| R-10: numeric config ranges | New parse test |
| R-11: UTF-8 BOM handling | New config test |
| R-12: catalogue cache cleanup call | Startup call added; fake-root test still open |
| R-13: second-launch retry | New util timeout tests; two-process smoke still open |
| R-15: Linux formatting gate | `cargo fmt --all -- --check`; docs cleanup still open |

Post-change gates on the branch:

```text
cargo clippy --workspace --all-targets --locked -- -D warnings  exit 0
cargo test --workspace --locked                                 exit 0
cargo build --workspace --release --locked                      exit 0
```

Test totals after the change set: **462 passed, 0 failed** (16 binary + 357
core + 18 media + 57 platform + 8 UI library + 6 UI binary).

## Phase 0 exit

- [x] Baseline gates recorded.
- [x] Ten critical flows mapped in the plan; the flows covered by this pass
      are documented above.
- [x] Confirmed findings separated from risks and blocked live scenarios.
- [x] Regression tests named for each fix.
- [x] No source fix was made in this phase.
- [ ] Linux live matrix — blocked by hardware.
- [ ] Windows multi-monitor/mixed-DPI matrix — blocked by hardware.
