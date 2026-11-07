# Contributing

Ferramari is a Rust desktop application with an Iced control surface and a separate wallpaper server. Keep changes small and
measurable; a feature that cannot be tested should be behind a clear
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
cargo fmt --all -- --check
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

`flatpak/io.github.ferramari.Ferramari.yml` is a self-hosted manifest. It uses
the GNOME runtime, the Rust SDK extension and host media tools through
`flatpak-spawn`. See `flatpak/README.md`. A Flathub submission needs bundled
media tools and vendored Cargo sources; do not describe the current bundle as
Flathub-ready.

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