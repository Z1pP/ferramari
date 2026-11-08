# Browser Engine Research for Web Wallpapers in Ferramari

**Status:** Investigation (MIGRATION_TODO §7)  
**Date:** 2026-01-08  
**Reference:** `docs/MIGRATION_TODO.md` item "Select a GTK-free browser engine or isolated browser process for web wallpapers"  

---

## 1. Problem Statement

Ferramari's previous GTK implementation used WebKitGTK to render web wallpaper content — essentially loading arbitrary URLs as full-page backgrounds that auto-play audio, run animations, and respond to user interaction (right-click menus, etc.). The GTK removal eliminated this entirely. `Engine::web()` now returns a clear error.

The new Iced-based architecture has no embedded browser at all. A solution must:

- Render a full web page (HTML + CSS + JavaScript) as a continuously-updated visual surface.
- Optionally play audio from the page.
- Be embeddable behind Ferramari's media layer without requiring GTK/GDK/WebKitGTK.
- Run as a separate process with network isolation so untrusted pages cannot access local resources.
- Produce an opaque surface handle that mpv (or a future compositing layer) can display on the wallpaper surface.

Key constraints inherited from the existing architecture:
- **No single-process rendering.** The command loop is single-threaded; the browser must be out-of-process.
- **Surface abstraction.** Media layer only knows `u64` embedding handles. Whatever produces frames for the web wallpaper must expose one of those.
- **Sandbox requirement.** Web pages are untrusted by definition. No direct filesystem access, no cookie persistence across sessions, no network access unless explicitly permitted.

---

## 2. Options Survey

### Option 1: Headless Chromium/Chrome via Chrome DevTools Protocol (CDP)

**Approach:** Launch a headless Chromium instance, communicate over CDP to load URLs, extract composited frames via the `CaptureDocument` or `LayoutSnapshot` APIs, composite into the wallpaper surface.

#### Technical details

Chromium exposes a comprehensive D-Bus-like API (CDP) on a TCP port when launched with `--remote-debugging-port`. Key relevant methods:

| CDP Method | Purpose | Feasibility |
|---|---|---|
| `Page.navigate` | Load a URL | Yes |
| `Runtime.evaluate` | Execute JavaScript | Yes |
| `DOMSnapshot.captureSnapshot` | Get rendered DOM tree | Informational only, not visual |
| `Page.captureScreenshot` | Get a PNG of the current frame | Works but round-trips large images |
| `CSS.forcePseudoState` | Style manipulation | Yes |
| `Input.dispatchMouseEvent` | Forward mouse events | Yes |
| `Fetch.enable` / `Network.setBlockedURLs` | Network interception | Yes |
| `Emulation.setFocusEmulationEnabled` | Focus behavior control | Yes |
| `Browser.getVersion` | Version info | Yes |

For continuous wallpaper rendering, there are two sub-approaches within CDP:

**Sub-approach A — Polling screenshots (`Page.captureScreenshot`):**
Periodically call `Page.captureScreenshot` (PNG format) on every requestAnimationFrame cycle (~60fps). Decode the PNG in Rust and blit it onto the wallpaper surface.

**Pros:**
- Simplest integration path. CDP is well-documented and stable.
- Works regardless of compositor — just a TCP connection.
- Frame data is self-contained; no complex shared memory negotiation.

**Cons:**
- High bandwidth: a 1920×1080 screenshot at ~60fps ≈ 30–60 MB/s of image data over the IPC channel.
- Decoding PNG at 60fps is CPU-intensive (requires libpng/ravif decode per frame).
- Latency: CDP messages are JSON/RPC, adding round-trip overhead.
- Audio must be handled separately (CDP has no `AudioContext.getDestinations()` equivalent; need to capture through PulseAudio/PipeWire loopback).

**Sub-approach B — Shared buffer / dmabuf export (advanced):**
Chromium's internal rasterizer can export frames via shared memory (e.g., using `--enable-gpu-rasterization` + reading GPU buffers). This is what tools like `chrome-capture` and various streaming setups do internally. In practice, this requires modifying or patching Chromium's dump-render-tree code, which is impractical for Ferramari.

**Verdict on CDP polling:** Works as a functional prototype but does not scale to multiple monitors at 60fps without significant CPU cost. Acceptable for initial web-wallpaper support if users understand the trade-off (higher idle CPU than video wallpaper).

#### Runtime dependency size

| Component | Size (binary) | Notes |
|---|---|---|
| chromium-browser | ~350–500 MB | System package (apt/pacman). Ferramari doesn't bundle. |
| chrome-devtools-protocol client | ~200 KB | Hand-rolled Rust library or crates.io crate |

**Net impact:** Ferramari does not ship Chromium. The user's system must have it installed. This is similar to requiring Firefox for GNOME Shell extensions that useGIO/GJS.

#### License compatibility

Chromium is BSD-style (see Chromium license: https://chromium.googlesource.com/chromium/src/+/main/LICENSE). This is fully compatible with GPLv2. The CDP protocol itself is a defined interface, not covered by any license restriction.

#### Feature completeness

| Feature | CDP Support | Notes |
|---|---|---|
| HTML/CSS layout | Full | Complete Blink engine |
| JavaScript execution | Full | V8 engine |
| Audio playback | Partial | No direct capture; needs PipeWire/PulseAudio loop |
| WebGL canvas | Full | Can read back pixels via `Rendering.disable` + screenshot |
| Video element | Full | Renders within the page |
| CSS animations | Full | Continuous animation works |
| Iframes | Full | Subject to CORS/security policy |
| WebSocket | Full | Native support |
| Service Workers | Full | Can interfere with wallpaper (background sync, push) |
| Fetch/XHR | Full | Configurable via `Network.setRequestInterception` |

#### Resource usage

| Metric | Typical value (single tab) |
|---|---|
| RAM | 200–400 MB (headless, minimal flags) |
| CPU (idle, animated page) | 5–15% single core (depends on JS complexity) |
| CPU (idle, static page) | 1–3% (paint throttling helps) |
| Disk cache | ~100 MB (configurable per-profile) |

Per web-wallpaper instance, expect 300 MB RAM baseline and 5–10% CPU on modern hardware for typical animated wallpapers.

#### Security considerations

- **Sandbox:** Chromium already runs renderer processes sandboxed via seccomp-bpf, namespace isolation, and credential isolation on Linux. Launched with `--no-sandbox` only if absolutely necessary (not recommended).
- **Network isolation:** CDP supports `Network.setBlockedURLs` to whitelist/blacklist domains. Can restrict to specific origins.
- **Cookie storage:** Use `--user-data-dir=/tmp/chrome-webwallpaper-{random}` per session to prevent persistent cookies/caching across runs.
- **Filesystem access:** Chromium sandbox prevents file:// protocol access unless explicitly enabled with `--allow-file-access-from-files` (not recommended).
- **Memory limits:** Chromium has `--max-old-space-size` flag for V8 heap; can cap at 512MB per instance.

#### Implementation effort

| Phase | Estimated time | Description |
|---|---|---|
| POC: launch + screenshot poll | 1 week | Proof-of-concept: spawn chromedriver, poll screenshots, blit to surface |
| Integration: frame loop + fade | 1 week | Connect to Engine lifecycle, support play/pause/fade, handle crashes |
| Security hardening | 1 week | User-data-dir isolation, network filtering, resource limits (cgroups?) |
| Audio bridge | 1–2 weeks | PipeWire/PulseAudio loop capture (hardest part) |
| Mouse event forwarding | 3 days | Click-to-interact feature for interactive wallpages |
| Error handling + monitoring | 1 week | Process death detection, restart policy, health checks |

**Total estimate: 5–7 weeks** for a working prototype with reasonable polish.

---

### Option 2: Servo Embedded Renderer

**Approach:** Embed the Servo rendering engine directly into the Ferramari process using `servo` or `geckodriver`-style approaches. Servo is a modern HTML/CSS renderer written in Rust, designed for parallel rendering.

#### Technical details

Servo provides a rendering backend that takes a DOM tree and CSS and produces rasterized output. Recent developments:

- **GeckoView** (Firefox's Android renderer) is NOT embeddable outside Android — it uses Java JNI bindings specific to the platform.
- **Pure Servo** renders to pixel buffers. There is no built-in networking stack in core Servo; it relies on `hyper`-based HTTP clients for fetch, which is available but incomplete compared to a full browser.
- **Wry/Tauri** ships a WebKit2gtk wrapper for macOS/Linux — this brings back WebKitGTK, which Ferramari explicitly removed.

Current status of Servo embeddability:
- `@servo/servo` (Rust crate) compiles independently and can render HTML to an image surface.
- However, Servo's networking is primitive: it lacks TLS certificate pinning, HTTP/2 ALPN, WebSocket, and many ES6+ features relative to Blink/Gecko.
- No official public API for "load a URL and get frames". You must build the fetch → parse → layout → paint pipeline yourself.
- JavaScript engine: Servo has historically been experimental here. As of 2025, SpiderMonkey integration is limited to Fennec/Android builds.

#### Runtime dependency size

| Component | Size (binary) | Notes |
|---|---|---|
| servo (compiled statically) | ~50–150 MB (static libs) | Must compile alongside ferramari |
| rustc toolchain requirement | — | Requires nightly for some features |

**Net impact:** Would significantly increase build times for Ferramari (Servo takes 30–60 minutes to compile from scratch). Not suitable for a project with limited CI resources.

#### License compatibility

Servo is MPL-2.0. Mozilla also dual-licenses some components under Apache-2.0. Compatible with GPLv2 via MPL-2.0 section 3 provisions (larger-work rule).

#### Feature completeness

| Feature | Servo Support | Notes |
|---|---|---|
| HTML/CSS layout | Good | Modern CSS Grid, Flexbox, mostly complete |
| JavaScript execution | Poor/experimental | SpiderMonkey integration exists but is not maintained for desktop embed scenarios |
| Audio playback | None | No audio context implementation |
| WebGL | Limited | Basic WebGL 1.0 may work; WebGL 2.0 unreliable |
| Video element | Limited | Depends on GStreamer binding; incomplete |
| CSS animations | Good | Supported |
| Iframes | Limited | Same-origin mostly; cross-origin is broken |
| WebSocket | Experimental | May exist but untested |

#### Resource usage

Lower than Chromium since there's no entire browser framework overhead. Expected ~50–100 MB RAM and 2–5% CPU for simple pages. But the poor JS support means most interactive web wallpapers would simply not work.

#### Security considerations

Since Servo embeds directly (same process), security depends entirely on Rust safety guarantees. Any CVE in Servo's parser or renderer is a potential RCE in Ferramari itself. Unlike Chromium's multi-process sandbox, there is no renderer-process boundary — a malicious page could theoretically crash the entire server process via a UAF in the HTML parser.

#### Implementation effort

| Phase | Estimated time | Description |
|---|---|---|
| Compile Servo + basic loader | 2 weeks | Get Servo building with Ferramari; load an HTML string |
| Networking stack integration | 2–3 weeks | Wire up hyper/tokio HTTP client with TLS |
| JS engine setup | Indefinite | SpiderMonkey integration is the hardest blocker |
| Paint callback integration | 1–2 weeks | Get pixel buffers back into Engine |
| Audio bridge | Blocked | No audio context to bridge |

**Total estimate: 3–6 months** with no guarantee of web-wallpaper usability due to incomplete JS/audio support. Servo is primarily a research project right now, not a production browser engine ready for embedding in third-party applications.

**Verdict on Servo:** Not viable for web wallpapers given incomplete JavaScript support and the lack of an audio path. Worth watching for the future but should not block implementing web wallpapers in Phase 1 of the migration.

---

### Option 3: mpv Built-in Web Support

**Approach:** Investigate whether mpv's existing media engine can render web content.

#### Technical assessment

mpv is a media player. Its video output subsystem renders decoded video frames to a window. It does **not** contain an HTML/CSS/JavaScript rendering engine.

- mpv supports `--input-media-keys`, `--script`, `--playlist`, and Lua scripting.
- mpv has no concept of loading HTML files, parsing CSS, or executing JavaScript.
- `mpv --web` does not exist.
- mpv scripts can make network requests via `socket.connect()` but receive raw data, never interpreted HTML.
- Some projects have experimented with injecting canvas elements via mpv's GUI layer, but this is a hack, not a feature.

There are also related questions:

- **Can mpv stream from an HLS/DASH manifest that originates from a web service?** Yes, but this is still a video stream, not general HTML rendering.
- **Can mpv act as a proxy between a web page and the wallpaper surface?** No — mpv consumes media files/URLs, not rendered page frames.

**Verdict on mpv:** Cannot render web wallpapers. mpv's role is limited to video/stream decoding. Web wallpapers require a fundamentally different rendering pipeline.

---

### Option 4: FlutterWebView / Tauri WebView Alternatives

Tauri v2 ships with WebView2 (Windows), WebKit2 (macOS), and WebKitGTK (Linux) as its default web views. Ferramari has eliminated GTK/WebKitGTK. Let's evaluate the alternatives:

#### 4a. Tauri WebView on Linux

Tauri's Linux backend is hardcoded to WebKitGTK. There is no Wry/tauri-wry configuration path to use anything else (CEF, Edge WebView2, or Apple WKWebView are non-Wayland/non-X11-only).

**Not viable for Linux.** Would reintroduce the exact dependency Ferramari spent months removing.

#### 4b. Flutter WebView (`flutter_webview_plugin` / `webview_flutter`)

Flutter's webview plugin wraps platform-native web views:
- Android: WebView (Chromium-based)
- iOS: WKWebView
- Windows: WebView2 (Edge/Chromium)
- Linux/macOS: **WebKitGTK**

Same problem as Tauri — Linux defaults to WebKitGTK. Flutter does not offer CEF or Chromium Embedded Framework as a backend option on Linux without heavy custom plumbing.

**Not viable for Linux.**

#### 4c. cef-rs (Chromium Embedded Framework Rust bindings)

[`cef-rs`](https://github.com/xcmake/cef-rs) provides Rust bindings for Google's CEF, which bundles Chromium and lets you embed a full browser with a real `wl_surface`/`XID` handle.

**Technical feasibility:**
- CEF produces actual `wl_shm` or XID surfaces that can be passed directly to mpv.
- Full browser engine — everything Option 1 supports and more.
- Handles Web Audio API natively (CEF routes audio through the host process).
- No CDP polling — the browser renders directly into your surface.

**Trade-offs:**

| Aspect | Details |
|---|---|
| Binary size | CEF redistributable is ~250–400 MB |
| License | CEF is BSD-3-Clause but includes Chromium licenses. Redistribution requires including ALL Chromium license files. |
| Build | Pre-built CEF binaries available from Chromium project (~2 GB download). Building from source is impractical (~3 hours on good hardware, massive disk). |
| Platform support | Linux/X11 supported. Wayland support improving but historically spotty (uses XWayland internally on some configurations). |
| Maintenance | CEF release cadence is quarterly; lagging behind upstream Chromium by ~1 major version. |
| Memory | ~300–500 MB per webview instance (similar to headless Chrome). |
| API quality | Mature, documented C++ API wrapped in safe Rust. Stable. |

**Implementation effort:**
- Integrate `cef-rs` into ferramari-platform: 2–3 weeks.
- Surface lifetime management matching the existing X11/Wayland model: 1 week.
- Audio routing from CEF to PipeWire: 1 week.
- Sandbox/isolation hardening: 1 week.
- Crash recovery and monitoring: 1 week.

**Total estimate: 5–6 weeks.** Comparable to Option 1 but with higher binary footprint and less flexibility for headless/low-memory modes.

**Recommendation:** Strong contender as an alternative to CDP polling. If Ferramari targets a distribution where Chromium is always present (Flathub, Snap), CEF might actually reduce overall install size because the user already has Chromium and CEF reuses the same libraries.

---

### Option 5: WebRender + Custom Compositor

**Appro approach:** Use Mozilla's WebRender (the GPU-accelerated 2D/3D renderer used by Firefox) directly, fed by Gecko's layout engine. This is what makes Firefox fast on Wayland.

#### Technical assessment

WebRender is Mozilla's compositor that renders WebKit-agnostic HTML/CSS/JavaScript (from Gecko) into GPU textures. However:

- **WebRender is not standalone.** It renders the output of Gecko's layout/paint pipeline. There is no way to feed arbitrary HTML/CSS into WebRender without Gecko.
- **Gecko is not embeddable outside mobile platforms.** Firefox's desktop build uses the full application shell, profile management, and UI toolkit bindings. The `geckodriver` / `firefox --headless` approach creates a full Firefox process — effectively Option 1 with Firefox instead of Chrome.
- The `geckolib` / `libxul` path was experimental and deprecated years ago. There is no maintained Rust crate for embedding Gecko on desktop Linux.
- Firefox's own headless mode (`firefox --headless`) is essentially a glorified CLI browser with no programmatic surface handle API. It outputs to `/dev/null` or captures screenshots.

Even if embedding were feasible:
- WebRender targets Mesa/EGL/Vulkan. Produces GPU textures, not pixel buffers. Integrating with mpv's rendering pipeline (which expects X11 windows or Wayland shm) would require complex EGL sharing.
- Memory footprint: ~500+ MB for a single Gecko instance (heavier than Chromium).
- Build times: compiling Firefox gecko-dev takes 2–4 hours.

**Verdict:** Not practical. The "custom compositor around WebRender" approach would require building almost an entire browser first, then integrating it. No advantage over Option 1 or Option 4.

---

### Option 6: Isolated Chromium Process with Shared Memory Compositing

**Approach:** A variant of Option 1 where instead of polling screenshots over JSON-RPC, we establish a proper shared memory channel for frame exchange.

#### How it would work

1. Launch `chromium --headless=new --disable-gpu-compositing --offscreen-mode --remote-debugging-port=XXXX --disable-software-rasterizer` (or similar flags).
2. On startup, Chromium exports a `wl_drm` or `shmem` file descriptor for its primary output surface.
3. Ferramari imports this FD via `wp_linux_dmabuf_v1` and composites frames onto the wallpaper surface.
4. CDP controls navigation, input, and lifecycle (load URL, reload, close tab).

#### Why this differs from Option 1

Option 1 polls `Page.captureScreenshot` which creates PNG-encoded images that travel over the CDP TCP connection and must be decoded in Rust. Option 6 bypasses the encoding/decoding bottleneck entirely by sharing raw pixel buffers through shared memory.

**Advantages:**
- Near-zero CPU overhead for frame transfer (just a memcpy or mmap).
- Can sustain 60fps even on high-resolution multi-monitor setups.
- Lower latency between page repaint and visible update.

**Disadvantages:**
- Extremely complex to implement correctly. Requires knowledge of both Chromium's internal buffer management and Wayland's dmabuf protocol.
- Chromium's headless mode does not expose dmabuf FDs through CDP. This would require patching Chromium or writing a custom build with added hooks.
- Even with vanilla Chromium, the `--disable-gpu` path writes to system memory; extracting those pointers safely requires accessing Chromium internals.

**Verdict:** Architecturally ideal but practically impossible without modifying Chromium upstream or maintaining a fork. Skip for initial implementation; consider only after Option 1 proves successful and users complain about CPU usage at 60fps.

---

## 3. Comparison Summary

| Criterion | Option 1: CDP + Screenshot | Option 4: CEF (cef-rs) | Option 2: Servo | Option 6: Shared Mem |
|---|---|---|---|---|
| **Runtime dependency** | System Chromium | CEF redistributable (~250 MB) | Compiled inline (~150 MB) | System Chromium |
| **Binary size added** | Minimal (~200 KB client) | Moderate (+250 MB CEF) | Moderate (+linkage) | Minimal |
| **License compat** | ✅ BSD | ✅ BSD-3 + Chromium notices | ✅ MPL-2.0 | ✅ BSD |
| **Feature completeness** | Full Blink engine | Full Blink engine | ❌ Poor JS/Audio | Full Blink engine |
| **CPU (idle anim.)** | Medium-High (decode overhead) | Low-Medium (native) | Low (but broken pages) | Very Low |
| **RAM per instance** | 200–400 MB | 300–500 MB | 50–100 MB | 200–400 MB |
| **Audio support** | ⚠️ PipeWire loopback only | ✅ Native CEF audio | ❌ None | ✅ Native CEF audio |
| **Security** | ✅ Sandboxed renderer | ✅ Sandboxed renderer | ⚠️ Single-process risk | ✅ Sandboxed renderer |
| **Implementation effort** | 5–7 weeks | 5–6 weeks | 3–6 months (uncertain) | 4–8 months (requires patching) |
| **Maintenance burden** | Medium (CDP evolves) | Medium-high (CEF lags Chromium) | High (unstable API) | Very High |
| **Wayland native** | ⚠️ Runs headless, composes manually | ⚠️ XWayland mostly | N/A | Needs investigation |
| **Multi-monitor** | Possible (multiple tabs/instances) | Possible (multiple contexts) | Unknown | Possible |

---

## 4. Recommendation

### Primary recommendation: Option 1 — Headless Chromium via CDP (initial implementation)

**Rationale:**

1. **Fastest path to working functionality.** A proof-of-concept can demonstrate the web-wallpaper feature within 1–2 weeks, allowing early user feedback before committing to heavier infrastructure.
2. **System-chromium-as-a-dependency is acceptable.** Like mpv and yt-dlp, Chromium is expected to be present on desktop Linux systems. Ferramari does not bundle it; users install it alongside the rest of their desktop stack. This matches how Firefox extensions, GNOME Shell integrations, and similar tools operate.
3. **CDP is stable.** Chrome/Chromium maintain backward compatibility in the DevTools Protocol for years. The APIs Ferramari needs (`Page.navigate`, `Page.captureScreenshot`, `Input.dispatchMouseEvent`, `Network.requestIntercept`) have existed unchanged since CDP was stabilized around Chrome 70 (2018).
4. **Audio is solvable.** PipeWire has `pactl` sink-input loopback mechanism: redirect a specific Chromium sink to Ferramari's monitor source. Not elegant, but functional. Alternatively, use `pipewire`'s node enumeration to find and remap Chromium's audio output automatically.
5. **Resource budget is known.** 300 MB RAM and 5–10% CPU per web wallpaper is measurable and predictable. Users can be informed in the UI ("Web wallpapers use additional resources").

### Secondary recommendation: Option 4 — CEF transition (long-term improvement)

If and when the CDP approach proves popular and users report CPU bottlenecks or want better audio fidelity, migrating to CEF (via `cef-rs`) is the logical next step. It eliminates the screenshot polling entirely by giving us a direct surface handle. The migration would look like:

```
ferramari-platform/src/web_cdp.rs   →  ferramari-platform/src/web_cef.rs
(ceps-based polling, PNG decode)       (direct wl_surface/XID from CEF)
```

Both paths produce the same `WallpaperSurface` trait object; the media layer doesn't care which produced the handle.

### What NOT to do

- **Do not embed Servo.** Incomplete JavaScript and zero audio support make it unusable for web wallpapers, regardless of how clean the integration would be.
- **Do not attempt WebRender/Gecko embedding.** There is no supported path to embed Firefox's rendering engine outside of mobile platforms. This would require maintaining a gecko-dev fork.
- **Do not patch Chromium for dmabuf exports.** The engineering effort is disproportionate and the maintenance burden (tracking Chromium internal API changes) is unsustainable for a small team.

### Implementation phases

| Phase | Scope | Timeline | Gate criteria |
|---|---|---|---|
| **Phase 1** | CDP POC: launch headless Chromium, load URL, poll screenshots, display on wallpaper surface. No audio. | 1–2 weeks | Demonstrates a web page rendered as wallpaper on a test URL |
| **Phase 2** | Full integration: connect to Engine lifecycle (play/pause/fade), add crash recovery/restart policy, handle monitor hotplug (destroy surface on monitor disconnect, recreate on reconnect). | 2–3 weeks | Survives a monitor replug cycle without hanging Chromium |
| **Phase 3** | Audio: PipeWire loopback or `pactl` sink routing. Allow user to toggle audio per-monitory wallpage. | 1–2 weeks | Audio plays from web wallpaper and mutes/unmutes with the global volume slider |
| **Phase 4** | Security hardening: user-data-dir isolation, network request filtering, resource limits, mouse event forwarding. | 1–2 weeks | Untrusted pages cannot access filesystem; can be disabled via config |
| **Phase 5** | UX polish: loading indicator, error page when page fails to load, ability to inspect devtools for debugging (expose `--remote-debugging-port` to developer settings). | 1 week | Developer can attach Chrome DevTools to a running web wallpaper for debugging |

### Configuration model (proposed)

```json
{
  "web": {
    "enabled": true,
    "browser_binary": "/usr/bin/google-chrome",
    "default_url": "https://example.com/wallpaper",
    "audio_enabled": true,
    "network_filter": {
      "allowed_origins": ["example.com", "cdn.example.com"],
      "blocked_origins": []
    },
    "resource_limits": {
      "max_memory_mb": 512,
      "max_cpu_percent": 15
    }
  }
}
```

### Migration TODO updates

When this work is completed, these items in `MIGRATION_TODO.md` should be updated:

- §7 item "Select a GTK-free browser engine or isolated browser process for web wallpapers" → Checked.
- §7 item "Define a strict web-wallpaper sandbox, network permissions and process limits" → Updated to reflect implemented security model.
- §7 item "Restore web wallpaper only after navigation, audio, reload and cleanup tests pass" → Add test matrix reference.
- Add a new entry to §10: "Web wallpaper packaging notes" documenting the Chromium dependency and Flathub/Snap requirements.

---

## 5. References

- **Chrome DevTools Protocol**: https://chromedevtools.github.io/devtools-protocol/tot/Page/
- **CEF documentation**: https://bitbucket.org/chromiumembedded/cef/wiki/Home
- **cef-rs bindings**: https://github.com/xcmake/cef-rs
- **Servo README/embedding guide**: https://github.com/servo/servo
- **PipeWire audio redirection**: https://wiki.archlinux.org/title/PipeWire#Redirecting_audio_between_sinks
- **Headless Chrome documentation**: https://developer.chrome.com/docs/chromium/headless-shell
- **MPV_LIFECYCLE_AUDIT.md** — existing audit that the web engine would follow for process management patterns
