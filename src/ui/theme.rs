//! The Liquid Glass design system for the control window.
//!
//! GTK 3 has no `backdrop-filter`, so the material is composed from three
//! layers instead: a custom painted, pre-softened backdrop, translucent
//! surfaces with a light inner edge, and depth shadows. The stylesheet below is
//! a token template; [`Palette`] substitutes the light or dark tokens before it
//! is handed to GTK. This keeps one source of truth for both themes.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk::RGBA;
use gtk::glib;
use gtk::prelude::*;
use gtk::{cairo, gdk, ApplicationWindow, CssProvider, DrawingArea, Settings, StyleContext};

const STYLESHEET: &str = include_str!("../../data/ui/ferramari.css");

/// Semantic colours for one theme. All values are GTK CSS colours.
#[derive(Clone, Copy)]
pub struct Palette {
    // Backdrop painting
    bg_top: &'static str,
    bg_bottom: &'static str,
    glow_warm: &'static str,
    glow_cool: &'static str,
    glow_rose: &'static str,
    glow_mint: &'static str,
    vignette: &'static str,
    sheen: &'static str,
    // CSS tokens
    header_top: &'static str,
    header_bottom: &'static str,
    text: &'static str,
    text_secondary: &'static str,
    text_muted: &'static str,
    surface: &'static str,
    surface_strong: &'static str,
    surface_hover: &'static str,
    surface_active: &'static str,
    input_bg: &'static str,
    input_bg_focus: &'static str,
    border: &'static str,
    border_strong: &'static str,
    divider: &'static str,
    shine: &'static str,
    shine_soft: &'static str,
    accent: &'static str,
    accent_text: &'static str,
    accent_bg: &'static str,
    accent_bg2: &'static str,
    accent_bg_hover: &'static str,
    accent_ink: &'static str,
    accent_soft: &'static str,
    accent_border: &'static str,
    accent_shadow: &'static str,
    focus: &'static str,
    focus_soft: &'static str,
    danger: &'static str,
    danger_soft: &'static str,
    shadow: &'static str,
    shadow_soft: &'static str,
    popover_bg: &'static str,
    tooltip_bg: &'static str,
    tooltip_text: &'static str,
    scrollbar: &'static str,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            bg_top: "#0B111C",
            bg_bottom: "#06080D",
            glow_warm: "rgba(242, 176, 84, 0.20)",
            glow_cool: "rgba(94, 142, 255, 0.17)",
            glow_rose: "rgba(222, 112, 160, 0.12)",
            glow_mint: "rgba(79, 214, 194, 0.09)",
            vignette: "rgba(0, 0, 0, 0.34)",
            sheen: "rgba(255, 255, 255, 0.05)",
            header_top: "#0C111B",
            header_bottom: "#090D15",
            text: "#F4F7FD",
            text_secondary: "#B9C5D8",
            text_muted: "#8E9BB2",
            surface: "rgba(20, 27, 41, 0.62)",
            surface_strong: "rgba(22, 30, 45, 0.82)",
            surface_hover: "rgba(255, 255, 255, 0.055)",
            surface_active: "rgba(255, 255, 255, 0.10)",
            input_bg: "rgba(7, 10, 16, 0.58)",
            input_bg_focus: "rgba(9, 13, 21, 0.78)",
            border: "rgba(255, 255, 255, 0.12)",
            border_strong: "rgba(255, 255, 255, 0.22)",
            divider: "rgba(255, 255, 255, 0.08)",
            shine: "rgba(255, 255, 255, 0.13)",
            shine_soft: "rgba(255, 255, 255, 0.025)",
            accent: "#F3CB7C",
            accent_text: "#F7D79A",
            accent_bg: "#F7D28C",
            accent_bg2: "#E9AE4C",
            accent_bg_hover: "#FFE0A3",
            accent_ink: "#1E1305",
            accent_soft: "rgba(243, 203, 124, 0.14)",
            accent_border: "rgba(243, 203, 124, 0.34)",
            accent_shadow: "rgba(227, 155, 55, 0.24)",
            focus: "#8FD8FF",
            focus_soft: "rgba(143, 216, 255, 0.24)",
            danger: "#FF8C86",
            danger_soft: "rgba(255, 110, 100, 0.20)",
            shadow: "rgba(0, 0, 0, 0.55)",
            shadow_soft: "rgba(0, 0, 0, 0.32)",
            popover_bg: "#101722",
            tooltip_bg: "#161F2D",
            tooltip_text: "#F4F7FD",
            scrollbar: "rgba(255, 255, 255, 0.18)",
        }
    }

    pub fn light() -> Self {
        Self {
            bg_top: "#FAF7F0",
            bg_bottom: "#EBEDF3",
            glow_warm: "rgba(246, 186, 92, 0.34)",
            glow_cool: "rgba(112, 154, 255, 0.22)",
            glow_rose: "rgba(236, 140, 176, 0.16)",
            glow_mint: "rgba(77, 201, 178, 0.16)",
            vignette: "rgba(58, 66, 86, 0.10)",
            sheen: "rgba(255, 255, 255, 0.34)",
            header_top: "#FCFAF5",
            header_bottom: "#F6F2E9",
            text: "#181B22",
            text_secondary: "#465064",
            text_muted: "#616C80",
            surface: "rgba(255, 255, 255, 0.58)",
            surface_strong: "rgba(255, 255, 255, 0.80)",
            surface_hover: "rgba(255, 255, 255, 0.72)",
            surface_active: "rgba(255, 255, 255, 0.92)",
            input_bg: "rgba(255, 255, 255, 0.62)",
            input_bg_focus: "rgba(255, 255, 255, 0.92)",
            border: "rgba(24, 27, 34, 0.10)",
            border_strong: "rgba(24, 27, 34, 0.18)",
            divider: "rgba(24, 27, 34, 0.08)",
            shine: "rgba(255, 255, 255, 0.75)",
            shine_soft: "rgba(255, 255, 255, 0.10)",
            accent: "#B87A1E",
            accent_text: "#8A5308",
            accent_bg: "#F5D893",
            accent_bg2: "#E8B45A",
            accent_bg_hover: "#FFE6AE",
            accent_ink: "#241705",
            accent_soft: "rgba(184, 122, 30, 0.12)",
            accent_border: "rgba(184, 122, 30, 0.30)",
            accent_shadow: "rgba(170, 110, 25, 0.22)",
            focus: "#0B6FB8",
            focus_soft: "rgba(11, 111, 184, 0.20)",
            danger: "#B3261E",
            danger_soft: "rgba(179, 38, 30, 0.14)",
            shadow: "rgba(31, 38, 53, 0.22)",
            shadow_soft: "rgba(31, 38, 53, 0.12)",
            popover_bg: "#FFFFFF",
            tooltip_bg: "#1C2330",
            tooltip_text: "#FFFFFF",
            scrollbar: "rgba(24, 27, 34, 0.22)",
        }
    }

    pub fn css(&self) -> String {
        let tokens = [
            ("BG_BOTTOM", self.bg_bottom),
            ("HEADER_TOP", self.header_top),
            ("HEADER_BOTTOM", self.header_bottom),
            ("TEXT", self.text),
            ("TEXT_SECONDARY", self.text_secondary),
            ("TEXT_MUTED", self.text_muted),
            ("SURFACE", self.surface),
            ("SURFACE_STRONG", self.surface_strong),
            ("SURFACE_HOVER", self.surface_hover),
            ("SURFACE_ACTIVE", self.surface_active),
            ("INPUT_BG", self.input_bg),
            ("INPUT_BG_FOCUS", self.input_bg_focus),
            ("BORDER", self.border),
            ("BORDER_STRONG", self.border_strong),
            ("DIVIDER", self.divider),
            ("SHINE", self.shine),
            ("SHINE_SOFT", self.shine_soft),
            ("ACCENT", self.accent),
            ("ACCENT_TEXT", self.accent_text),
            ("ACCENT_BG", self.accent_bg),
            ("ACCENT_BG2", self.accent_bg2),
            ("ACCENT_BG_HOVER", self.accent_bg_hover),
            ("ACCENT_INK", self.accent_ink),
            ("ACCENT_SOFT", self.accent_soft),
            ("ACCENT_BORDER", self.accent_border),
            ("ACCENT_SHADOW", self.accent_shadow),
            ("FOCUS", self.focus),
            ("FOCUS_SOFT", self.focus_soft),
            ("DANGER", self.danger),
            ("DANGER_SOFT", self.danger_soft),
            ("SHADOW", self.shadow),
            ("SHADOW_SOFT", self.shadow_soft),
            ("POPOVER_BG", self.popover_bg),
            ("TOOLTIP_BG", self.tooltip_bg),
            ("TOOLTIP_TEXT", self.tooltip_text),
            ("SCROLLBAR", self.scrollbar),
        ];

        let mut css = STYLESHEET.to_string();
        for (name, value) in tokens {
            css = css.replace(&format!("@@{name}@@"), value);
        }
        css
    }

    /// Paint the soft, out-of-focus world behind the glass surfaces.
    fn paint_backdrop(&self, cr: &cairo::Context, width: f64, height: f64) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let base = cairo::LinearGradient::new(0.0, 0.0, width * 0.25, height);
        let (r, g, b, _) = parse_rgba(self.bg_top);
        base.add_color_stop_rgba(0.0, r, g, b, 1.0);
        let (r, g, b, _) = parse_rgba(self.bg_bottom);
        base.add_color_stop_rgba(1.0, r, g, b, 1.0);
        let _ = cr.set_source(&base);
        let _ = cr.paint();

        // A wide, very soft sheen across the upper third keeps the surface
        // from looking like a flat gradient.
        let (r, g, b, a) = parse_rgba(self.sheen);
        let sheen = cairo::LinearGradient::new(0.0, 0.0, 0.0, height * 0.55);
        sheen.add_color_stop_rgba(0.0, r, g, b, a);
        sheen.add_color_stop_rgba(1.0, r, g, b, 0.0);
        let _ = cr.set_source(&sheen);
        let _ = cr.paint();

        for (x, y, rx, ry, color) in [
            (-0.08, 0.00, 0.58, 0.52, self.glow_warm),
            (0.98, -0.02, 0.52, 0.58, self.glow_cool),
            (0.08, 1.00, 0.52, 0.56, self.glow_rose),
            (1.00, 0.86, 0.46, 0.48, self.glow_mint),
        ] {
            let (r, g, b, a) = parse_rgba(color);
            let cx = width * x;
            let cy = height * y;
            let rx = width * rx;
            let ry = height * ry;

            let _ = cr.save();
            cr.translate(cx, cy);
            cr.scale(rx, ry);
            let glow = cairo::RadialGradient::new(0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
            glow.add_color_stop_rgba(0.0, r, g, b, a);
            glow.add_color_stop_rgba(0.55, r, g, b, a * 0.55);
            glow.add_color_stop_rgba(1.0, r, g, b, 0.0);
            let _ = cr.set_source(&glow);
            cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            let _ = cr.restore();
        }

        let (r, g, b, a) = parse_rgba(self.vignette);
        let vignette = cairo::RadialGradient::new(
            width * 0.5,
            height * 0.48,
            width.min(height) * 0.15,
            width * 0.5,
            height * 0.5,
            width.max(height) * 0.78,
        );
        vignette.add_color_stop_rgba(0.0, r, g, b, 0.0);
        vignette.add_color_stop_rgba(0.62, r, g, b, a * 0.25);
        vignette.add_color_stop_rgba(1.0, r, g, b, a);
        let _ = cr.set_source(&vignette);
        cr.rectangle(0.0, 0.0, width, height);
        let _ = cr.fill();
    }
}

fn parse_rgba(value: &str) -> (f64, f64, f64, f64) {
    let color = RGBA::parse(value).unwrap_or_else(|_| RGBA::new(0.0, 0.0, 0.0, 0.0));
    (color.red(), color.green(), color.blue(), color.alpha())
}

/// Owns the active palette and the style provider for one control window.
pub struct Theme {
    window: ApplicationWindow,
    settings: Settings,
    provider: RefCell<Option<CssProvider>>,
    palette: RefCell<Palette>,
    backgrounds: RefCell<Vec<glib::WeakRef<DrawingArea>>>,
}

impl Theme {
    pub fn install(window: &ApplicationWindow) -> Rc<Self> {
        let settings = Settings::default().expect("GTK settings are available after init");
        let palette = if settings.is_gtk_application_prefer_dark_theme() {
            Palette::dark()
        } else {
            Palette::light()
        };
        let theme = Rc::new(Self {
            window: window.clone(),
            settings,
            provider: RefCell::new(None),
            palette: RefCell::new(palette),
            backgrounds: RefCell::new(Vec::new()),
        });

        window.style_context().add_class("ferramari-window");
        theme.apply();

        {
            let weak = Rc::downgrade(&theme);
            theme
                .settings
                .connect_gtk_application_prefer_dark_theme_notify(move |_| {
                    if let Some(theme) = weak.upgrade() {
                        theme.apply();
                    }
                });
        }
        {
            let weak = Rc::downgrade(&theme);
            theme
                .settings
                .connect_gtk_enable_animations_notify(move |_| {
                    if let Some(theme) = weak.upgrade() {
                        theme.apply_motion();
                    }
                });
        }

        theme
    }

    /// Create the custom-painted backdrop for the window content.
    pub fn background(self: &Rc<Self>) -> DrawingArea {
        let area = DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_can_focus(false);
        area.set_sensitive(false);

        let weak = Rc::downgrade(self);
        area.connect_draw(move |area, cr| {
            if let Some(theme) = weak.upgrade() {
                theme.palette.borrow().paint_backdrop(
                    cr,
                    area.allocated_width() as f64,
                    area.allocated_height() as f64,
                );
            }
            glib::Propagation::Proceed
        });

        self.backgrounds.borrow_mut().push(area.downgrade());
        area
    }

    fn apply(&self) {
        let dark = self.settings.is_gtk_application_prefer_dark_theme();
        let palette = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        let css = palette.css();
        debug_assert!(
            !css.contains("@@"),
            "stylesheet has an unsubstituted design token"
        );

        let provider = CssProvider::new();
        provider.connect_parsing_error(|_, section, error| {
            log::warn!("[UI] CSS error at line {}: {error}", section.start_line());
        });
        if let Err(error) = provider.load_from_data(css.as_bytes()) {
            log::warn!("[UI] Could not load stylesheet: {error}");
        }

        if let Some(display) = gdk::Display::default() {
            let screen = display.default_screen();
            if let Some(old) = self.provider.borrow_mut().replace(provider.clone()) {
                StyleContext::remove_provider_for_screen(&screen, &old);
            }
            StyleContext::add_provider_for_screen(
                &screen,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        self.palette.replace(palette);
        let context = self.window.style_context();
        if dark {
            context.remove_class("theme-light");
            context.add_class("theme-dark");
        } else {
            context.remove_class("theme-dark");
            context.add_class("theme-light");
        }
        self.apply_motion();

        for area in self.backgrounds.borrow().iter() {
            if let Some(area) = area.upgrade() {
                area.queue_draw();
            }
        }
    }

    fn apply_motion(&self) {
        let context = self.window.style_context();
        if self.settings.is_gtk_enable_animations() {
            context.remove_class("reduce-motion");
        } else {
            context.add_class("reduce-motion");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_is_substituted() {
        for palette in [Palette::light(), Palette::dark()] {
            let css = palette.css();
            assert!(!css.contains("@@"), "unsubstituted token in stylesheet");
        }
    }

    #[test]
    fn themes_have_distinct_backgrounds() {
        assert_ne!(Palette::light().bg_bottom, Palette::dark().bg_bottom);
        assert_ne!(Palette::light().text, Palette::dark().text);
    }
}
