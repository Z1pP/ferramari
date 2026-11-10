//! Ferramari Liquid Glass palette for Iced.
//!
//! Maps the CSS token values from `data/ui/ferramari.css` into Iced `Color`s,
//! preserving the design system across both themes. Dark is the default.

use iced::Color;
use iced::Theme;

// --------------------------------------------------------------------------- //
// Helper constructors                                                          //
// --------------------------------------------------------------------------- //

/// Opaque colour from sRGB bytes.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

/// Colour from sRGB bytes plus an alpha channel in `[0, 1]`.
const fn argb(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, a)
}

/// Translucent white overlay — hover, shine, border highlight tokens.
pub const fn white_a(a: f32) -> Color {
    Color::from_rgba(1.0, 1.0, 1.0, a)
}

/// Translucent black overlay — shadow tokens.
pub const fn black_a(a: f32) -> Color {
    Color::from_rgba(0.0, 0.0, 0.0, a)
}

/// Translucent overlay of one sRGB byte triple — accent/danger tints.
const fn tint(r: u8, g: u8, b: u8, a: f32) -> Color {
    argb(r, g, b, a)
}

// --------------------------------------------------------------------------- //
// Semantic colour constants (one per token)                                    //
// --------------------------------------------------------------------------- //

/// A semantic theme for one visual mode. Each field corresponds to one CSS
/// token from `ferramari.css`. These values drive widget-level style closures,
/// mirroring how the GTK stylesheet substituted `@@TOKEN@@` placeholders.
#[derive(Debug, Clone, Copy)]
pub struct Semantics {
    /// Which palette this is. A style that has to lean harder on black over a
    /// light surface — the hero scrim is the one — reads this instead of
    /// comparing colours to guess.
    pub dark: bool,
    pub bg_bottom: Color,
    pub header_top: Color,
    pub header_bottom: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub surface: Color,
    pub surface_strong: Color,
    pub surface_hover: Color,
    pub surface_active: Color,
    pub input_bg: Color,
    pub input_bg_focus: Color,
    pub border: Color,
    pub border_strong: Color,
    pub divider: Color,
    pub shine: Color,
    pub shine_soft: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub accent_bg: Color,
    pub accent_bg2: Color,
    pub accent_bg_hover: Color,
    pub accent_ink: Color,
    pub accent_soft: Color,
    pub accent_border: Color,
    pub accent_shadow: Color,
    pub focus: Color,
    pub focus_soft: Color,
    pub danger: Color,
    pub danger_soft: Color,
    pub shadow: Color,
    pub shadow_soft: Color,
    pub popover_bg: Color,
    pub tooltip_bg: Color,
    pub tooltip_text: Color,
    pub scrollbar: Color,
    // --- Glassmorphism II: new tokens --------------------------------------- //
    /// Primary ambient shadow for floating cards.
    pub card_shadow: Color,
    /// Top-light edge highlight on glass cards.
    pub card_highlight: Color,
    /// Soft amber glow used on interactive states and nav active items.
    pub glow_accent: Color,
    /// Shimmer animation base color for loading placeholders.
    pub shimmer_base: Color,
    /// Shimmer animation highlight streak.
    pub shimmer_highlight: Color,
    /// Background for stat tiles on the Performance page.
    pub tile_bg: Color,
    /// Progress bar / sparkline fill colour.
    pub progress_fill: Color,
    /// Green-ish tone for low metric usage (≤50%).
    pub success_tone: Color,
    /// Yellow-ish tone for medium metric usage (50–80%).
    pub warning_tone: Color,
    /// Red-ish tone for high metric usage (>80%).
    pub danger_tone: Color,
    /// Icon button background on hover.
    pub icon_hover: Color,
    /// Subtle radial gradient overlay for sidebar brand area.
    pub brand_glow: Color,

    // --- Shell v4: the cinematic surface the reference design is drawn on --- //
    // The reference is not a flat dark panel: it is a night sky with a cold
    // blue light coming from the top-right, and every card is a piece of that
    // same sky, edged in faint daylight. These tokens carry that; the older
    // `surface`/`tile_bg` pair stays for widgets that want the flat material.
    /// Top stop of the page backdrop gradient (cold, near the light source).
    pub bg_glow: Color,
    /// Middle stop: where the glow has faded into the base tone.
    pub bg_mid: Color,
    /// Bottom stop of the page backdrop gradient.
    pub bg_deep: Color,
    /// The rail's own pane, a shade deeper than a card so it reads as the
    /// fixed layer the content slides over.
    pub rail_bg: Color,
    /// The strip the window's own title bar is painted on. The decorations
    /// are off, so this is the topmost thing in the window.
    pub title_bg: Color,
    /// Text drawn on top of a wallpaper frame.
    ///
    /// Deliberately the same in both palettes. The scrim under it is black
    /// with alpha, so it darkens whatever the video shows in either theme,
    /// and a light-theme token here would put near-black text on a
    /// near-black plate. The surface is the artwork, not the app, so the
    /// app's theme does not get to choose the ink.
    pub on_artwork: Color,
    /// The quieter second line on that same plate.
    pub on_artwork_soft: Color,
    /// The plate a status badge sits on when it floats over artwork.
    pub badge_artwork: Color,
    /// The green that means "this is running", drawn on a wallpaper frame.
    ///
    /// Fixed across themes like `on_artwork`: the light theme's `success_tone`
    /// is a dark green chosen to read on paper, and on the dark scrim over a
    /// frame it is the least legible thing on the plate.
    pub live_artwork: Color,
    /// Card fill: translucent so the backdrop reads through it.
    pub card_bg: Color,
    /// Hairline edge of a card, brighter than `border`.
    pub card_edge: Color,
    /// Fill of the inner rows inside a card (list rows, tiles).
    pub row_bg: Color,
    /// Fill of the compact pills and chips.
    pub pill_bg: Color,
    /// Edge of the compact pills and chips.
    pub pill_edge: Color,
    /// Sparkline / progress stroke for the CPU series.
    pub series_cpu: Color,
    /// Sparkline stroke for the memory series.
    pub series_ram: Color,
    /// Sparkline stroke for the GPU series.
    pub series_gpu: Color,
    /// Sparkline stroke for the frame-rate series.
    pub series_fps: Color,
    /// Sparkline stroke for the frame-time series.
    pub series_frame: Color,
    /// The dark ink the amber buttons write their label in.
    pub on_accent: Color,
}

impl Semantics {
    // --- Dark theme (v3: deep blue-black, orange accent) ------------------- //
    // The reference design is a dark cinematic surface lit by a cold blue glow
    // at the top and a warm orange accent. `bg_top` is the cool highlight the
    // root gradient fades from; cards stay translucent so the glow reads
    // through them.
    pub const DARK: Self = Self {
        dark: true,
        bg_bottom: rgb(0x05, 0x08, 0x0E),
        header_top: rgb(0x0A, 0x12, 0x1E),
        header_bottom: rgb(0x07, 0x0B, 0x14),
        text: rgb(0xF5, 0xF7, 0xFA),
        text_secondary: rgb(0xC3, 0xCB, 0xD9),
        text_muted: rgb(0x8A, 0x94, 0xA6),
        surface: argb(0x11, 0x18, 0x27, 0.72),
        surface_strong: argb(0x14, 0x1C, 0x2C, 0.88),
        surface_hover: white_a(0.07),
        surface_active: white_a(0.11),
        input_bg: argb(0x0A, 0x10, 0x1A, 0.85),
        input_bg_focus: argb(0x0D, 0x15, 0x22, 0.95),
        border: white_a(0.08),
        border_strong: white_a(0.16),
        divider: white_a(0.07),
        shine: white_a(0.10),
        shine_soft: white_a(0.02),
        accent: rgb(0xF9, 0x73, 0x16),
        accent_text: rgb(0xFB, 0x92, 0x3C),
        accent_bg: rgb(0xF9, 0x73, 0x16),
        accent_bg2: rgb(0xEA, 0x58, 0x0C),
        accent_bg_hover: rgb(0xFB, 0x92, 0x3C),
        accent_ink: rgb(0x1A, 0x0D, 0x02),
        accent_soft: tint(0xF9, 0x73, 0x16, 0.16),
        accent_border: tint(0xF9, 0x73, 0x16, 0.42),
        accent_shadow: tint(0xEA, 0x58, 0x0C, 0.30),
        focus: rgb(0x8E, 0xD6, 0xFC),
        focus_soft: tint(0x8E, 0xD6, 0xFC, 0.22),
        danger: rgb(0xFE, 0x8A, 0x84),
        danger_soft: tint(0xFE, 0x72, 0x6A, 0.18),
        shadow: black_a(0.55),
        shadow_soft: black_a(0.30),
        popover_bg: rgb(0x0E, 0x15, 0x22),
        tooltip_bg: rgb(0x14, 0x1C, 0x2B),
        tooltip_text: rgb(0xF5, 0xF8, 0xFE),
        scrollbar: white_a(0.15),
        // Glassmorphism II tokens — Dark
        card_shadow: black_a(0.38),
        card_highlight: white_a(0.14),
        glow_accent: tint(0xF9, 0x73, 0x16, 0.38),
        shimmer_base: tint(0x11, 0x18, 0x27, 0.55),
        shimmer_highlight: white_a(0.12),
        tile_bg: argb(0x0F, 0x16, 0x24, 0.82),
        progress_fill: rgb(0xF9, 0x73, 0x16),
        success_tone: rgb(0x22, 0xC5, 0x5E),
        warning_tone: rgb(0xFF, 0xB0, 0x20),
        danger_tone: rgb(0xFF, 0x6B, 0x6B),
        icon_hover: white_a(0.08),
        brand_glow: tint(0xF9, 0x73, 0x16, 0.10),
        // Shell v4 — Dark. The light source is the cold blue in the top
        // corner; cards are translucent so it reads through them.
        bg_glow: rgb(0x10, 0x24, 0x3A),
        bg_mid: rgb(0x0A, 0x14, 0x22),
        bg_deep: rgb(0x04, 0x07, 0x0C),
        rail_bg: argb(0x0B, 0x12, 0x1E, 0.88),
        title_bg: argb(0x07, 0x0C, 0x14, 0.92),
        on_artwork: rgb(0xF5, 0xF7, 0xFA),
        on_artwork_soft: rgb(0xC3, 0xCB, 0xD9),
        badge_artwork: argb(0x0A, 0x10, 0x1A, 0.72),
        live_artwork: rgb(0x34, 0xD3, 0x99),
        card_bg: argb(0x0E, 0x16, 0x22, 0.78),
        card_edge: white_a(0.09),
        row_bg: white_a(0.04),
        pill_bg: white_a(0.07),
        pill_edge: white_a(0.12),
        series_cpu: rgb(0x34, 0xD3, 0x99),
        series_ram: rgb(0x60, 0xA5, 0xFA),
        series_gpu: rgb(0xA7, 0x8B, 0xFA),
        series_fps: rgb(0xFB, 0x92, 0x3C),
        series_frame: rgb(0xF5, 0x9E, 0x0B),
        on_accent: rgb(0x1A, 0x0D, 0x02),
    };

    // --- Light theme (v3: same orange accent, cool paper surface) ----------- //
    pub const LIGHT: Self = Self {
        dark: false,
        bg_bottom: rgb(0xEE, 0xF1, 0xF6),
        header_top: rgb(0xFB, 0xFC, 0xFE),
        header_bottom: rgb(0xF1, 0xF4, 0xF9),
        text: rgb(0x12, 0x15, 0x1C),
        text_secondary: rgb(0x3A, 0x43, 0x54),
        text_muted: rgb(0x5A, 0x64, 0x78),
        surface: white_a(0.72),
        surface_strong: white_a(0.90),
        surface_hover: white_a(0.82),
        surface_active: white_a(0.95),
        input_bg: white_a(0.78),
        input_bg_focus: white_a(0.96),
        border: tint(0x14, 0x17, 0x1E, 0.10),
        border_strong: tint(0x14, 0x17, 0x1E, 0.18),
        divider: tint(0x14, 0x17, 0x1E, 0.08),
        shine: white_a(0.85),
        shine_soft: white_a(0.10),
        accent: rgb(0xEA, 0x58, 0x0C),
        accent_text: rgb(0xC2, 0x41, 0x0C),
        accent_bg: rgb(0xF9, 0x73, 0x16),
        accent_bg2: rgb(0xEA, 0x58, 0x0C),
        accent_bg_hover: rgb(0xFB, 0x8B, 0x34),
        accent_ink: rgb(0xFF, 0xFF, 0xFF),
        accent_soft: tint(0xEA, 0x58, 0x0C, 0.12),
        accent_border: tint(0xEA, 0x58, 0x0C, 0.34),
        accent_shadow: tint(0xEA, 0x58, 0x0C, 0.22),
        focus: rgb(0x0A, 0x66, 0xAD),
        focus_soft: tint(0x0A, 0x66, 0xAD, 0.18),
        danger: rgb(0xA8, 0x22, 0x1A),
        danger_soft: tint(0xA8, 0x22, 0x1A, 0.12),
        shadow: tint(0x1A, 0x20, 0x30, 0.20),
        shadow_soft: tint(0x1A, 0x20, 0x30, 0.10),
        popover_bg: rgb(0xFE, 0xFE, 0xFE),
        tooltip_bg: rgb(0x18, 0x1E, 0x2A),
        tooltip_text: rgb(0xFE, 0xFE, 0xFE),
        scrollbar: tint(0x14, 0x17, 0x1E, 0.20),
        // Glassmorphism II tokens — Light
        card_shadow: tint(0x1A, 0x20, 0x30, 0.16),
        card_highlight: white_a(0.70),
        glow_accent: tint(0xF9, 0x73, 0x16, 0.24),
        shimmer_base: tint(0xEE, 0xF1, 0xF6, 0.50),
        shimmer_highlight: white_a(0.60),
        tile_bg: white_a(0.78),
        progress_fill: rgb(0xEA, 0x58, 0x0C),
        success_tone: rgb(0x15, 0x80, 0x3D),
        warning_tone: rgb(0xB4, 0x69, 0x0E),
        danger_tone: rgb(0xC9, 0x2A, 0x2A),
        icon_hover: white_a(0.72),
        brand_glow: tint(0xF9, 0x73, 0x16, 0.07),
        // Shell v4 — Light. Same structure, no night glow: the backdrop is
        // paper lit from the top, cards are near-white panes.
        bg_glow: rgb(0xF7, 0xFA, 0xFE),
        bg_mid: rgb(0xEE, 0xF2, 0xF8),
        bg_deep: rgb(0xE3, 0xE8, 0xF1),
        rail_bg: white_a(0.90),
        title_bg: white_a(0.55),
        // Same ink as the dark palette: the scrim is black with alpha in both
        // themes, so the text over a wallpaper frame is light in both.
        on_artwork: rgb(0xF5, 0xF7, 0xFA),
        on_artwork_soft: rgb(0xC3, 0xCB, 0xD9),
        badge_artwork: argb(0x0A, 0x10, 0x1A, 0.72),
        live_artwork: rgb(0x34, 0xD3, 0x99),
        card_bg: white_a(0.82),
        card_edge: tint(0x14, 0x17, 0x1E, 0.10),
        row_bg: tint(0x14, 0x17, 0x1E, 0.04),
        pill_bg: tint(0x14, 0x17, 0x1E, 0.06),
        pill_edge: tint(0x14, 0x17, 0x1E, 0.12),
        series_cpu: rgb(0x0F, 0x9E, 0x67),
        series_ram: rgb(0x25, 0x63, 0xEB),
        series_gpu: rgb(0x7C, 0x3A, 0xED),
        series_fps: rgb(0xEA, 0x58, 0x0C),
        series_frame: rgb(0xB4, 0x69, 0x0E),
        on_accent: rgb(0xFF, 0xFF, 0xFF),
    };

    /// The same colour at a fraction of its alpha, for the translucent fill
    /// under a sparkline. Series colours are opaque tokens; a fill has to be
    /// derived, not added, or the two drift apart when the palette changes.
    pub fn fade(color: Color, alpha: f32) -> Color {
        Color {
            a: color.a * alpha,
            ..color
        }
    }

    /// Resolve a theme mode string plus the desktop preference to a palette.
    /// `"dark"`, `"light"`, anything else means "follow system".
    pub fn resolve(mode: &str, system_dark: bool) -> &'static Semantics {
        let dark = match mode {
            "dark" => true,
            "light" => false,
            _ => system_dark,
        };
        if dark {
            &Self::DARK
        } else {
            &Self::LIGHT
        }
    }
}

// --------------------------------------------------------------------------- //
// Theme builder                                                               //
// --------------------------------------------------------------------------- //

/// Returns the Iced [`Theme`] for the active visual mode, so built-in widgets
/// (slider, text input, scrollable) pick up a matching base palette.
pub fn ferramari_theme(system_prefers_dark: bool, mode: &str) -> Theme {
    let sem = Semantics::resolve(mode, system_prefers_dark);
    Theme::custom(String::from("Ferramari"), make_palette(sem))
}

fn make_palette(sem: &Semantics) -> iced::theme::Palette {
    iced::theme::Palette {
        background: sem.bg_bottom,
        text: sem.text,
        primary: sem.accent,
        success: sem.focus,
        warning: sem.accent_bg2,
        danger: sem.danger,
    }
}

/// Best-effort desktop dark preference without GTK.
///
/// Iced 0.14 also exposes `iced::system::theme()` which queries XDG settings
/// over D-Bus (pure Rust). That is the authoritative source and is used at
/// runtime; this helper only covers code paths that run before the event loop
/// boots, such as parsing stored configuration.
pub fn system_prefers_dark() -> bool {
    if let Ok(color) = std::env::var("COLORFGBG") {
        // "0;15" style value: last field is the foreground index.
        if let Some(last) = color.rsplit(';').next() {
            if let Ok(index) = last.trim().parse::<u8>() {
                return index < 8;
            }
        }
    }
    // GNOME/KDE advertise the preference through the settings portal; when
    // unavailable, default to dark, matching Ferramari's GTK default surface.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::theme::Base;

    #[test]
    fn dark_and_light_palettes_differ() {
        assert_ne!(Semantics::DARK.bg_bottom, Semantics::LIGHT.bg_bottom);
        assert_ne!(Semantics::DARK.text, Semantics::LIGHT.text);
        assert_ne!(Semantics::DARK.accent, Semantics::LIGHT.accent);
    }

    #[test]
    fn resolve_follows_system_when_mode_unknown() {
        // Compare by value: `&Semantics::DARK` at two call sites may refer to
        // two promoted copies, so pointer identity is not a valid assertion.
        assert_eq!(
            Semantics::resolve("", true).bg_bottom,
            Semantics::DARK.bg_bottom
        );
        assert_eq!(
            Semantics::resolve("system", false).bg_bottom,
            Semantics::LIGHT.bg_bottom
        );
        // Explicit modes win over the desktop preference.
        assert_eq!(
            Semantics::resolve("dark", false).bg_bottom,
            Semantics::DARK.bg_bottom
        );
        assert_eq!(
            Semantics::resolve("light", true).bg_bottom,
            Semantics::LIGHT.bg_bottom
        );
    }

    #[test]
    fn ferramari_theme_is_named_custom() {
        let name = ferramari_theme(true, "dark").name().to_lowercase();
        assert!(
            name.contains("ferramari"),
            "expected custom theme, got: {name}"
        );
    }

    #[test]
    fn dark_mode_has_near_black_background() {
        let bg = ferramari_theme(true, "").palette().background;
        assert!(bg.r < 0.1 && bg.g < 0.1 && bg.b < 0.1);
    }

    #[test]
    fn light_mode_has_bright_background() {
        let bg = ferramari_theme(false, "light").palette().background;
        assert!(bg.r > 0.8 && bg.g > 0.8);
    }

    #[test]
    fn accent_tokens_are_opaque_and_overlay_tokens_are_translucent() {
        // Surface tokens must stay translucent: that is what makes the glass
        // material read as glass rather than a flat panel.
        const {
            assert!(Semantics::DARK.surface.a < 1.0);
            assert!(Semantics::DARK.border.a < 1.0);
            assert!(Semantics::DARK.accent.a == 1.0);
            assert!(Semantics::DARK.text.a == 1.0);
        }
    }
}
