# Ferramari MPV Process Lifecycle Audit

> **Date:** 2026-11-07  
> **Version:** 2.0.0  
> **TODO Reference:** §7 — Media and web wallpaper engine (items 110–116)  
> **Source files:** `crates/ferramari-media/src/mpv.rs`, `engine.rs`, `crates/ferramari-core/src/supervisor.rs`

## 1. Spawn phase (§7.1)

| Aspect | Detail | Status |
|--------|--------|--------|
| Binary discovery | `ferramari_core::paths::tool("mpv")` — checks sibling binary first, then PATH | ✅ |
| Environment cleanup | `WAYLAND_DISPLAY` removed; X11-only mode enforced via `--gpu-context=x11egl` | ✅ |
| IPC server socket | `--input-ipc-server=<socket>` creates Unix socket in `~/.cache/ferramari/mpv-spid-N.sock` | ✅ |
| Socket uniqueness | `SOCKET_COUNTER` ensures no collisions across engines | ✅ |
| Input stripping | No OSC, OSD, keybindings, cursor autohide disabled for wallpaper use | ✅ |
| Audio control | `--no-audio` flag for secondary monitors; audio routed to primary | ✅ |
| Loop control | `--loop-file=inf` enables infinite loop by default | ✅ |
| HW decode | `--hwdec={auto|vaapi|none}` passed through user config | ⚠️ Section 7.2 |

**Findings:** All spawn flags are minimal and correct for a background wallpaper renderer. No unnecessary GUI elements exposed.

## 2. IPC communication (§7.3, §7.4)

| Aspect | Detail | Status |
|--------|--------|--------|
| Connection timeout | `SOCKET_TIMEOUT = 3s` with exponential retry every 25ms | ✅ |
| Command timeout | `IPC_TIMEOUT = 1s` per read/write on the stream | ✅ |
| Protocol | JSON-RPC over Unix domain socket (`{"command":[...], "request_id":N}`) | ✅ |
| Error classification | `is_fatal()` — I/O errors terminate, command-level `error` codes are recoverable | ✅ |
| Response matching | Filters events by `request_id`; ignores stale replies from other requests | ✅ |
| Worker thread | Dedicated `ferramari-mpv-ipc` thread runs the entire connect→serve→retry loop | ✅ |

**Findings:** IPC design is robust. Fatal I/O errors set `ipc_dead` flag which prevents further enqueueing and signals the caller to restart the player.

## 3. Park/unpark cycle (§7.2 verification)

The Eco Engine park/unpark mechanism works as follows:

```
Engine.park_if_due() → Mpv.park() → IPC Request::Park
    ↓ (idle mode + time-pos capture + stop)
    mpv process stays alive, socket open, decoder unloaded
    ↓ (user resumes / rotation advances)
Engine.unpark(source) → Mpv.unpark(source) → IPC Request::Unpark
    ↓ (loadfile + seek to saved position)
    Cold-start cost avoided
```

| Aspect | Detail | Status |
|--------|--------|--------|
| Position preservation | `time-pos` captured before idle; restored on unpark | ✅ |
| Parking guard | `park_allowed` flag prevents premature parking during load or fade | ✅ |
| Threshold-based | Uses `Duration` threshold from core (typically 15-30s idle) | ✅ |
| Failed unpark handling | Warns but does not crash; falls back to new spawn | ✅ |

**Findings:** Park/unpark correctly balances resource savings against complexity. No known race conditions between park timer and unpark trigger.

## 4. Player death detection (§7.1 revalidation)

The system uses triple-check for player liveness:

| Check Layer | Location | Mechanism | Status |
|-------------|----------|-----------|--------|
| IPC health | `Mpv.is_running()` | Checks `ipc_dead` atomic + `child.try_wait()` | ✅ |
| Engine polling | `Engine.player_alive()` | Wraps `Mpv::is_running()` call | ✅ |
| Application sync | `App.window_sync()` | Calls `player_alive()` each tick; removes dead window for recreation | ✅ |

Restart policy uses `supervisor::RestartPolicy` with exponential backoff:
- Max retries: `MAX_RETRIES = 5`
- Initial delay: 1 second, doubles on each failure
- After given up: window removed entirely, waits for next content change

**Findings:** Death detection chain is thorough. The only gap is that `try_wait()` returns `Ok(None)` for running processes but `Err(EAGAIN)` on Linux might be treated incorrectly — currently the `_ => { kill + wait }` catch-all handles it safely.

## 5. Cleanup and Drop (§7.1 — cleanup validation)

On `Drop<Mpv>`:

1. Sets `stop.store(true)` to wake the IPC worker's connect loop
2. Sends `Request::Quit` to the IPC channel
3. Waits up to 1 second for child exit (exponential poll at 20ms intervals)
4. Falls back to `child.kill()` + `child.wait()` if deadline exceeded
5. Removes the socket file from disk
6. Joins the IPC worker thread

**Findings:** Cleanup is aggressive but correct — wedged decoders cannot block the session indefinitely. The 1-second grace period covers slow mpv shutdown while still guaranteeing termination.

## 6. Known issues and recommendations

### Section 7.1 — Audit mpv process lifecycle ✓ COMPLETE
All spawn, IPC, cleanup, and restart paths reviewed. Design is solid.

### Section 7.2 — Validate hardware decode per OS/GPU backend
**Status:** Partially complete. The `--hwdec` parameter passes through to mpv, but there is no runtime validation of whether the detected GPU supports the requested decode path. On some systems VA-API may fail silently and fall back to software without clear logging.

**Recommendation:** Add `mpv --log-level=debug vd=info` output parsing after spawn to detect hardware decode failures and log warnings. Consider falling back to `--hwdec=auto` when explicit request fails.

### Section 7.3 — Stream resolution, cache invalidation, yt-dlp failures
**Status:** Not addressed in this audit. Requires separate review of `ytdlp.rs` and stream handling in `engine.rs`.

**Recommendation:** Schedule for next audit cycle with focus on:
- Cache invalidation for yt-dlp metadata
- Timeout/retry behavior for stream downloads
- Validation of resolved URL formats

### Section 7.4 — Web wallpaper sandbox
**Status:** Unsupported per documentation. Web page wallpapers require a replacement renderer for WebKitGTK.

**Current state:** Webpage enum variant exists in `ContentKind` but `Engine::web()` always returns an error. This is correct behavior — do not advertise unsupported functionality.

### Section 7.5 — Restore web wallpaper after tests pass
**Status:** Blocked by §7.4. No action until renderer is selected.

### Section 7.6 — Hide web-wallpaper UI flag until renderer present
**Status:** Already implemented. No `"MODE_WEBPAGE"` action buttons exposed in Settings page. Only shown when a webpage source has been manually assigned via D-Bus or package data.

## 7. Summary scores

| Sub-item | TODO ref | Status | Notes |
|----------|----------|--------|-------|
| 1. Audit mpv lifecycle | §7.1 | ✅ Done | Full code review completed |
| 2. Hardware decode validation | §7.2 | ⚠️ Partial | Parameter passes through; need runtime GPU capability check |
| 3. Stream/cache/ytdlp failures | §7.3 | ❌ Out of scope | Requires separate audit of ytdlp.rs |
| 4. Web wallpaper renderer selection | §7.4 | ❌ Blocked | Awaiting renderer decision (non-GTK) |
| 5. Restore web wallpaper | §7.5 | ❌ Blocked | Depends on §7.4 completion |
| 6. Hide web feature flag | §7.6 | ✅ Done | Verified in code — no UI actions exposed |
