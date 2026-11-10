# Ferramari UI — design system (Iced)

The control surface is an **Iced 0.14** application. There is no GTK, no Cairo
painter and no runtime CSS: every token is a Rust constant, every widget style
is a closure, and every shape a page uses is built by one shared module.

The historical GTK specification (1.9.x) is archived at
[`archive/DESIGN-gtk-1.9.md`](archive/DESIGN-gtk-1.9.md). `data/ui/ferramari.css`
survives as a **reference only**: nothing loads it, and `theme.rs` no longer
tracks it value for value.

## Direction

**Night glass.** The product is a video wallpaper app, so the interface is a
dark cinematic surface with a cold light source in the top corner, and the
warm amber accent is reserved for what is live: the play control, the selected
rail entry, the frame-rate tile.

- A gradient backdrop, not a flat fill, so translucent panes have something to
  be translucent *over*.
- One material vocabulary: card, inset row, pill, tile. A page composes those
  and never invents a fifth.
- Text over a wallpaper frame is a separate case from text over the app's own
  surface, and has its own tokens.
- Every control performs a real D-Bus call. Nothing is decorative, and no
  number is invented.

## Files

| File | Responsibility |
| --- | --- |
| `crates/ferramari-ui/src/theme.rs` | `Semantics` token set (DARK/LIGHT), the `fade` helper, Iced `Theme` builder, system dark-preference probe |
| `crates/ferramari-ui/src/style.rs` | Every style closure and shared primitive: backdrop, card, rail, pill, buttons, toggler, `Sparkline`, the icon table |
| `crates/ferramari-ui/src/main.rs` | The shell (title bar, rail, content) and the five pages, assembled from `style::*` |
| `crates/ferramari-ui/src/client.rs` | D-Bus snapshot the view renders from |
| `data/ui/ferramari.css` | **Reference only** — the GTK-era token sheet |

`main.rs` contains no colour literal and no radius arithmetic of its own. A new
shape is added to `style.rs` once, or a token is added to `Semantics` in both
palettes; a page that spells out `Color::from_rgb8` is a page that will drift.

## Colour tokens

All colours live in `Semantics`, with `DARK` and `LIGHT` constants and
`Semantics::resolve(mode, system_dark)` choosing between them. `dark` and
`light` win over the desktop preference; anything else follows it.
`ferramari_theme()` maps the same palette into Iced's built-in `Palette` so
`slider`, `text_input`, `scrollable` and `pick_list` match.

| Group | Tokens | Role |
| --- | --- | --- |
| Backdrop | `bg_glow` / `bg_mid` / `bg_deep` | The three stops of the diagonal wash `style::gradient` paints |
| Panes | `rail_bg`, `card_bg`, `row_bg`, `title_bg` | Rail, glass card, inset row, the app's own title bar |
| Edges | `card_edge`, `pill_bg`, `pill_edge`, `border`, `border_strong`, `divider` | Hairlines at three strengths |
| Text | `text`, `text_secondary`, `text_muted` | Three levels, on the app's own surfaces |
| Text on artwork | `on_artwork`, `on_artwork_soft`, `badge_artwork`, `live_artwork` | The same hierarchy, over a wallpaper frame |
| Accent | `accent`, `accent_text`, `accent_bg`, `accent_bg2`, `accent_bg_hover`, `accent_ink`, `on_accent`, `accent_soft`, `accent_border`, `accent_shadow` | Sunlight amber: actions, the selected rail entry, ink on amber |
| Glow | `glow_accent`, `brand_glow` | The blurred amber behind active and highlighted states |
| Data | `series_cpu`, `series_ram`, `series_gpu`, `series_fps`, `series_frame` | Sparkline strokes, one hue per series |
| Status | `success_tone`, `warning_tone`, `danger_tone`, `danger`, `danger_soft`, `focus` | Metric thresholds and inline errors |
| Material | `surface`, `surface_strong`, `surface_hover`, `surface_active`, `shadow`, `card_shadow` | Interaction fills and elevation |
| Overlays | `input_bg`, `input_bg_focus`, `popover_bg`, `tooltip_bg`, `tooltip_text`, `scrollbar` | Entries and popups |

Two invariants are enforced by a compile-time test
(`accent_tokens_are_opaque_and_overlay_tokens_are_translucent`): accent and
text tokens are opaque, `surface` and `border` are translucent. A surface that
becomes opaque stops reading as glass.

**Text over artwork is not themeable.** `on_artwork` and its companions hold
the same values in both palettes on purpose. The scrim under them is black with
alpha, so it darkens whatever the video is showing in either theme; using the
light theme's near-black `text` there puts near-black ink on a near-black
plate. This came from a screenshot, not from theory.

`Semantics::fade(color, alpha)` derives a translucent version of an opaque
token. Series fills and hover washes use it rather than carrying a second
constant, so a palette change cannot leave a fill behind its stroke.

## Material layers

1. **Backdrop** — `style::backdrop` paints `gradient(sem)`: a linear gradient
   at 115° through `bg_glow`, `bg_mid`, `bg_deep`, so the light arrives from
   the top-right corner the way the reference draws it. Iced has no
   `backdrop-filter`, so depth is translucent fills over a lit base, not a blur
   pass.
2. **Panes** — `card_style`: `card_bg` fill, a 1px `card_edge` hairline, one
   shadow (12px down, 34px blur). `rail_style` is the same material with a
   directional shadow. `row_style` is the inset used inside a card, so a list
   reads as rows cut into the glass rather than tiles stacked on it.
3. **Glow** — `glow_style` and `Kind::NavActive` use `glow_accent`: the accent
   blurred, not a border drawn in amber.
4. **Elevation** — one shadow per pane. Hover changes a fill; it does not move
   a widget.

## Icons

`style::glyph(name)` maps a design name to a **Font Awesome 4.7** codepoint and
`style::icon` binds the family in one place, because the codepoints live in the
Private Use Area: with the default family, fontconfig falls back through DejaVu,
which has a house and a cog but not a filmstrip, and a rail reads as half
icons, half tofu.

The font is a system font, not a bundled asset. Consequences:

- A name missing from the table renders as the fallback glyph, silently.
- A codepoint from Font Awesome 5 renders as *some other glyph*, silently.

`scripts/glyph-check.py` reads the installed font's own charset and reports
every name the UI references that the font cannot draw, so both cases are a
failing check rather than a screenshot hunt. Run it after touching `glyph`.

## Typography

Sizes are passed per `text()` call. The scale in use:

| Role | Size |
| --- | --- |
| Page title | 26 |
| Brand wordmark, card title over artwork | 16–17 |
| Metric value, frame-time headline | 18–22 |
| Section heading | 15 |
| Rail label, settings row title | 13–13.5 |
| Hints, subtitles, list titles | 11.5–12.5 |
| Meta lines, chips, axis labels | 10.5–11 |

Long words break: file names are not words, so card titles and paths use
`text::Wrapping::WordOrGlyph`. The default `Word` lets
`untitled_Motion_2026-03-15.mp4` run past the card edge unbroken.

## Spacing and shape

- Rhythm: 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24 px. Page gutter 14 px, gap
  between rail and content 14 px, between sections 16 px.
- Radii: `RADIUS` 22 px for the rail, header and cards; `RADIUS_INNER` 14 px
  for inset rows and tiles; 999 px for pills, chips and the toggler.
- Card padding: 20 px for sections, 12 px for media cards, 14 px horizontal
  inside an inset row.
- `style::RAIL_WIDTH` 244 px, `style::SECTION_WIDTH` 224 px, default window
  1180 × 780, minimum 900 × 620.

The two widths are measured, not picked: `Производительность` is the widest
label either column ever holds, and the number is the one that keeps it on one
line. Both columns also wrap their labels, so a translation longer than the
shipped ones grows the row to two lines instead of clipping — the width is a
comfort target, not the thing that prevents a cut-off.

## Components

| Builder | Produces | Used by |
| --- | --- | --- |
| `card()` / `card_fit()` | Glass pane, filled or hugging | Every section |
| `rail_style()` / `backdrop()` | The two fixed surfaces | Shell |
| `row_style()` + `inset()` | A settings row cut into a card | Settings |
| `pill_style()` + `chip()` / `accent_chip()` | A fact, not a control | Mode, Eco state, category |
| `status_pill()` | Dot + label + sub-line | Engine badge |
| `dot_pill` / `dot()` | A status dot | Rail footer, badges |
| `primary()` / `primary_icon()` | Filled amber action | Start, Add, Install |
| `secondary()` / `secondary_icon()` | Outlined glass action | Stop, Restart, Open folder |
| `icon_button()` | Round icon control | Transport, kebab, carousel arrows |
| `tile_button()` | Icon over label, full width | Home quick actions |
| `chip_button()` / `chip_row()` / `segmented()` | Mutually exclusive options | Filters, profile, theme, language |
| `line()` / `kv_row()` | Title + hint + trailing control, or label + value | Settings, detail cards |
| `toggler_style()` | The amber switch | Every boolean setting |
| `Sparkline` | A filled area chart over `Vec<Option<f64>>` | Resource tiles, frame time |
| `brand_mark()` | The amber play square | Rail, window icon |
| `heading()` / `heading_icon()` / `caption()` | Section titles | Every card |
| `divider()` | A rule that reads as a shadow line | Card groups |
| `metric_short()` | A value, or the one-word reason there is none | Tiles |

`style::Kind` is the single place button visuals are decided: `Primary`,
`Secondary`, `Icon`, `Nav`, `NavActive`, `Chip`, `ChipActive`, `Tile`,
`ChromeClose`, `IconOnArtwork`.

## The shell

The window draws its own chrome: `decorations: false`, and the gradient has to
reach the top edge the way the reference shows it. That costs two things the
compositor would otherwise provide, and both are rebuilt here.

- **Title bar** — `App::title_bar`: minimise, maximise/restore, close, and a
  `mouse_area` that calls `window::drag`. The maximise glyph follows
  `window::is_maximized`, asked for after the window opens and after every
  toggle, because this Iced version emits no maximize event.
- **Resize** — `App::resize_edge`: four invisible 6px strips over the content,
  each sending `window::drag_resize` for its side. Without them the window
  would be resizable only from the keyboard.

The rail holds the brand, five entries and the daemon's heartbeat. Below
`NARROW` (980 px) it drops its labels to icons only, and the brand and footer
drop their words with it: a label sized to `Fill` inside a 66px rail spills
over into the page beside it.

## Pages

**Home** — the wallpaper's own frame as the hero, with the title, the measured
technical line, transport and volume over a scrim; a Monitors card; four
resource tiles; four action tiles. The hero artwork is `thumbnails::generate_hero`,
so the plate shows the file that is actually playing.

**Library** — the union of history and the managed folder (history forgets
files the user dropped in, the folder list forgets videos played from elsewhere),
as a card grid or rows, filtered by type and by search. Quality labels come from
`ffprobe`, never from the file name: a card claiming 60 FPS because someone
named a file that way is worse than a card saying `Reading…`.

**Online** — a carousel, category tiles, and the listing. The view chips are
`Featured / Recent / Downloaded / Collections`, not the reference's
`Popular`: the listing carries no rating, no download count and no view count,
so a Popular tab would sort by the default order and call it something else.
`Recent` reads the publication month out of the item's own upload path, which
is a fact about the item rather than an inference.

**Performance** — the same four tiles as Home, then what the decoder reports,
then frame time with its axis. The FPS tile shows `estimated-vf-fps` — what the
decoder is producing — not `container-fps`, because a 60 FPS file capped to 30
by the Eco Engine is running at 30. Its ceiling is the display's measured
refresh rate, and the rate is read from RandR (`MonitorInfo::refresh_hz`)
rather than assumed: the panel here reports 120 Hz, and mpv's own
`display-fps` independently answers 119.93.

**Settings** — six sections in a sub-nav column, one panel. `Start with system`
writes the autostart entry, `Run in background` registers or withdraws the tray
item at runtime, `Check for updates` and `Check now` drive the release check.
Nothing on the page is a placeholder: a switch that changed no behaviour was
removed rather than left looking right.

## Sparklines

`Sparkline` takes `Vec<Option<f64>>`, one entry per tick. A tick with no reading
is `None`, and the line **breaks** there. Carrying the last value forward would
draw a plateau that never happened; connecting across the gap would draw a
slope that never happened. The renderer also cannot take a `NaN`: lyon asserts
every point is finite, so a missing sample as a number is a panic, not a gap.

Values are normalised against the series' own floor and its `ceiling`, in the
same unit as the ceiling — the RAM series is stored in GiB because its ceiling
is GiB. A series and its axis in different units is a chart that lies by a
factor of 1024³.

## Interaction and motion

- Hover and pressed states come from `button::Status`; `Primary` darkens on
  press, everything else uses a fill.
- The UI is a polling D-Bus client (1 s tick). Optimistic updates flip the
  rendered value immediately, and `pending_play` guards the one field that must
  not flicker back while the server catches up.
- The rail footer dot is the only state that changes colour on its own:
  `success_tone` when the daemon answers, `danger` when it does not.
- Iced implements no `reduce-motion`, so nothing animates except explicit
  widget interactions. There are no infinite pulses.

## Resource budget

The surface polls the bus once a second and redraws, so anything a view builds
per frame is measured against that.

- `ffprobe` answers are cached per path for the session, and `None` is a cached
  answer too — an unreadable file is asked about once, not sixty times a minute.
- Catalogue previews are fetched once per id.
- The release check is gated by the user's switch *and* a seven-day cache, so
  opening the window to change a volume costs no request.
- Probes and downloads run on the blocking pool via `Task::perform`; the Iced
  thread never waits on a subprocess or a socket.

## Responsive behaviour

| Width | Behaviour |
| --- | --- |
| > 1360 px | Four library columns, six category tiles |
| 1180 px | The designed layout: three columns, five category tiles |
| 980–1179 px | Content narrows, columns still three |
| < 980 px (`NARROW`) | Rail drops to icons, library to two columns, categories to three |
| < 820 px | One library column |

Minimum window is 900 × 620, which is where the narrow layout still shows a
whole resource row; below that nothing fits honestly.

Translations matter here: Russian runs roughly 30% wider than English and
Japanese similarly. Labels wrap rather than ellipsize, and the two fixed
columns are sized from the widest shipped translation, verified by rendering
the Russian build rather than by arithmetic.

## Debug entry point

`FERRAMARI_UI_BOOT="page=library size=900x700 section=playback"` opens the
window at one page, one size and one settings section. The control surface has
no CLI, and the rail that would be clicked through is drawn by the toolkit
under review, so scripted screenshots — every page, both themes, three widths,
three locales — need a way in that does not depend on synthesising a mouse
click. Unrecognised values are ignored rather than fatal: a typo in a test
command should not look like a bug in the app.

## Quality passes

Required before any UI change is called done:

- `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D
  warnings`, `cargo test --workspace --locked`,
  `cargo build --workspace --release --locked`.
- `python3 scripts/glyph-check.py` after touching icons.
- Both light and dark palettes rendered and inspected, plus the system default.
- Window resized narrow and wide; no truncated label in English, Russian or
  Japanese.
- Screenshots reviewed against `docs/screenshots/`.
