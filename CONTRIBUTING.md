# Contributing

Ferramari is a GTK 3 / Rust desktop application. Keep changes small and
measurable; a feature that cannot be tested should be behind a clear
limitation, not presented as supported.

## Setup

```sh
cargo build
cargo test
```

The runtime needs GTK 3, WebKitGTK 4.1, `mpv`, `ffmpeg`/`ffprobe` and (for
streaming) `yt-dlp`. The release build is `cargo build --release`.

The application runs on X11/XWayland by design. It sets `GDK_BACKEND=x11`
before GTK starts; desktop-type windows and mpv `--wid` embedding are X11
features. Do not claim native Wayland support without implementing and testing
a wallpaper surface first.

## Before opening a pull request

Run all of these:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
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

- The GTK main loop owns every widget. Never move a `gtk::Widget`, `Rc` or
  `Rc<RefCell<_>>` containing widgets into a worker thread.
- Background work (network, ffmpeg, thumbnails, catalogues) communicates with
  the main loop through `std::sync::mpsc` and a GLib timeout.
- External commands must have a deadline and must be reaped. Use
  `util::wait_with_timeout`, `util::output_with_timeout` or
  `util::run_with_timeout`.
- Configuration and library data are written atomically, with a `.bak` copy.
- Do not add a runtime dependency without a concrete reason. Prefer the
  existing GTK/mpv/ffmpeg stack.

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
- [ ] The GTK main loop is not blocked.
- [ ] External processes have timeouts and are reaped.
- [ ] Existing configuration remains backward compatible.
- [ ] User-visible strings are in `tr()` and the catalogs still compile.
- [ ] Documentation and known limitations are updated.
- [ ] The full check list above passes.