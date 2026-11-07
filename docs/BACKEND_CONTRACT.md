# Backend contract

The boundary a platform backend has to satisfy. Read this before starting a
second OS implementation: everything here is what the Linux backend already
owes, so a new backend either matches it or the contract gets amended first,
in writing, not by silent exceptions in one platform's code.

Companion documents: `docs/UI_ACTION_INVENTORY.md` (what the product does),
`docs/AUDIT.md` and `docs/MPV_LIFECYCLE_AUDIT.md` (media layer).

## 1. Layering

```
ferramari-ui (Iced)          ferramari (server binary)
      |  D-Bus client              |
      +-----------------------------+
                D-Bus API  (ferramari-platform::dbus)
                    |
              App (src/app.rs)  — owns all playback state
          /         |          \        \
   ferramari-core  media     platform    (injected at startup)
```

- `ferramari-core` is the policy and data layer. It compiles on every target
  and must never import X11, Win32, Cocoa, D-Bus, GTK or Iced types. Nothing
  in it may assume a process-local file layout beyond what `paths` resolves.
- `ferramari-media` renders. It receives an opaque embedding handle and keeps
  the surface alive; it must not know which toolkit produced the handle.
- `ferramari-platform` is the OS adapter. Native handles, native API types and
  watcher threads live here and nowhere else.
- The binary wires concrete backends into the app at startup. The app never
  selects a backend by `cfg`.

## 2. What a backend must provide

Startup injection (`src/main.rs`), each one a `Box<dyn …>` from core:

| Contract | Responsibility | Linux implementation |
| --- | --- | --- |
| `DisplayProvider` | enumerate connected displays | XRandR (`monitors::list`) |
| `WallpaperSurfaceProvider` / `WallpaperSurface` | create and own a surface fit for embedding | X11 desktop-type window |
| `PowerProvider` | current power-supply state | sysfs polling |
| `PlatformEventSource` / `PlatformWatchers` | start OS event sources, hand back a teardown token | X11 EWMH + RandR watchers, D-Bus session/lock/sleep/idle watchers |
| `PlatformCapabilities` | what is actually reachable in this session | `desktop::platform_capabilities` |

Rules that apply to all of them:

1. **No native leakage.** An embedding handle crosses the boundary as `u64`
   and is documented per backend. Core and media must never persist it, print
   it as meaningful, or interpret it.
2. **Report, do not assume.** Support is `Supported`, `Partial` or
   `Unsupported`, decided by probing the live session (`--doctor` prints the
   result). A capability that only covers a subset of clients is `Partial`,
   not `Supported`: XWayland sees X11 windows but no native Wayland windows,
   and the report says so.
3. **Degrade to a state, not a crash.** No X server, no session bus, no
   battery, no driver: the backend returns an empty list, `Unsupported` or
   `None`, and the product keeps running with a visible limitation.
4. **Never block the caller.** Enumeration and surface creation are called
   from the command-loop thread. Any probe that can stall (D-Bus, subprocess,
   GPU query) runs behind a deadline or on its own thread.

## 3. Threading and ownership

One thread owns playback state; everything else produces messages.

- The **command loop** (`App::run`) is the only thread that mutates `App`,
  players and config. It blocks in `recv_timeout` until the earliest deadline,
  then drains everything already queued.
- **Watchers** run on their own threads and only ever send `Command` values.
  They must not touch application state, media objects, or each other.
- The **D-Bus interface** answers reads from a `Snapshot` behind a mutex and
  turns writes into commands. A D-Bus call therefore never blocks on the
  player; the reply may describe the state just before the command applied,
  which clients tolerate by polling the snapshot again.
- The **Iced UI** owns its widget tree on its own event-loop thread. UI state
  and widget handles never move into a background worker.

Consequence for backends: a command that arrives from a watcher is already
serialised. There is no second queue, so there is no ordering to negotiate
between watchers — only the order the channel delivers them in.

## 4. Event ordering

Guarantees the core relies on, which a new backend must keep:

1. **Commands are applied in channel order** on the loop thread. Nothing
   reorders or coalesces them implicitly.
2. **Periodic jobs are deadlines, not threads**: player watch (1 s), rotation
   (5 s), power poll (30 s). They run after the queued commands are drained,
   so a burst of events cannot delay a tick indefinitely.
3. **A hotplug is a debounce, not a rebuild.** Watchers send
   `MonitorsChanged`; the loop collapses a burst into one
   `MonitorsChangedNow` after `MONITOR_DEBOUNCE`. A backend that emits
   per-output events therefore costs nothing extra.
4. **Config is flushed before it is re-read.** A hotplug writes through the
   debounced config first, then reloads, so a fast slider edit is not lost to
   a file that does not contain it yet.
5. **Session signals are aggregated, never overwritten.** Lock state comes
   from more than one source (screensaver services and logind); the app keeps
   each source's last value and treats any locked source as locked, so one
   source's unlock cannot cancel another's lock.
6. **Terminal events are commands too.** `Command::Quit` from an end-session
   signal or a Unix signal goes through the same loop, so every shutdown path
   runs the same teardown.

## 5. Cancellation and shutdown

- Watcher threads stop when the command channel closes: `send` fails and the
  thread returns. This is the portable cancellation mechanism, and it is why
  watchers hold no state worth flushing.
- Watchers with a native event loop additionally take a stop flag and are
  joined when their token is dropped, so the X connection is not torn down
  while an event thread still reads it.
- `PlatformEventSource::start` returns `Box<dyn PlatformWatchers>`; dropping
  that box is the documented teardown signal. A backend that cannot stop a
  subscription must say so in this document rather than pretend the token
  covers it.
- Application shutdown order is fixed: flush config → close windows (stops
  players and destroys surfaces) → restore the static background → drop
  watchers. Surface destruction therefore always precedes connection teardown.
- `Drop` on a surface releases its native resources; on a media engine it
  stops the fade threads and kills the player.

**Known gap.** The D-Bus session/lock/sleep/idle watchers in
`ferramari-platform/src/handlers.rs` block on `receive_signal` and are only
released when the process exits: the token does not stop them. Today the only
in-process teardown of the whole app is process exit, so the gap is not
reachable, but a backend that restarts its event sources without exiting would
inherit it. Fix shape: give each watcher an owned `zbus::blocking::Connection`
plus a self-message or a poll deadline, so the signal loop can be woken and the
thread joined. Until then `PlatformWatchers` promises less than its name.

## 6. Display identity

A config key is user-visible data, so it must not follow enumeration order.

- Each display carries a `DisplayIdentity`: connector port, EDID when the
  server exposes it, physical size, and any backend-native stable id.
- `MonitorInfo::stable_id` digests the *screen* (EDID/native serial) and falls
  back to the *port*. Resolution, position and scale are excluded on purpose.
- `display_ids::DisplayIdRegistry` maps identity → config key and persists it
  in `display-ids.json`. A screen seen for the first time keeps the name the
  backend reported, which is what pre-2.0 configs already contain; a known
  screen reclaims its key.
- A backend that can report nothing identifiable must say so: the display
  keeps its enumeration name and is not recorded. Inventing a key would
  rewrite entries the user can see.

Under XWayland EDID is hidden by the compositor, so the identity is
connector-based: a wallpaper follows the socket, not the panel. A backend with
a stronger identity source is expected to use it.

## 7. Data locations

`ferramari-core::paths` resolves config, cache, data and video-library roots
per platform and must stay the only place that knows them. A backend may not
build a path by hand, and its helpers must be found through `paths::tool` so a
sandboxed or non-default prefix still works.

## 8. Before a second backend is accepted

- [ ] Implements every contract in §2 without a `cfg` branch inside core or media.
- [ ] Capability probing returns `Partial` wherever coverage is a subset, and
      `--doctor` shows the reason.
- [ ] Stable display ids use the strongest identity the OS exposes; the
      fallback rule and its limits are recorded in §6.
- [ ] Event sources can be stopped, and §5 has no "only at process exit" case.
- [ ] Surface lifecycle survives a player crash and a surface destruction and
      re-creation without leaking native handles.
- [ ] Contract tests run against the fake providers in
      `ferramari-core/src/platform.rs`, then against the real backend.
- [ ] Smoke matrix recorded: multiple displays, reconfiguration, sleep/wake,
      lock/unlock, session restart, mixed DPI.
- [ ] `docs/MIGRATION_TODO.md` sections 3–6 updated with what is and is not
      supported; a limitation stays documented until it is removed.
