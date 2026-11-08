# Flatpak

`io.github.ferramari.Ferramari.yml` builds a **self-hosted** Flatpak bundle from
the 2.0 workspace: the wallpaper server and the Iced control surface are both
installed, and neither links GTK, GDK or WebKitGTK.

This is not a Flathub submission. The manifest has been validated end-to-end
with `flatpak-builder --build-only`: both binaries are present in the bundle,
neither links GTK, GDK or WebKitGTK, and all desktop metadata and translations
are installed. What has not been done yet is a full `--install` + sandbox launch
(smoke-test of `ferramari --doctor`, control surface reaching the server, tray)
and the export step that would produce a standalone `.flatpak` file for
sideloading. Until those are recorded, treat the Flatpak path as
**built but not exercised inside a running sandbox**.

## What the bundle contains

| Path | Source |
| --- | --- |
| `/app/bin/ferramari` | server: surfaces, players, config, D-Bus API |
| `/app/bin/ferramari-ui` | Iced control surface, a D-Bus client of the server |
| `/app/bin/mpv`, `ffmpeg`, `ffprobe`, `yt-dlp`, `xdg-open`, `xdg-user-dir` | wrappers that run the host tool through `flatpak-spawn` |
| `/app/share/applications`, `metainfo`, `icons`, `locale` | desktop integration and translations |

The server resolves the control surface next to its own executable first and
falls back to `PATH`, so the two binaries must be installed side by side; the
manifest fails the build if the second one is missing.

## Two ways to be wrong about media tools

The wrappers keep the bundle thin by running the host's `mpv`/`ffmpeg`/`yt-dlp`
through `flatpak-spawn --host`, which is why `org.freedesktop.Flatpak` is
granted. That is the trade-off this repository made before 2.0 and it has two
consequences worth stating:

- the sandbox needs `org.freedesktop.Flatpak`, a broad permission;
- hardware decoding depends on the host driver stack being reachable the way
  the host expects, not the bundle's.

A Flathub submission should bundle mpv, ffmpeg and yt-dlp as modules with
vendored Cargo sources, drop `org.freedesktop.Flatpak`, and then re-run the
`--doctor` checks inside the sandbox before being called equivalent.

## Build and install

```sh
# Note: `org.freedesktop.Sdk.Extension.rust-stable//50` does not exist; the
# SDK pins its own extension versions internally and only accepts what it
# ships. Asking for a newer minor breaks the build. flatpak-builder also needs
# FUSE — if `rofiles-fuse` is unavailable the builder falls back to a tmpfs
# cache but some operations fail (see below). If you need a working sandbox,
# install FUSE or use a CI runner.
flatpak install --user flathub org.gnome.Platform//50
flatpak install --user flathub org.gnome.Sdk//50
flatpak install --user flathub org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak install --user flathub org.flatpak.Builder

flatpak run org.flatpak.Builder --user --install --force-clean \
  --repo=repo build-dir flatpak/io.github.ferramari.Ferramari.yml

flatpak run io.github.ferramari.Ferramari
```

## Checks to run before calling it tested

```sh
# Both binaries exist and neither links a toolkit.
flatpak run --command=sh io.github.ferramari.Ferramari -c \
  'ls -l /app/bin/ferramari /app/bin/ferramari-ui
   ldd /app/bin/ferramari /app/bin/ferramari-ui | grep -Ei "gtk|gdk|webkit" && echo BAD || echo GTK-free'

# Session/backend/capability report from inside the sandbox.
flatpak run io.github.ferramari.Ferramari --doctor

# The control surface reaches the server over the session bus.
flatpak run io.github.ferramari.Ferramari &
flatpak run --command=ferramari-ui io.github.ferramari.Ferramari
```

`--doctor` is the honest report: under a Wayland session it says
`wallpaper=partial` because the surface is XWayland, and a capability the
sandbox blocks shows up as `unsupported` with the reason next to it. Anything
that used to be a guess is now a stated limitation.

Then follow `docs/TESTING.md` inside the sandbox: playback on the desktop
layer, pause under a fullscreen window, lock/unlock, suspend/resume and a
tray action. `--filesystem=xdg-videos` is deliberate — the Videos directory is
localised, so its path cannot be hardcoded — and the whole home folder is not
requested.

## Known sandbox limitations

- **Tray.** A sandbox may not own names outside its own id, so the SNI item is
  served on the connection's unique name instead of
  `org.kde.StatusNotifierItem-<pid>-<n>`. That is what ksni recommends for
  sandboxes and the same route Chromium took, but not every host honours it;
  the tray not appearing inside Flatpak is a known open item, not a crash.
- **Autostart** goes through the Background portal, and portal autostart inside
  a sandbox has not been verified here.
- **Host tools.** `mpv` and `ffmpeg` must exist on the host with the codecs the
  wallpaper needs; `--doctor` prints what it found.
- **Native Wayland** is not implemented, so the bundle asks for `--socket=x11`
  rather than a Wayland fallback, and on a Wayland session the wallpaper is an
  XWayland surface.
