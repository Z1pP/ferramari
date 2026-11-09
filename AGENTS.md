# Agent Operating Rules for Ferramari

These rules govern autonomous work on this repository. The agent (this AI) is expected to follow them without interruption or confirmation requests.

## 1. Autonomy

- Work independently until the task is fully complete. Do not ask permission for normal technical decisions (naming, module structure, error-handling approach, test strategy).
- Do not output "Continue?", "Shall I proceed?", or similar checkpoint questions.
- When multiple valid approaches exist, pick the safest / most conservative one and document the rationale briefly in a commit message or code comment — never in a message to the user.
- Do not stop after each phase for approval. Report only at natural boundaries: end of a coherent feature, bugfix set, or refactoring unit.
- No intermediate status reports unless something blocks progress or requires a trade-off decision between roughly equal-cost options.

## 2. Independent Development

- Always read existing code before making changes. Respect current naming, module layout, and architectural conventions.
- Plan the implementation mentally; do not produce half-finished scaffolding then wait for instructions.
- Fix bugs you encounter during implementation. Do not silence compilation errors with `#[allow(...)]`, `unimplemented!()`, `todo!()`, or `dbg!()` left in release code.
- Refactor when it makes the result clearer, safer, or easier to test. A superficial change that works but leaves cruft is considered incomplete.
- Temporary stubs are forbidden. If a piece cannot be completed because an external dependency (runtime tool, hardware) is missing, emit a clear, documented limitation and move on — do not mask it as working.

## 3. Testing & Quality Gates

### Build & lint (mandatory every time)

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

All four must pass before declaring any work done. Run them in this order: format → clippy → test → release build. If any step fails, fix it and rerun from format.

### Additional checks (when relevant)

| Area | Command(s) |
| --- | --- |
| Translation catalogs | `for po in po/*.po; do msgfmt --check -c -o /dev/null "$po"; done` |
| Desktop metadata | `appstreamcli validate --no-net data/io.github.ferramari.Ferramari.metainfo.xml`<br>`desktop-file-validate data/io.github.ferramari.Ferramari.desktop` |
| GTK-free assertion | `cargo tree --workspace --locked --all-features -e normal \| grep -E '(^| )(gtk|gdk|webkit2gtk|gdk-pixbuf|glib|gio|pango|cairo-rs) v'` (must find nothing) |

### Runtime verification

- For UI changes: run `ferramari-ui` (the separate binary), take screenshots with `max_size` ≤ 1280, inspect visually, fix issues, screenshot again. Repeat until the page looks correct in both light and dark themes.
- For media/wallpaper changes: use `GSETTINGS_BACKEND=memory ferramari -d` in an isolated `dbus-run-session` to avoid disturbing the host wallpaper. Verify mpv starts, logs show correct engine flags, and the surface appears.
- Never rely solely on successful compilation. The CI explicitly checks that *both* `target/release/ferramari` and `target/release/ferramari-ui` exist and link no GTK/GDK.

### Test philosophy

- The existing test suite (`cargo test --workspace`) covers config parsing/migration, Eco Engine policy, mpv IPC framing, yt-dlp timeouts, `.ferramari` import/export, catalogue parsing, fade maths, path handling, EWMH coverage, process-tree metric parsing, and more. Add tests for new public-facing logic whenever feasible.
- Tests that require a display server, X11 session, or real mpv/mpv JSON IPC belong in the smoke checklist (`docs/TESTING.md`) and must be verified live, not headlessly. Do not mark a smoke item checked without actual execution on the target environment.
- Pure-policy modules (eco.rs, config.rs, package.rs) should have property-based or exhaustive unit tests covering edge cases.

## 4. UI/UX Design

### Design system

The Iced control surface uses the Liquid Glass design language (tokens, material layers, typography, motion, accessibility). It is documented in `docs/DESIGN.md`. The CSS stylesheet lives in `data/ui/ferramari.css` (compiled into the binary). **Always** consult `docs/DESIGN.md` before changing colours, spacing, typography, or widget behaviour.

### Conventions

- Follow the existing palette defined in `crates/ferramari-ui/src/theme.rs` — do not introduce ad-hoc hex values.
- The app has Light, Dark, and System-default colour schemes. Every UI change must be verified in all three.
- Check minimum window width/height and reasonable wide layouts. Resize the window to verify no truncation or overflow.
- Translations (English, Russian, Japanese) affect string length. In Russian the text can be 30% wider than English; in Japanese similarly. Ensure no labels or buttons are cut off at any supported locale width.
- Use `tr("...")` for all user-visible strings. Counted nouns use `tr_n(...)`.
- The Iced event loop must never be blocked. Heavy work goes through Iced tasks / channels.

### Screenshots & references

Reference images are in `docs/screenshots/`. Before designing new UI elements, compare against these references and `docs/DESIGN.md`. Do not claim visual work is complete without taking and reviewing screenshots.

### Constraints

- The app is intentionally GTK-free. The UI runs purely on Iced. Any dependency that pulls in GTK/GDK/WebKitGTK breaks the contract and will fail CI's `cargo tree` check.
- File dialogs use `rfd` (XDG Desktop Portal on Linux). Do not add GTK-native file picker bindings.
- Native Wayland support is not implemented. On Wayland sessions the app runs through XWayland. Accept this as a given limitation; do not attempt to implement native Wayland surfaces until the backend contract (`docs/BACKEND_CONTRACT.md`) is updated.

## 5. Decision-Making

### Sources of truth (in priority order)

1. This file (`AGENTS.md`).
2. `CONTRIBUTING.md` — contributor guidelines and pre-PR checklist.
3. `README.md` — architecture overview, feature list, known limitations.
4. `ROADMAP.md` — planned direction.
5. Code comments and crate docstrings.
6. Documents in `docs/`.
7. Existing code patterns (inductive reasoning).

### When information is missing

- Read surrounding code and related crates to infer the intended pattern.
- Prefer the existing stack: Iced + mpv + ffmpeg + x11rb. Avoid adding a new runtime dependency without a concrete, justifiable reason.
- When uncertain about a choice, pick the option that is easiest to reverse and least invasive.

### Blocking

- If one sub-task is blocked (e.g., waiting for a runtime tool like mpv or yt-dlp), continue working on independent parts (tests, pure logic, UI polish, documentation) rather than idle.

### Architectural records

Record important decisions in-line via `// NOTE:` comments near the relevant code, or in a short `docs/` addition if the decision affects cross-crate boundaries, public API shape, or migration strategy. See also `docs/AUDIT.md` for open findings and `docs/MIGRATION_TODO.md` for migration tracking items.

## 6. Safety & Restraint

### Absolute prohibitions (require explicit user consent)

- `git push` to any remote.
- `git push --force` / `git reset --hard`.
- Deleting user data files (`config.json`, `library.json`, signing keys, video files, cache).
- Overwriting someone else's uncommitted changes.
- Disabling safety checks (`clippy`, format guards, Cargo lock).
- Modifying `Cargo.lock` directly by hand.

### Safe by default

- Local branches and worktrees: safe.
- Committing local changes: safe (unless instructed otherwise).
- `cargo clean`: safe, but only at task boundaries.
- Running the application in an isolated D-Bus session (`dbus-run-session`): safe and preferred for smoke tests.

### Git hygiene

- Use descriptive branch names following the pattern `work/<topic>`.
- Small, focused commits with messages that explain *why*, not just *what*.
- Never amend history on shared branches.
- Rebase only on unpushed, private work.

## 7. Completion Criteria

Declare a task done **only** when every acceptance criterion listed for it is satisfied. For undefined tasks, use these defaults:

### Definition of Done

| Criterion | How verified |
| --- | --- |
| Compiles (`debug` and `release`) | `cargo build --workspace --release --locked` succeeds |
| Both binaries present | `test -x target/release/ferramari && test -x target/release/ferramari-ui` |
| Clippy clean | `cargo clippy --workspace --all-targets --locked -- -D warnings` — zero warnings |
| Format clean | `cargo fmt --all` reports no diffs |
| Tests pass | `cargo test --workspace --locked` — all green |
| GTK-free | `cargo tree ... -e normal` finds no GTK-family deps |
| Functional correctness | Manual smoke test or unit test proves the logic works |
| UI correct (if applicable) | Screenshots in light/dark theme, resized windows, no truncation |
| Docs updated | CHANGELOG entry, README notes, or docs/ additions reflect the change |
| No regressions | Previous smoke checklist items still pass |

### Final report format

At the end of each significant task, provide a single consolidated report containing:

1. **What was implemented** — brief description of changes.
2. **Files changed** — paths and purpose of each modification.
3. **Decisions made** — key trade-offs or assumptions with rationale.
4. **Build results** — output summary of `cargo build --workspace --release --locked`.
5. **Test results** — output summary of `cargo test --workspace --locked` (pass count, failed tests).
6. **Clippy results** — whether the run is clean or any suppressed warnings remain.
7. **Visual results** (UI tasks) — description of what was checked and screenshots path.
8. **What remains incomplete** — open items with reasons (missing hardware, blocked dependency, planned follow-up).

## 8. Continuous Improvement of These Instructions

If repeated problems, confusion, or gaps emerge during work:

- Fix obvious typos, stale paths, or contradictory statements immediately.
- For substantive rule additions (new patterns discovered in code, additional quality gates, new tools/checks), make the change and note it in the commit message.
- Keep the file scannable. Use tables and code blocks consistently. Remove obsolete entries when practices change.

---

## Project-Specific Quick Reference

### Workspace structure

```
crates/ferramari-core   – shared business logic, config, policy, data models (~12k LOC)
crates/ferramari-ui     – Iced control surface + D-Bus client (~3.3k LOC)
crates/ferramari-media  – mpv controller, X11 wallpaper surfaces, playback engine (~2.1k LOC)
crates/ferramari-platform – Linux integration: tray, D-Bus, monitors, autostart, power (~5k LOC)
src/main.rs             – root package: CLI entry point, server bootstrap (~0.9k LOC)
src/app.rs              – wallpaper-server state machine (~2k LOC)
```

### Two-binary ship requirement

The workspace produces two executables:
- `ferramari` (root package) — wallpaper server + CLI commands
- `ferramari-ui` (crate `ferramari-ui/bin`) — Iced control-surface GUI

A plain `cargo build` builds only the root package and silently omits `ferramari-ui`. **Always use `--workspace`**.

### Key files

| Path | Purpose |
| --- | --- |
| `crates/ferramari-core/src/bus.rs` | Command bus: serialises all state mutations through one channel |
| `crates/ferramari-core/src/config.rs` | Schema-v4 config loading, atomic writes, .bak backup recovery |
| `crates/ferramari-core/src/eco.rs` | Pure per-monitor Eco Engine policy (Active/Idle/Occluded/Suspended) |
| `crates/ferramari-core/src/package.rs` | `.ferramari` manifest validation, import/export |
| `crates/ferramari-core/src/catalogue.rs` | Catalogue listing, search, download cache, frame generation |
| `crates/ferramari-media/src/engine.rs` | Per-monitor wallpaper playback engine |
| `crates/ferramari-media/src/mpv.rs` | mpv JSON IPC driver |
| `crates/ferramari-platform/src/surface.rs` | Native X11 wallpaper windows via x11rb |
| `crates/ferramari-platform/src/dbus.rs` | zbus D-Bus server interfaces + single-instance ownership |
| `crates/ferramari-platform/src/monitors.rs` | XRandR discovery + hotplug watcher |
| `crates/ferramari-platform/src/tray.rs` | StatusNotifierItem tray (ksni) |
| `crates/ferramari-ui/src/main.rs` | Iced pages, widgets, app state (~2.5k LOC) |
| `crates/ferramari-ui/src/client.rs` | D-Bus proxy, ServerState snapshot |
| `crates/ferramari-ui/src/theme.rs` | Liquid Glass token definitions |
| `data/ui/ferramari.css` | Compiled-in stylesheet |
| `po/*.po` | gettext translation catalogs |
| `flatpak/io.github.ferramari.Ferramari.yml` | Self-hosted Flatpak manifest |
| `.github/workflows/ci.yml` | GitHub Actions quality gate |

### Runtime dependencies (external tools)

| Tool | Why needed |
| --- | --- |
| `mpv` | Video playback (`--wid` embedding, JSON IPC) |
| `ffmpeg` / `ffprobe` | Static wallpaper frame extraction, thumbnails |
| `yt-dlp` | Streaming URLs (YouTube and friends) |

All three must be present for full functionality. The build itself does not depend on them.

### Threading model

- **Iced UI thread**: owns the widget tree. No background goroutines, no GLib main loop. Heavy work via Iced tasks / channels.
- **Wallpaper-server command loop**: owns media engines on its own thread. State mutations flow through the command bus (`bus::Command`) serialised in one place.
- **Cross-process**: server ↔ UI communicate over session D-Bus (`io.github.ferramari.Ferramari.server`). The server owns the `session` D-Bus name for single-instance semantics.
- **Background workers**: network (ureq), ffmpeg probing, thumbnail generation, catalogue downloads — all return plain data via channels or tasks.

### External process safety

Every external command must have a deadline. Use `util::wait_with_timeout`, `util::output_with_timeout`, or `util::run_with_timeout`. Kill and reap timed-out processes. Never leave children orphaned.

### Configuration storage

Atomic writes via temp file + rename, with `.json.bak` backup copy. Crashes cannot corrupt config. Corrupt files auto-recover from backup on load. Same pattern for `library.json` and `display-ids.json`.

### Known architectural debt visible today

- Web-page wallpapers temporarily unavailable after GTK removal; browser engine replacement needed.
- Native Wayland wallpaper surface not implemented (X11/XWayland only).
- KDE Plasma not yet tested.
- Multi-monitor >2 displays not benchmarked.
- Flatpak sandbox not exercised end-to-end (build validated, runtime checks pending).
- Some legacy config identifiers (`performance`, `power`) kept for backwards compatibility — see `Profile::from_config()` / `Profile::config_value()` in `eco.rs`.
