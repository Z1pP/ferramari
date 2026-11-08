# Ferramari UI — Liquid Glass design system

The control window is a GTK 3 desktop surface, so the design system is
implemented as a token-substituted GTK stylesheet plus a small Cairo backdrop
painter. There is no web layer and no new UI framework.

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
| `data/ui/ferramari.css` | Token template for every component and state |
| `src/ui/theme.rs` | Light/dark palettes, CSS loading, backdrop painting |
| `src/ui/control.rs` | Header, sidebar, pages, cards, forms, popovers |

## Colour tokens

All colours live in `Palette` (`light()` / `dark()`) and are substituted into
`@@TOKEN@@` slots at runtime. The app follows the system GTK dark preference;
there is deliberately no in-app theme switch.

| Token | Role |
| --- | --- |
| `BG_TOP` / `BG_BOTTOM` | Painted backdrop base |
| `SURFACE` | Standard glass pane |
| `SURFACE_STRONG` | Toolbars and sticky action bars |
| `SURFACE_HOVER` / `SURFACE_ACTIVE` | Interaction fills |
| `TEXT` / `TEXT_SECONDARY` / `TEXT_MUTED` | Three-level text hierarchy |
| `BORDER` / `BORDER_STRONG` | Hairline and emphasised edges |
| `SHINE` / `SHINE_SOFT` | Inner highlight gradient on glass |
| `ACCENT` / `ACCENT_TEXT` / `ACCENT_BG*` | Sunlight accent, action colour, text |
| `ACCENT_INK` | Text on the amber primary button |
| `FOCUS` / `FOCUS_SOFT` | Keyboard focus ring |
| `DANGER` / `DANGER_SOFT` | Inline validation errors |
| `SHADOW` / `SHADOW_SOFT` | Elevation and atmospheric depth |

Measured contrast for the key pairs (WCAG AA normal text target is 4.5:1):

| Pair | Ratio |
| --- | --- |
| Dark body text on surface | 16.1:1 |
| Dark secondary text on surface | 9.9:1 |
| Dark muted text on surface | 6.1:1 |
| Light body text on surface | 17.2:1 |
| Light secondary text on surface | 8.1:1 |
| Light muted text on surface | 5.3:1 |
| Amber button ink on amber | 12.6:1 |

## Material layers

1. **Backdrop** — a Cairo-painted base gradient, four very large radial glows
   (warm, cool, rose, mint), a top sheen and a soft vignette. It is painted once
   per resize/theme change, not per frame, and uses no blur filters.
2. **Glass panes** — `SURFACE`/`SURFACE_STRONG` fills with a 145° `SHINE`
   gradient, a 1px border and an inset top highlight. The main panes are
   opaque enough (62–82% + tint) to keep text readable over the backdrop.
3. **Content** — text, controls, thumbnails and actions sit above the panes.
4. **Elevation** — one soft ambient shadow per pane; no stack of competing
   glows. Hover raises a card by changing its edge and shadow only.

GTK 3 has no `backdrop-filter`, so this is the native equivalent: pre-softened
background, translucent material, light edge, depth. It also means there is no
expensive large-area blur and no unsupported-API fallback to manage.

## Typography

- Family: **Lato** with the system sans/Noto fallback for missing glyphs.
- Base: 14 px, body copy 13 px, metadata 11–12 px.
- Page title: 27 px / 800. Form title: 18 px / 800.
- Section label: 12 px / 700, muted. Labels are sentence case.
- Line length is capped on forms and hints (~60 characters) with wrapping.

## Spacing and shape

- Spacing rhythm: 4, 8, 12, 16, 18, 20, 26 px. Page gutter: 26 px desktop.
- Radii: 9 px titlebar buttons, 12 px inputs/buttons, 18–22 px panes and cards,
  999 px pills and switches.
- Header: 62 px. Sidebar: 238 px. Card grid: 210 px min with a 16:9 frame.

## Components

- **Header** — product mark, live playback status chip, transport control,
  lucky, volume and quick-settings buttons. Close/minimise/maximise stay native.
- **Sidebar** — section label, three navigation items with icon/title/context,
  live “Now playing” card, version at the base.
- **Top rail** — the narrow-window equivalent of the sidebar; same stack, same
  active state, no duplicate product logic.
- **Video library** — searchable FlowBox cards with cached 16:9 thumbnails,
  selection state, disabled/enabled apply bar, and two empty states
  (no videos / no search matches).
- **Forms** — glass cards with visible labels, helper copy, inline errors and
  primary actions. The HTML picker uses a native file dialog rather than a
  persistent `(None)` file button.
- **Quick settings** — switch rows grouped by Playback / Wallpaper / System.
  Empty groups are removed on platforms where the feature does not exist.
- **Popovers and menus** — glass sheets with consistent radii, padding, hover
  and keyboard focus states. Wallpaper right-click menus use the same styles.

## Interaction and motion

- Hover/active: 140–160 ms colour, border and shadow transitions.
- Stack changes: GTK crossfade, 180 ms.
- No infinite pulsing or decorative animation. Streaming resolution shows a
  real spinner and status text until yt-dlp returns.
- When GTK reports `gtk-enable-animations = false`, the window gets the
  `reduce-motion` class and CSS transitions are disabled. GTK stack transitions
  already honour the same setting.
- `GtkStack` pages, flow-box cards and form entries are keyboard reachable.
  Mouse clicks on navigation do not steal keyboard focus (`focus-on-click` is
  off on nav items); keyboard focus is visible on every control.
- Icon-only buttons carry tooltips and ATK accessible names. Cards expose the
  filename as their accessible name. Field labels are also set as entry
  accessible names.

## Responsive behaviour

| Width | Layout |
| --- | --- |
| ≥ 1020 px | Sidebar + centred status chip in the header |
| 900–1019 px | Sidebar, status chip hidden |
| 680–899 px | Sidebar hidden, horizontal top rail, brand subtitle hidden below 800 px |
| 680 px minimum | Two/three card columns, no horizontal scrolling |

The app is a Linux desktop utility; 680 px is the supported minimum viewport.
Tested at 680, 760, 1100 and 1600 px.

## Quality passes

- Both light and dark palettes were rendered and inspected.
- The default, streaming, web-page, search-with-results, search-empty,
  selected-card, quick-settings, volume and narrow layouts were rendered.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` and `cargo build --release` pass.
- A release run produces no GTK CSS parsing warnings or panics.