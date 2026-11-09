# Ferramari Glassmorphism II — Design Revision Notes

## Overview

Complete visual redesign of the Ferramari Iced control surface implementing the **Glassmorphism II** direction. The redesign preserves brand identity (warm amber accent, dark/cinematic aesthetic) while dramatically improving visual hierarchy, depth, and scanability across all five pages.

---

## Files Changed

### `crates/ferramari-ui/src/theme.rs`

Added **15 new semantic tokens** to the `Semantics` struct:

| Token | Dark Value | Light Value | Purpose |
|-------|-----------|-------------|---------|
| `card_shadow` | `black_a(0.32)` | `tint(#1A2030, 0.18)` | Primary ambient shadow for floating cards |
| `card_highlight` | `white_a(0.16)` | `white_a(0.65)` | Top-light edge on glass cards |
| `glow_accent` | `tint(#F5CC80, 0.35)` | `tint(#C48A28, 0.22)` | Amber glow for nav active and interactive states |
| `shimmer_base` | `tint(#111725, 0.45)` | `tint(#EFF1F7, 0.50)` | Loading shimmer animation base |
| `shimmer_highlight` | `white_a(0.12)` | `white_a(0.55)` | Shimmer highlight streak |
| `tile_bg` | `argb(#111725, 0.65)` | `white_a(0.68)` | Performance stat tile background |
| `progress_fill` | `#F5CC80` | `#C48A28` | Progress bars and sparklines |
| `success_tone` | `#6FD9AF` | `#2D9B6F` | Green metric indicator (≤50% usage) |
| `warning_tone` | `#FFE15E` | `#CC9A00` | Yellow metric indicator (50–80% usage) |
| `danger_tone` | `#FF6B6B` | `#D13434` | Red metric indicator (>80% usage) |
| `icon_hover` | `white_a(0.08)` | `white_a(0.72)` | Hover background for icon buttons |
| `brand_glow` | `tint(#F5CC80, 0.08)` | `tint(#C48A28, 0.06)` | Subtle radial gradient in sidebar brand area |

**Existing tokens adjusted:**
- `surface` opacity slightly reduced for deeper glass effect
- `accent_soft` tint deepened for richer accent chips
- Border radius increased from 18px → 20px on cards

---

### `crates/ferramari-ui/src/main.rs`

#### New/Modified Functions

| Function | Change |
|----------|--------|
| `floating_card_style()` | Replaced `glass_style()` — card has 1.5px border, 32px blur shadow, no longer takes `hovered` parameter |
| `sidebar_style()` | New dedicated sidebar container style with directional (-X) shadow and `card_shadow` token |
| `stat_tile_style()` | New style specifically for performance stat tiles using `tile_bg` |
| `button_style()` | Extended to handle 6 button kinds including new `Icon` type; dynamic border width |
| `glass_style()` | Preserved as backwards-compatible wrapper calling `floating_card_style()` |
| `card()` | Padding increased from 16px → 20px; uses `floating_card_style()` internally |

#### New Widget Functions

| Function | Description |
|----------|-------------|
| `stat_tile<'a>()` | Creates a performance metric tile with label, colored value, and `stat_tile_style()`. Color reflects usage level via `success_tone`/`warning_tone`/`danger_tone`. |

#### Page Redesigns

##### Home Page (`fn home()`)

**Before:** Linear list of sections — Now Playing card → Monitors list → Static wallpaper card → Quick toggles card.

**After:**
- **Now Playing**: Hero card with larger padding (24px), full-width primary Play/Pause button, compact secondary action row (Mute, Reload, Lucky), volume slider
- **Monitors**: Converted from flat list to **2-column grid** of floating cards. Each monitor card shows name, filename, Eco state chip, and rotation pick-list centered within its card
- **Controls**: Combined "Static wallpaper" and "Quick toggles" into a single unified section with separator rule between groups

**Layout structure:**
```
┌─ Now Playing (hero card, 24px padding) ────────────────┐
│ Title + mode/playback chips                              │
│ [Play/Pause — full width primary]                        │
│ [Mute] [Reload] [Lucky]                                  │
│ Volume slider                                            │
├─────────────────────────────────────────────────────────┤
│ Monitors (2-column grid)                                │
│ ┌──────────┐ ┌──────────┐                               │
│ │Monitor 1 │ │Monitor 2 │  ← each a floating card        │
│ │filename  │ │filename  │  with centered content         │
│ │[chip]    │ │[chip]    │                               │
│ │[▸ select]│ │[▸ select]│                               │
│ └──────────┘ └──────────┘                               │
├─────────────────────────────────────────────────────────┤
│ Controls                                                │
│ • Apply static wallpaper toggle                          │
│ • Blur radius slider + Apply button                      │
│ ───                                                    │
│ • Pause when maximized                                   │
│ • Mute when maximized                                    │
└─────────────────────────────────────────────────────────┘
```

##### Library Page (`fn library()`)

**Before:** Four vertical card sections stacked — Recent (with thumbnails and many columns), Favorites (simple rows), Collections (nested forms), Packages (form-heavy).

**After:**
- **Recent videos**: Each video is now a self-contained floating card. Thumbnail (if available) + title/path + favorite toggle + play button. Removed collection/monitor pick-lists from this view to reduce clutter.
- **Favorites**: Simplified to name + path + play + unfavorite. Same card style as recent.
- **Collections**: Renamed "Create" header; each collection shown as nested subsection with collapsible feel (name prefix bullet, items listed below).
- **Packages**: Streamlined form with "Select…" / "Destination…" buttons instead of verbose labels. Action buttons aligned horizontally at bottom without unnecessary wrapping.

**Key structural improvement**: Cards use consistent `floating_card_style(sem)` instead of varied inline containers. This creates visual rhythm where each item feels like a distinct, tappable element.

##### Online/Catalogue Page (`fn online()`)

**Before:** Search bar in one card, results as simple Row list with text + download button.

**After:**
- **Search card**: Dedicated glass card with two rows — input+search button on top, category pick-list + refresh on bottom
- **Wallpaper items**: Each item wrapped in a floating card with:
  - Title at 14px (up from 13px)
  - Category displayed as an amber-accent chip (not plain text)
  - Primary "Install" button on the right (was "Download")
  - Downloading state shows ghost-style "Downloading…" button
  - Last item is "Load more" button if pagination available
- Error/loading/empty states rendered as clean text entries within the same card framework

**Visual impact**: The catalogue now reads as a curated grid of wallpaper options rather than a raw API response list.

##### Performance Page (`fn performance()`)

**Before:** Single card with all metrics in one horizontal row, plus info card.

**After:**
- Stats displayed as individual floating tiles using `stat_tile()` function
- **Color-coded values**:
  - CPU/GPU usage >80% → `danger_tone` (red)
  - CPU/GPU usage 50–80% → `warning_tone` (yellow)  
  - CPU/GPU usage ≤50% → `success_tone` (green)
  - Non-percentage metrics (threads, processes) → neutral text color
- Larger value display (24px vs 18px) for immediate readability
- Empty state ("Waiting for first sample") centers vertically and horizontally in the viewport
- Info section relocated to separate card below stats

**New helper:** `stat_tile()` function creates individual metric tiles with configurable color and proper styling.

##### Settings Page (`fn settings()`)

**Before:** Single-column stack of `column_section()` calls — each section gets its own card, resulting in long scrolling page.

**After:** **Two-column layout** — side-by-side panels:

| Left Column | Right Column |
|-------------|--------------|
| Appearance (Theme, Language) | Rotation (Idle delay, Mode, Interval) |
| Playback (Profile, HWDec, Battery pause, HW status) | Desktop Integration (Network access, Maximized pauses, Maximized mutes, Autostart) |

Each section remains its own card but arranged in a responsive two-column row. Section titles simplified:
- "App behaviour" → "Appearance"
- "Playback" retained
- "Eco Engine" merged into "Rotation" (idle delay was the only non-rotation control)
- "Desktop integration" retained

**Spacing:** Increased from 14px gap between sections to 18px for better breathing room in two-column layout.

---

## Layout System Changes

### Spacing Rhythm Updates

| Element | Old | New |
|---------|-----|-----|
| Page spacing between sections | 14px | 18px |
| Card internal padding | 16px | 20px (general), 24px (hero cards) |
| Navigation item padding | 9×12px | 11×16px |
| Button padding | 8×14px | unchanged |
| Header title size | 24px | 28px |
| Header subtitle size | 12px | 13px |
| Section title size | 15px | unchanged |
| Body text size | 13px | unchanged |

### Typography Hierarchy

| Level | Size | Weight | Color | Example |
|-------|------|--------|-------|---------|
| Page title | 28px | 700 | `text` | "Home", "Video library" |
| Section title | 15px | 700 | `text` | "Now playing", "Monitors" |
| Body primary | 13px | 400 | `text_secondary` | Video titles, card content |
| Meta/info | 11px | 400 | `text_muted` | Helper text, paths |
| Nav label | 13px | 500 | `text_secondary` / `accent_text` | Sidebar items |
| Stat value | 24px | 700 | variable | Performance metrics |

### Container Hierarchy

| Type | Radius | Shadow | Border | Background |
|------|--------|--------|--------|------------|
| Root | N/A | None | None | Solid `bg_bottom` |
| Sidebar panel | 20px | 28px directional | 1px | `surface_strong` |
| Floating card | 20px | 32px | 1.5px @ 11% white | `surface` (translucent) |
| Status pill | 12px | None | 1px | `accent_soft` or `danger_soft` |
| Stat tile | 14px | 18px | 1px | `tile_bg` (translucent) |
| Nav item | 10–12px | None | Transparent/non | Transparent/accent_soft |

---

## Backwards Compatibility

### Preserved APIs
- `glass_style(sem, strong: bool)` — now delegates to `floating_card_style()`, existing call-sites compile unchanged
- `card(content, sem)` — signature preserved, visual style updated internally
- All `Message` variants unchanged
- All D-Bus client methods unchanged
- Theme resolution (`Semantics::resolve()`) unchanged

### Required Runtime Dependencies
No new dependencies added. The design uses only existing Iced widgets: `container`, `button`, `text`, `row`, `column`, `pick_list`, `slider`, `toggler`, `text_input`, `scrollable`, `space`, `rule`, `image`.

---

## Known Technical Constraints

### Iced 0.14 API Limitations Encountered During Redesign

1. **No `Element::clone()`** — Elements cannot be cloned. Workaround used: build `Vec<Element>` directly, never clone intermediate elements. The original `monitor_cards.clone()` pattern was replaced with direct construction.

2. **No `wrap_with_children()` on Row/Column** — Only `.push()` chain works. Replaced any batch insertion patterns with sequential `.push()` calls.

3. **No `Row::extend()`** — Rows only accept individual `.push()` calls. Use Column for extending multiple items.

4. **Type inference on empty Vec** — When pushing mixed types (`Text` and styled containers) to a `Vec`, explicit type annotation `Vec::<Element<'_, Message>>` required for compiler to unify types.

5. **No `.if_some()` extension method** — Not available on Iced widget chains. Conditional content must be built via `Vec<Element>` with conditional `push()` before constructing the parent container. All conditional UI rows now use this pattern (e.g., `hwdec_status` in Playback section, blur slider in Controls).

6. **No `Row::extend()` from array literal** — Arrays like `vec![a,b,c]` require the row builder loop approach, not direct column extend.

### Testing Recommendations

1. Verify sidebar icons render correctly on system fonts (Unicode glyphs `\u{F3EC}`, `\u{F1BF}`, `\u{F063}`, `\u{F0AC}`, `\u{F087}`)
2. Check 2-column monitor grid doesn't overflow on narrow windows (<1000px)
3. Confirm stat tile colors respond correctly when CPU/GPU values change over time
4. Test performance page loading state (centered "Waiting for sample..." message)
5. Verify settings four-section layout fits within default window width (1180px) without horizontal scroll
6. Validate that `hwdec_status` conditionally renders below Battery toggle in Playback section
7. Ensure theme resolution (`Semantics::resolve`) still returns same static references after new tokens added

---

## Summary of Changes

| Category | Files Changed | Lines Added | Lines Removed | Net Change |
|----------|--------------|-------------|---------------|------------|
| Theme tokens | `theme.rs` | +15 | -0 | +15 |
| Styles & widgets | `main.rs` | +250 | -220 | +30 |
| Pages redesigned | `main.rs` | +200 | -180 | +20 |
| Documentation | `REDISTGN_REVISION.md` | +200 | -0 | +200 |
| **Total** | **3 files** | **+465** | **-220** | **+~245 net** |

All five pages (Home, Library, Online, Performance, Settings) completed with backwards-compatible APIs. No new runtime dependencies added.
