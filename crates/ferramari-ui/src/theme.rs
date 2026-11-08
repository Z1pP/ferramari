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
const fn white_a(a: f32) -> Color {
    Color::from_rgba(1.0, 1.0, 1.0, a)
}

/// Translucent black overlay — shadow tokens.
const fn black_a(a: f32) -> Color {
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
}

impl Semantics {
    // --- Dark theme (values transcribed from Palette::dark()) --------------- //
    pub const DARK: Self = Self {
        bg_bottom: rgb(0x06, 0x08, 0x0D),
        header_top: rgb(0x0C, 0x11, 0x1B),
        header_bottom: rgb(0x09, 0x0D, 0x15),
        text: rgb(0xF4, 0xF7, 0xFD),
        text_secondary: rgb(0xB9, 0xC5, 0xD8),
        text_muted: rgb(0x8E, 0x9B, 0xB2),
        surface: argb(0x14, 0x1B, 0x29, 0.62),
        surface_strong: argb(0x16, 0x1E, 0x2D, 0.82),
        surface_hover: white_a(0.055),
        surface_active: white_a(0.10),
        input_bg: argb(0x07, 0x0A, 0x10, 0.58),
        input_bg_focus: argb(0x09, 0x0D, 0x15, 0.78),
        border: white_a(0.12),
        border_strong: white_a(0.22),
        divider: white_a(0.08),
        shine: white_a(0.13),
        shine_soft: white_a(0.025),
        accent: rgb(0xF3, 0xCB, 0x7C),
        accent_text: rgb(0xF7, 0xD7, 0x9A),
        accent_bg: rgb(0xF7, 0xD2, 0x8C),
        accent_bg2: rgb(0xE9, 0xAE, 0x4C),
        accent_bg_hover: rgb(0xFF, 0xE0, 0xA3),
        accent_ink: rgb(0x1E, 0x13, 0x05),
        accent_soft: tint(0xF3, 0xCB, 0x7C, 0.14),
        accent_border: tint(0xF3, 0xCB, 0x7C, 0.34),
        accent_shadow: tint(0xE3, 0x9B, 0x37, 0.24),
        focus: rgb(0x8F, 0xD8, 0xFF),
        focus_soft: tint(0x8F, 0xD8, 0xFF, 0.24),
        danger: rgb(0xFF, 0x8C, 0x86),
        danger_soft: tint(0xFF, 0x6E, 0x64, 0.20),
        shadow: black_a(0.55),
        shadow_soft: black_a(0.32),
        popover_bg: rgb(0x10, 0x17, 0x22),
        tooltip_bg: rgb(0x16, 0x1F, 0x2D),
        tooltip_text: rgb(0xF4, 0xF7, 0xFD),
        scrollbar: white_a(0.18),
    };

    // --- Light theme (values transcribed from Palette::light()) -------------- //
    pub const LIGHT: Self = Self {
        bg_bottom: rgb(0xEB, 0xED, 0xF3),
        header_top: rgb(0xFC, 0xFA, 0xF5),
        header_bottom: rgb(0xF6, 0xF2, 0xE9),
        text: rgb(0x18, 0x1B, 0x22),
        text_secondary: rgb(0x46, 0x50, 0x64),
        text_muted: rgb(0x61, 0x6C, 0x80),
        surface: white_a(0.58),
        surface_strong: white_a(0.80),
        surface_hover: white_a(0.72),
        surface_active: white_a(0.92),
        input_bg: white_a(0.62),
        input_bg_focus: white_a(0.92),
        border: tint(0x18, 0x1B, 0x22, 0.10),
        border_strong: tint(0x18, 0x1B, 0x22, 0.18),
        divider: tint(0x18, 0x1B, 0x22, 0.08),
        shine: white_a(0.75),
        shine_soft: white_a(0.10),
        accent: rgb(0xB8, 0x7A, 0x1E),
        accent_text: rgb(0x8A, 0x53, 0x08),
        accent_bg: rgb(0xF5, 0xD8, 0x93),
        accent_bg2: rgb(0xE8, 0xB4, 0x5A),
        accent_bg_hover: rgb(0xFF, 0xE6, 0xAE),
        accent_ink: rgb(0x24, 0x17, 0x05),
        accent_soft: tint(0xB8, 0x7A, 0x1E, 0.12),
        accent_border: tint(0xB8, 0x7A, 0x1E, 0.30),
        accent_shadow: tint(0xAA, 0x6E, 0x19, 0.22),
        focus: rgb(0x0B, 0x6F, 0xB8),
        focus_soft: tint(0x0B, 0x6F, 0xB8, 0.20),
        danger: rgb(0xB3, 0x26, 0x1E),
        danger_soft: tint(0xB3, 0x26, 0x1E, 0.14),
        shadow: tint(0x1F, 0x26, 0x35, 0.22),
        shadow_soft: tint(0x1F, 0x26, 0x35, 0.12),
        popover_bg: rgb(0xFF, 0xFF, 0xFF),
        tooltip_bg: rgb(0x1C, 0x23, 0x30),
        tooltip_text: rgb(0xFF, 0xFF, 0xFF),
        scrollbar: tint(0x18, 0x1B, 0x22, 0.22),
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
