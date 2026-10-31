# Flatpak

`io.github.ferramari.Ferramari.yml` builds a **self-hosted** Flatpak bundle.
It is not a Flathub submission yet.

## Requirements

- `org.gnome.Sdk//50` and `org.freedesktop.Sdk.Extension.rust-stable`
- `org.flatpak.Builder` (or `flatpak-builder`)
- Host `mpv`, `ffmpeg`, `ffprobe` and (for streaming) `yt-dlp`

The wrappers in `flatpak/bin/` call the host tools through `flatpak-spawn`.
This keeps the bundle small while the application is still built against the
GNOME runtime and WebKitGTK. A future Flathub manifest should bundle these
tools and use vendored Cargo sources instead of network access.

## Build and install

```sh
flatpak install --user flathub org.gnome.Sdk//50
flatpak install --user flathub org.freedesktop.Sdk.Extension.rust-stable
flatpak install --user flathub org.flatpak.Builder

flatpak run org.flatpak.Builder --user --install --force-clean \
  --repo=repo build-dir flatpak/io.github.ferramari.Ferramari.yml

flatpak run io.github.ferramari.Ferramari
```

On a host where `appstreamcli-compose` is missing, skip the final export step
with `--build-only`, then finalize/export manually:

```sh
flatpak-builder --user --build-only --state-dir=state build flatpak/io.github.ferramari.Ferramari.yml
flatpak build-finish --command=ferramari --share=ipc --share=network \
  --socket=x11 --socket=pulseaudio --device=dri --filesystem=xdg-videos \
  --talk-name=org.freedesktop.Flatpak --talk-name=org.freedesktop.ScreenSaver \
  --talk-name=org.gnome.ScreenSaver --talk-name=org.freedesktop.login1 \
  --talk-name=org.kde.StatusNotifierWatcher --talk-name=org.freedesktop.portal.Desktop \
  build
flatpak build-export repo build stable
flatpak build-bundle repo ferramari.flatpak io.github.ferramari.Ferramari stable
```

Run `docs/TESTING.md` inside the sandbox before calling the Flatpak build
tested. The manifest deliberately requests `--filesystem=xdg-videos` because
the video directory is localised; it does not request the whole home folder.
The StatusNotifierItem and portal autostart paths still need verification in
the sandbox.
