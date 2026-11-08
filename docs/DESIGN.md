# Ferramari UI — Liquid Glass design system (Iced)

The control surface is an **Iced 0.14** application. There is no GTK, no
Cairo painter and no runtime CSS: every token is a Rust constant and every
widget style is a closure. The design language is unchanged — *Sunlit glass* —
only its implementation moved.

The historical GTK specification (1.9.x) is archived at
[`archive/DESIGN-gtk-1.9.md`](archive/DESIGN-gtk-1.9.md). `data/ui/ferramari.css`
survives as a **reference only**: it is no longer compiled into the binary and
no code loads it. `theme.rs` was derived from its token values.

## Direction

**Sunlit glass.** The product is a video wallpaper app, so the interface is a
dark, cinematic control surface lit by the same warm sunlight that gives
Ferramari its name. Content sits on translucent panes over a soft, out-of-focus
world; the controls are the sharp, tactile layer on top.

- Apple-style material depth: translucent panes, inner light, fine edges.
- A warm amber accent for actions and live playback state.
- Large, calm typography with short lines and generous breathing room.
- Motion only where it explains a state change.

## Files

| File | Responsibility |
| --- | --- |
| `crates/ferramari-ui/src/theme.rs` | `Semantics` token set (DARK/LIGHT), Iced `Theme` builder, system dark-preference probe |
| `crates/ferramari-ui/src/main.rs` | Pages, widget builders (`card`, `chip`, `primary_button`, `toggle_row`, `segmented_row`, `slider_row`, `stat_tile`) and every style closure |
| `crates/ferramari-ui/src/client.rs` | D-Bus snapshot the view renders from |
| `data/ui/ferramari.css` | **Reference only** — the GTK-era token sheet, kept so palette values stay traceable |

## Colour tokens

All colours live in one struct, `Semantics`, with two associated constants:
`Semantics::DARK` and `Semantics::LIGHT`. `Semantics::resolve(mode, system_dark)`
picks one; `"dark"` and `"light"` win over the desktop preference, anything
else follows it. `ferramari_theme()` maps the same palette into Iced's built-in
`Palette` so `slider`, `text_input`, `scrollable` and `pick_list` match.

Do not introduce ad-hoc hex values in `main.rs`. Add a token to `Semantics` in
both palettes instead, and keep the CSS sheet in step if the value is shared.

| Token | Role |
| --- | --- |
| `bg_bottom` | Window backdrop |
| `header_top` / `header_bottom` | Header gradient endpoints |
| `text` / `text_secondary` / `text_muted` | Three-level text hierarchy |
| `surface` / `surface_strong` | Standard and elevated glass panes |
| `surface_hover` / `surface_active` | Interaction fills |
| `input_bg` / `input_bg_focus` | Text-entry material |
| `border` / `border_strong` / `divider` | Hairline, emphasised edge, rule |
| `shine` / `shine_soft` | Inner highlight on glass |
| `accent` / `accent_text` / `accent_bg` / `accent_bg2` / `accent_bg_hover` / `accent_ink` | Sunlight accent, action colour, text on amber |
| `accent_soft` / `accent_border` / `accent_shadow` | Accent tint for chips, glows and borders |
| `focus` / `focus_soft` | Keyboard focus ring |
| `danger` / `danger_soft` | Inline validation errors |
| `shadow` / `shadow_soft` / `card_shadow` | Elevation and atmospheric depth |
| `card_highlight` | Top-light edge highlight on glass cards |
| `glow_accent` | Soft amber glow for interactive and active-nav states |
| `shimmer_base` / `shimmer_highlight` | Loading-placeholder shimmer |
| `tile_bg` | Stat tiles on the Performance page |
| `progress_fill` | Progress and sparkline fill |
| `success_tone` / `warning_tone` / `danger_tone` | Metric usage ≤50% / 50–80% / >80% |
| `icon_hover` | Icon and nav button hover fill |
| `brand_glow` | Radial overlay behind the sidebar brand |
| `popover_bg` / `tooltip_bg` / `tooltip_text` / `scrollbar` | Overlays and scroll thumb |

Two invariants are enforced at compile time by a test
(`accent_tokens_are_opaque_and_overlay_tokens_are_translucent`): accent and
text tokens are opaque, `surface` and `border` are translucent. A surface that
becomes opaque stops reading as glass.

## Material layers

1. **Backdrop** — `root_style` paints a solid `bg_bottom`. Iced has no
   `backdrop-filter`, so depth comes from translucent fills over an already
   dark base rather than from a blur pass.
2. **Glass panes** — `floating_card_style`: a `surface` fill, a 1.5px
   translucent border, and one soft ambient shadow offset 10px down with a
   32px blur. `sidebar_style` uses `surface_strong` and a directional shadow.
3. **Content** — text, controls and thumbnails sit above the panes.
4. **Elevation** — one shadow per pane; no stack of competing glows. Hover
   changes a button's fill and border, it does not move the widget.

## Typography

Sizes are passed per `text()` call; the scale in use is:

| Role | Size |
| --- | --- |
| Brand wordmark (sidebar) | 22 |
| Page title | 28 |
| Metric value | 24 |
| Card / list title, nav labels, section title | 13–15 |
| Body, hints, subtitles | 11–12 |
| Path underlines, chips | 10–11 |

Icons are Material Symbols codepoints from a bundled icon font
(`Page::icon()`), so there is no external asset to ship and no SVG pipeline.

## Spacing and shape

- Rhythm: 4, 6, 8, 10, 12, 14, 16, 18, 20, 24 px. Page gutter: 14 px, gap
  between sidebar and content: 14 px.
- Radii: 7 px chips, 10–12 px buttons and ghost states, 14 px primary buttons
  and tiles, 20 px cards and the sidebar.
- Sidebar: fixed 240 px. Default window: 1180 × 780.
- Card padding: 20 px for sections, 16 px for monitor tiles, 12–14 px for list
  rows.

## Components

| Builder | Produces | Used by |
| --- | --- | --- |
| `card()` | `surface` glass pane, 20 px padding | Collections |
| `floating_card_style` | Standalone card style | Monitor tiles, list rows, catalogue rows |
| `sidebar_style` | Elevated left panel | Sidebar |
| `root_style` | Window backdrop | `view()` |
| `status_style()` | Accent or danger pill | Header status line |
| `stat_tile_style()` | `tile_bg` metric tile | Performance |
| `primary_button()` | Filled amber action | Play/Pause, Apply blur |
| `secondary_button()` | Outlined glass action | Every other action |
| `button_style(…, Nav / NavActive)` | Sidebar rows | Navigation |
| `chip()` | Small accent pill | Mode, playback state, Eco state, category |
| `section_title()` | 15 px heading | Every section |
| `toggle_row()` | Label + hint + `toggler` | Boolean settings |
| `slider_row()` | Label + value + `slider` | Volume, blur, intervals |
| `segmented_row()` | Label + hint + mutually exclusive buttons | Profile, hwdec, theme, language, rotation mode |
| `metric()` + `stat_tile()` | Value or its *reason for absence* | Performance |

`ButtonKind` is the single place button visuals are decided: `Primary`,
`Secondary`, `Ghost`, `Nav`, `NavActive`, `Icon`.

## Interaction and motion

- Hover/pressed states come from `button::Status`; there is no separate
  pressed style beyond the hover fill.
- The UI is a polling D-Bus client (1 s tick). Optimistic updates flip the
  rendered value immediately and `pending_play` guards the one field that must
  not flicker back while the server catches up.
- The connection dot in the sidebar is the only state that changes colour on
  its own: `focus` when reachable, `danger` when not.
- Iced does not implement GTK's `reduce-motion` transition, so nothing
  animates except explicit widget interactions. There are no infinite pulses.
- Every control performs a real D-Bus call. Nothing is decorative.

## Responsive behaviour

The sidebar is fixed 240 px and content fills the rest; monitor tiles form a
two-column grid and Settings uses a two-column row.

| Width | Behaviour |
| --- | --- |
| ≥ 1180 px | Designed layout |
| 760–1179 px | Content column narrows, grids reflow to one column |
| < 760 px | Two-column rows compress; text wrapping is relied on rather than ellipsis |

Translations matter here: Russian strings run roughly 30% wider than English
and Japanese similarly, so `toggle_row` and `segmented_row` give the label
column `Length::Fill` and let it wrap.

## Quality passes

Required before any UI change is called done:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace --locked`,
  `cargo build --workspace --release --locked`.
- Both light and dark palettes rendered and inspected, plus the system default.
- Window resized narrow and wide; no truncated label in any of the three
  locales.
- Screenshots reviewed against `docs/screenshots/`.
