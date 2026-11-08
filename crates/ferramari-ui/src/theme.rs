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
}

impl Semantics {
    // --- Dark theme (v2: deeper, warmer, cleaner contrast) ------------------ //
    pub const DARK: Self = Self {
        bg_bottom: rgb(0x05, 0x07, 0x0C),
        header_top: rgb(0x0B, 0x10, 0x1A),
        header_bottom: rgb(0x08, 0x0C, 0x14),
        text: rgb(0xF5, 0xF8, 0xFE),
        text_secondary: rgb(0xBD, 0xC8, 0xDB),
        text_muted: rgb(0x8B, 0x96, 0xAE),
        surface: argb(0x11, 0x17, 0x25, 0.58),
        surface_strong: argb(0x13, 0x19, 0x28, 0.80),
        surface_hover: white_a(0.06),
        surface_active: white_a(0.09),
        input_bg: argb(0x06, 0x09, 0x0F, 0.55),
        input_bg_focus: argb(0x08, 0x0C, 0x14, 0.75),
        border: white_a(0.10),
        border_strong: white_a(0.18),
        divider: white_a(0.07),
        shine: white_a(0.11),
        shine_soft: white_a(0.02),
        accent: rgb(0xF5, 0xCC, 0x80),
        accent_text: rgb(0xFA, 0xDA, 0xA2),
        accent_bg: rgb(0xF9, 0xD6, 0x94),
        accent_bg2: rgb(0xEC, 0xB2, 0x5E),
        accent_bg_hover: rgb(0xFF, 0xE4, 0xAB),
        accent_ink: rgb(0x20, 0x14, 0x05),
        accent_soft: tint(0xF5, 0xCC, 0x80, 0.13),
        accent_border: tint(0xF5, 0xCC, 0x80, 0.30),
        accent_shadow: tint(0xDE, 0x96, 0x34, 0.22),
        focus: rgb(0x8E, 0xD6, 0xFC),
        focus_soft: tint(0x8E, 0xD6, 0xFC, 0.22),
        danger: rgb(0xFE, 0x8A, 0x84),
        danger_soft: tint(0xFE, 0x72, 0x6A, 0.18),
        shadow: black_a(0.50),
        shadow_soft: black_a(0.28),
        popover_bg: rgb(0x0E, 0x13, 0x1E),
        tooltip_bg: rgb(0x14, 0x1C, 0x2B),
        tooltip_text: rgb(0xF5, 0xF8, 0xFE),
        scrollbar: white_a(0.15),
        // Glassmorphism II tokens — Dark
        card_shadow: black_a(0.32),
        card_highlight: white_a(0.16),
        glow_accent: tint(0xF5, 0xCC, 0x80, 0.35),
        shimmer_base: tint(0x11, 0x17, 0x25, 0.45),
        shimmer_highlight: white_a(0.12),
        tile_bg: argb(0x11, 0x17, 0x25, 0.65),
        progress_fill: rgb(0xF5, 0xCC, 0x80),
        success_tone: rgb(0x6F, 0xD9, 0xAF),
        warning_tone: rgb(0xFF, 0xE1, 0x5E),
        danger_tone: rgb(0xFF, 0x6B, 0x6B),
        icon_hover: white_a(0.08),
        brand_glow: tint(0xF5, 0xCC, 0x80, 0.08),
    };

    // --- Light theme (v2: cleaner, higher contrast) -------------------------- //
    pub const LIGHT: Self = Self {
        bg_bottom: rgb(0xEF, 0xF1, 0xF7),
        header_top: rgb(0xFD, 0xFB, 0xF7),
        header_bottom: rgb(0xF8, 0xF5, 0xEC),
        text: rgb(0x14, 0x17, 0x1E),
        text_secondary: rgb(0x3E, 0x47, 0x5A),
        text_muted: rgb(0x5A, 0x64, 0x78),
        surface: white_a(0.52),
        surface_strong: white_a(0.78),
        surface_hover: white_a(0.70),
        surface_active: white_a(0.90),
        input_bg: white_a(0.58),
        input_bg_focus: white_a(0.90),
        border: tint(0x14, 0x17, 0x1E, 0.08),
        border_strong: tint(0x14, 0x17, 0x1E, 0.16),
        divider: tint(0x14, 0x17, 0x1E, 0.06),
        shine: white_a(0.80),
        shine_soft: white_a(0.08),
        accent: rgb(0xC4, 0x8A, 0x28),
        accent_text: rgb(0x92, 0x5C, 0x0A),
        accent_bg: rgb(0xF6, 0xDC, 0x96),
        accent_bg2: rgb(0xEE, 0xBD, 0x62),
        accent_bg_hover: rgb(0xFF, 0xE8, 0xB4),
        accent_ink: rgb(0x22, 0x15, 0x05),
        accent_soft: tint(0xC4, 0x8A, 0x28, 0.11),
        accent_border: tint(0xC4, 0x8A, 0x28, 0.28),
        accent_shadow: tint(0xBB, 0x7D, 0x24, 0.20),
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
        card_shadow: tint(0x1A, 0x20, 0x30, 0.18),
        card_highlight: white_a(0.65),
        glow_accent: tint(0xC4, 0x8A, 0x28, 0.22),
        shimmer_base: tint(0xEF, 0xF1, 0xF7, 0.50),
        shimmer_highlight: white_a(0.55),
        tile_bg: white_a(0.68),
        progress_fill: rgb(0xC4, 0x8A, 0x28),
        success_tone: rgb(0x2D, 0x9B, 0x6F),
        warning_tone: rgb(0xCC, 0x9A, 0x00),
        danger_tone: rgb(0xD1, 0x34, 0x34),
        icon_hover: white_a(0.72),
        brand_glow: tint(0xC4, 0x8A, 0x28, 0.06),
    };

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
