# Contributing

Ferramari is a Rust desktop application with an Iced control surface and a
separate wallpaper server. Keep changes small and measurable; a feature that cannot be tested should be behind a clear
limitation, not presented as supported.

## Setup

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

The runtime needs `mpv`, `ffmpeg`/`ffprobe` and (for streaming) `yt-dlp`. Web-page wallpapers are temporarily unavailable. Build both application binaries with `cargo build --workspace --release --locked`.

The wallpaper server currently creates desktop surfaces through X11/XWayland; the Iced control surface is separate and communicates over D-Bus. Do not claim native Wayland support without implementing and testing a compositor-specific wallpaper surface first.

## Before opening a pull request

Run all of these:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

For translation changes also run:

```sh
for po in po/*.po; do msgfmt --check -c -o /dev/null "$po"; done
```

For metadata changes:

```sh
appstreamcli validate --no-net data/io.github.ferramari.Ferramari.metainfo.xml
desktop-file-validate data/io.github.ferramari.Ferramari.desktop
```

## Architecture rules

- The Iced UI owns its widget tree on its event-loop thread. Never move UI state or widget handles into background workers.
- The wallpaper server owns media engines on its command-loop thread. The UI and server communicate through the stable session D-Bus API.
- Background work (network, ffmpeg, thumbnails, catalogues) returns plain data through channels or Iced tasks; do not reintroduce a GLib main loop.
- External commands must have a deadline and must be reaped. Use
  `util::wait_with_timeout`, `util::output_with_timeout` or
  `util::run_with_timeout`.
- Configuration and library data are written atomically, with a `.bak` copy.
- Do not add a runtime dependency without a concrete reason. Prefer the existing Iced/mpv/ffmpeg stack and avoid GTK/GDK dependencies.

## Translations

Source strings use `tr("...")`; counted nouns use `tr_n(...)`. Catalogs live in
`po/*.po` and are compiled by `make install` (or `make translations`). Add the
language to `po/LINGUAS` only after the catalog is complete enough to be useful.
`msgfmt --check` must pass.

## Flatpak

The manifest in `flatpak/io.github.ferramari.Ferramari.yml` builds the 2.0
workspace and installs both binaries. `cargo build --workspace` is required,
not stylistic: the default members are the root package only, so a plain
`cargo build` produces the server and silently skips the UI binary — which
would only surface as a failed `show_gui` at runtime. The manifest therefore
checks for `/app/bin/ferramari-ui` and fails the build. Media tools come from
the host through `flatpak-spawn`, which is why `org.freedesktop.Flatpak` is
granted; a Flathub submission should bundle them instead and drop that
permission.

The manifest is not yet exercised end to end inside a sandbox. `flatpak/README.md`
lists the checks (both binaries present, GTK-free `ldd`, `--doctor`, control
surface reaching the server) that must be recorded before the Flatpak row in
`docs/TESTING.md` can say *tested*. Two known build-environment traps are
documented there: the SDK pins its extension versions (`org.gnome.Sdk//50`
accepts rust-stable `25.08`, not the newest), and `flatpak run org.flatpak.Builder`
sees its own `/tmp`, so state and build paths must live in the home directory.

## Performance

Use `docs/PERFORMANCE.md` and `docs/TESTING.md`. Measure before optimising:
record the scenario, build, session and method. Run performance work in an
isolated `dbus-run-session` so the host wallpaper is not disturbed.

## Pull request checklist

- [ ] The change has a test for its critical logic, or a documented reason why
      it cannot be tested headlessly.
- [ ] Neither the Iced event loop nor the wallpaper-server command loop is blocked.
- [ ] External processes have timeouts and are reaped.
- [ ] Existing configuration remains backward compatible.
- [ ] User-visible strings are in `tr()` and the catalogs still compile.
- [ ] Documentation and known limitations are updated.
- [ ] The full check list above passes.