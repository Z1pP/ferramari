# Ferramari 🦀

Video wallpaper for Linux, written in Rust.

<img src="data/icons/hicolor/scalable/apps/io.github.ferramari.Ferramari.svg" width="128" height="128" alt="Ferramari icon: a crab holding the sun">

Ferramari makes video and web content your desktop wallpaper. The name nods to
iron (*ferrum*) and Rust's crab mascot.

## Features

- Play a local video as the wallpaper, one video per monitor
- Browse a catalogue of live wallpapers with categories and search, and
  preview six frames before installing
- Streaming URLs through `yt-dlp` (YouTube and many other sites)
- Any web page as the wallpaper (WebKitGTK, so WebGL/Unity/Godot work)
- GNOME static wallpaper with a blurred frame from the video
- Pause or mute playback when a window is maximized or fullscreen (X11)
- Pause while the screen is locked; clean shutdown on logout/reboot
- Volume control with fade in/out, mute toggle
- Autostart after login
- “I'm Feeling Lucky” random video
- Tray icon (StatusNotifierItem) for GNOME/KDE and friends
- Config format schema version 4, including the 3.x → 4 migration
- D-Bus API for scripting and the tray/CLI

## Requirements

### Runtime

| Dependency | Why |
| --- | --- |
| GTK 3 + WebKitGTK 4.1 (>= 2.40) | windows and the web-page wallpaper |
| `mpv` | video playback (embedded through `--wid` and controlled via JSON IPC) |
| `ffmpeg` / `ffprobe` | static wallpaper frame, thumbnails |
| `yt-dlp` | only for streaming mode; use a current release (distro packages are often too old for YouTube) |

An X11 session is used directly; on Wayland the app runs through XWayland
(`GDK_BACKEND=x11` is set automatically).

### Build

- Rust 1.75+ (a `rust-toolchain.toml` pins stable)
- `pkg-config`, a C compiler
- GTK 3 development headers
- WebKitGTK 4.1 development headers (>= 2.40)

Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev
```

Fedora:

```sh
sudo dnf install gcc pkgconf-pkg-config gtk3-devel webkit2gtk4.1-devel
```

Arch Linux:

```sh
sudo pacman -S base-devel pkgconf gtk3 webkit2gtk-4.1
```

## Build, test, install

```sh
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Install (binary, desktop entry, AppStream metadata, icon, translations):

```sh
# build as your user, then only install as root:
cargo build --release
sudo make install CARGO=true            # PREFIX=/usr/local by default
sudo make install CARGO=true PREFIX=/usr

# per-user, no root:
make install PREFIX="$HOME/.local"

# staged install
make install DESTDIR=/tmp/package PREFIX=/usr
```

`CARGO=true` stops the install target from invoking cargo as root (root often
cannot see the rustup toolchain in your home directory).

Uninstall with `sudo make uninstall` (same `PREFIX`).

## Usage

```sh
ferramari             # start the wallpaper server and open the control window
ferramari -b          # background: live wallpaper only
ferramari -b -p 5     # wait 5 seconds before starting (autostart delay)
ferramari -d          # debug logging
ferramari -r          # reset the configuration
```

A second `ferramari` invocation does not start another server: it asks the
running instance to show its control window and exits.

Local videos are read from `$XDG_VIDEOS_DIR/Ferramari` (usually
`~/Videos/Ferramari`). The control window can open that folder, refresh the
thumbnail grid, and apply a video to one monitor or all of them.

### Wallpaper catalogue

The **Wallpapers** page lists live wallpapers: pick a category, search, or load
more pages. Opening a card shows the still preview; after you press
**Download** it fetches the file, reads the metadata (size, resolution, frame
rate, duration) and builds a 3×2 sheet of frames, so you can inspect the motion
before deciding. **Install…** puts the video into
`$XDG_VIDEOS_DIR/Ferramari` and opens the same monitor menu as a local video.
**Apply…** reuses the installed file.

Downloads only start when you ask for them. Previews, archives and extracted
videos are cached in `$XDG_CACHE_HOME/ferramari/catalogue` (default
`~/.cache/ferramari/catalogue`); cached videos older than seven days or beyond
the 2 GB budget are removed on start, and **Remove download** deletes one
entry.

### D-Bus

The server owns `io.github.ferramari.Ferramari.server` and exports
`io.github.ferramari.ferramari.server` at
`/io/github/ferramari/ferramari/server`. The player interface
(`io.github.ferramari.ferramari.player`) mirrors it.

```sh
# switch to a video
gdbus call --session --dest io.github.ferramari.Ferramari.server \
  --object-path /io/github/ferramari/ferramari/server \
  --method io.github.ferramari.ferramari.server.video /path/to/video.mp4 ""

# set the volume (0-100)
gdbus call --session --dest io.github.ferramari.Ferramari.server \
  --object-path /io/github/ferramari/ferramari/server \
  --method org.freedesktop.DBus.Properties.Set \
  io.github.ferramari.ferramari.server volume '<int32 30>'

# quit
gdbus call --session --dest io.github.ferramari.Ferramari.server \
  --object-path /io/github/ferramari/ferramari/server \
  --method io.github.ferramari.ferramari.server.quit
```

### Configuration

`$XDG_CONFIG_HOME/ferramari/config.json` (default
`~/.config/ferramari/config.json`). The format uses schema version 4; unknown
keys are ignored, missing keys get defaults, and a schema-3 file is migrated on
first start.

## Architecture

One process, one GTK main loop, and a single command channel:

```
GUI / tray / D-Bus / X11 watcher / screen-lock watcher
                      │  Command
                      ▼
                 ┌─────────┐
                 │   App   │  (GTK main thread, owns all widgets)
                 └────┬────┘
      ┌───────────────┼────────────────┐
      ▼               ▼                ▼
WallpaperWindow   Config/D-Bus    GNOME background
 (mpv / WebKit)    snapshot       (ffmpeg + gsettings)
```

| Path | Responsibility |
| --- | --- |
| `src/app.rs` | application state, command handling, mode switching |
| `src/config.rs` | schema-v4 config, validation, 3.x migration |
| `src/mpv.rs` | mpv process + JSON IPC (volume, mute, pause) |
| `src/ui/control.rs` | control window (videos, streaming, web page, toggles) |
| `src/ui/online.rs` | wallpaper catalogue and preview/install dialog |
| `src/catalogue.rs` | catalogue client: listing/search parsing, cache, download, frames |
| `src/ui/wallpaper.rs` | one desktop-type window per monitor |
| `src/x11.rs` | EWMH polling for maximized/fullscreen windows (replaces libwnck) |
| `src/dbus.rs` | zbus server/player interfaces, single-instance logic |
| `src/tray.rs` | StatusNotifierItem tray (`ksni`) |
| `src/wallpaper.rs` | blurred static wallpaper, original-background restore |
| `src/handlers.rs` | screen-lock and end-session signals |
| `src/ytdlp.rs` | `yt-dlp -J` parsing and format selection (unit-tested) |

Threading rule: GTK widgets never leave the main thread. Background work
(yt-dlp, HTTP checks, thumbnails, X11 polling, D-Bus, tray) sends commands or
messages into the main loop.

The control window's Liquid Glass design system (tokens, material layers,
typography, motion and accessibility rules) is documented in
[`docs/DESIGN.md`](docs/DESIGN.md). The stylesheet lives in
`data/ui/ferramari.css` and is compiled into the binary.

## Known limitations

- The app always runs on X11/XWayland (`GDK_BACKEND=x11`): a wallpaper window is
  a `DESKTOP`-type X11 toplevel and mpv embeds into it with `--wid`. Native
  Wayland has no background role in `xdg-shell`, and `wlr-layer-shell` is not
  available on GNOME, so native Wayland windows are not an option.
- Pause/mute when maximised needs `_NET_CLIENT_LIST`, which only lists X11
  windows. On Wayland sessions the feature is therefore unavailable and its
  controls are hidden in the quick settings.
- Fractional scaling and mixed-DPI layouts are upscaled by XWayland; expect
  softer output than a native Wayland client.
- KDE Plasma is not part of the tested matrix; the XWayland desktop window is
  the only supported path there.

## Implementation notes

- GTK 3 is kept on purpose: WebKitGTK 4.1 is the only WebKit line with a
  maintained Rust binding, and the web-page wallpaper needs it. A GTK4 port is
  possible once a `webkit6` binding appears.
- Streaming needs a recent `yt-dlp`. The Ubuntu package can be months behind
  and then returns YouTube URLs that mpv cannot open (HTTP 403). The official
  binary is installed next to `ferramari` and is preferred automatically:

  ```sh
  sudo curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux \
    -o /usr/local/bin/yt-dlp
  sudo chmod +x /usr/local/bin/yt-dlp
  ```

- `mpv` is embedded with `--wid`, uses `--panscan=1.0` for the center-crop
  effect and is driven through its JSON IPC socket.
- Everything runs in one process; the D-Bus method and property shape stays
  stable for scripts and desktop integrations.
- Window state detection uses EWMH atoms over X11.
- `-b` starts only the wallpaper, `-p` adds a startup delay, and `-r` resets the
  config (only when no instance is running).
- No Flatpak manifest is shipped in this tree yet; the code keeps the Flatpak
  code paths (`flatpak-spawn`, background portal, WebKit NVIDIA workaround).

## Testing notes

`cargo test` covers config parsing/migration, yt-dlp format selection and
process timeouts, EWMH decisions, mpv IPC framing, monitor naming, the fade
maths, path handling and the catalogue parsers (cards, categories, search,
signed download links, archive extraction, video probing and frame sheets).
The GUI, video embedding, streaming, webpage and catalogue modes were
smoke-tested on Ubuntu 26.04 / GNOME Wayland (XWayland). To exercise the
static-wallpaper code without touching your real background:

```sh
GSETTINGS_BACKEND=memory ferramari -b -d
```

## License

GPL-3.0-or-later. See `LICENSE`.