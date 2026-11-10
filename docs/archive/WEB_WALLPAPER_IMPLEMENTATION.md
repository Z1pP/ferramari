# Web Wallpaper Implementation Plan

> **Status:** Design complete — awaiting renderer integration
> **Date:** 2026-11-08
> **Related:** `docs/archive/BROWSER_ENGINE_RESEARCH.md` (engine selection), §7.176–178 (TODO)

## 1. Summary

Ferramari needs a way to render arbitrary HTML/CSS/JS web pages as live wallpapers, similar to the old GTK+ WebKitGTK implementation that was removed during the 2.0 migration. The selected approach is **headless Chromium via CDP** (Chrome DevTools Protocol), which has been evaluated against five alternatives.

## 2. Architecture Overview

```
┌─────────────────┐       ┌──────────────────────┐      ┌──────────────────────┐
│  Ferramari Core │──────▶│  WebRendererBackend  │─────▶│  Headless Chrome     │
│                  │       │  (ferramari-webext)  │      │  (system binary)     │
│  • Commands      │◀──────│  • CDP client        │◀─────│                      │
│  • Surface handle│       │  • Process manager   │      │  --remote-debugging- │
│  • Config        │       │  • Lifecycle         │      │    port=XXXXXX       │
└─────────────────┘       └──────────────────────┘      └──────────────────────┘
```

The `WebRendererBackend` crate is a new workspace member sitting between `ferramari-core` and the external Chrome process. It owns the lifecycle and communicates exclusively over CDP JSON-RPC, never touching any GUI framework.

## 3. Process Model

| Aspect | Detail |
|--------|--------|
| **Binary** | System-installed `google-chrome` or `chromium-browser`; found via `paths::tool()` |
| **Launch flags** | `--no-sandbox` (flatpak-safe), `--disable-gpu`, `--disable-software-rasterizer`, `--headless=new`, `--disable-background-timer-throttling`, `--disable-renderer-backgrounding` |
| **Debug port** | Ephemeral (`port=0`), discovered via `CDP` auto-negotiation or `chrome://version` scraping |
| **Protocol** | WebSocket on `ws://localhost:{port}/devtools/browser/{id}` (or `http` POST if ws unavailable) |
| **Lifetime** | Tied to `WallpaperSurfaceProvider` drop; explicit `Page.close` + `Browser.closeTarget` sent before exit |

## 4. CDP Command Flow (per monitor)

```rust
// Launch sequence
1. Browser.launch()                        // Start Chrome
2. Target.createTarget(url="about:blank")  // Empty page first
3. Browser.setWindowBounds(width, height)  // Match monitor geometry
4. Page.enable()                           // Enable Page domain events
5. Runtime.evaluate(expression=`document.body.style.background='transparent'`)
6. Page.navigate(url=webpage_config.url)   // Navigate to configured URL

// Play/Pause
7. Runtime.evaluate(`if(document.hidden){location.reload()}`) // Un-hide if needed
8. ...or use `ServiceWorkerRegistration` to pause network requests

// Stop
9. Page.close(targetId)                   // Close the page target
10. Browser.closeTarget(browseId)         // Close the browser if last page
11. Process.kill()                        // Ensure termination
```

## 5. Sandbox & Security Constraints

Per §7.174 (still open), the sandbox must enforce:

| Constraint | Mechanism |
|------------|-----------|
| **Network isolation** | Chrome flag `--disable-features=NetworkPrediction` + `--host-rules=MAP * .local` |
| **No file access** | `--disable-file-access` (default in headless) |
| **Process limits** | `rlimit_nproc`, `cgroups` memory/cpu caps in deployment config |
| **Audio redirect** | PipeWire loopback sink `ferramari-web-audio`; muted by default |
| **Execution sandbox** | Flatpak `--socket=pulseaudio --filesystem=xdg-config/chromium` only; no host fs |

## 6. Audio Handling

Web pages often contain audio. Two approaches:

**Approach A (preferred):** PipeWire virtual sink + mute-by-default.
- Creates `module-null-sink` named `ferramari-web-audio`
- Sets `load-module module-native-protocol-unix auth-anonymous=1`
- UI toggle `"Allow network for web wallpapers"` includes audio control
- Volume mapped to Firefox/Chrome master volume controls

**Approach B (fallback):** Block audio at CDP level.
- Send `Runtime.evaluate("document.suspendPlayback()")` on every page load
- `Runtime.evaluate("document.mozResumePlayback()")` on play command
- Works even on platforms without PipeWire

## 7. Performance Budget

Based on Benchmarks.md estimates:

| Metric | Budget | Monitoring |
|--------|--------|------------|
| RAM per instance | < 300 MB | `/proc/[pid]/status` VmRSS in `Sample` |
| Startup time | < 2s | Time from `Command::Play(Web)` to visible frame |
| Frame sync | ≤ 1 frame vs server cycle | `Page.frameScheduledNavigation` event timing |
| Memory leak guard | Restart if RSS > 500 MB after 24h uptime | Watchdog in `App::tick()` using `restart_policy` |

## 8. Integration Points

### New crates/modules

| Path | Purpose | Dependencies |
|------|---------|-------------|
| `crates/ferramari-webext/` | Workspace member: CDP client, process management | `tokio`, `futures`, `serde_json` (no GUI) |
| `crates/ferramari-webext/src/cdp/` | WebSocket/http CDP transport | `tokio-tungstenite` |
| `crates/ferramari-webext/src/renderer.rs` | Renderer abstraction implementing core contracts | `ferramari-core` |
| `crates/ferramari-webext/src/prober.rs` | Feature detection script run on startup | None |

### D-Bus surface additions

New methods on `io.github.ferramari.ferramari.web` interface:

```
enable(url: str)          -> void
disable()                 -> void
reload()                  -> void
set_volume(db: float)     -> void    # negative = mute
is_enabled()              -> bool    # property
is_ready()                -> bool    # property: chrome responded to ping
error_message()           -> string? # property: last error text
```

Properties updated through `App::eco_plan_for()` → `WebRenderer::apply_state(State)` mapping identical to video engine.

### Config migration

`config.json` additions:

```json
{
  "web_wallpaper": {
    "enabled": false,
    "url": "",
    "allow_audio": false,
    "volume_db": -1.0,
    "max_rss_mb": 500
  }
}
```

## 9. Phase Timeline

| Phase | Duration | Deliverables |
|-------|----------|--------------|
| **Phase 0** | POC | Prober runs, CDP connects to local page, screenshot captured |
| **Phase 1** | Alpha | Renderer lives alongside mpv; handles start/stop/lifecycle; basic layout |
| **Phase 2** | Beta | D-Bus interface complete; audio control; watchdog restart; flatpak bundle tested |
| **Phase 3** | RC | Performance benchmarks passed; security audit (network isolation); release notes |
| **Phase 4** | GA | Full rollout behind feature flag; default disabled |

## 10. Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Chrome updates break CDP API | Low-Medium | High | Pin Chrome version in Flatpak SDK; test matrix per major release |
| Memory leaks in long-lived tabs | Medium | High | Watchdog restart at configurable interval (default 24h); OOM tracking via /proc |
| Audio wakes users at night | Low | Medium | Mute-by-default; sleep timer in config |
| Flatpak sandbox prevents Chrome launch | Medium | Medium | Use `flatpak-spawn` from host; documented workaround in Flatpak manifest |
| Web content too heavy for low-end GPUs | Medium | Low | Fallback to static snapshot (`Page.captureScreenshot`); configurable fallback mode |

## 11. Acceptance Criteria

Before §7.178 ("Restore web wallpaper") can be marked complete:

1. ✅ Headless Chrome starts and connects via CDP within 2 seconds
2. ✅ Renders a sample webpage as wallpaper matching monitor resolution
3. ✅ Respects Eco policy states (pauses/resumes like mpv)
4. ✅ Can be started/stopped/restarted via D-Bus without crashing mpv processes
5. ✅ Audio either redirected or blocked; never plays unexpectedly
6. ✅ Flatpak build succeeds; process stays within 300 MB RAM budget
7. ✅ No GTK/GDK/WebKitGTK symbols in final binary (`ldd` verification passes)
8. ✅ Unit tests cover: prober failures, CDP disconnect, invalid URLs, large payloads
9. ✅ Documentation covers installation, troubleshooting, and known limitations
