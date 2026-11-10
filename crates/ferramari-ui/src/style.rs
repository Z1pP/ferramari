//! Widget styles and the small building blocks every page composes from.
//!
//! One place owns the material: the backdrop gradient, the glass card, the
//! amber pill, the sparkline. A page asks for a shape by name and never spells
//! out a colour, which is what keeps the reference design from drifting once
//! someone adds a sixth page.

use iced::widget::{
    button, canvas, container, mouse_area, rule, text, Button, Column, Container, Row, Space,
};
use iced::{
    Alignment, Background, Border, Color, Degrees, Element, Font, Length, Padding, Pixels, Point,
    Rectangle, Shadow, Vector,
};

use crate::theme::{black_a, Semantics};

// --------------------------------------------------------------------------- //
// Icons                                                                        //
// --------------------------------------------------------------------------- //

/// The family name the distro packages give Font Awesome 4.7.
///
/// The glyphs live in the Private Use Area, so they only resolve when the font
/// is named: with the default family, cosmic-text falls back through DejaVu,
/// which has a house and a cog but not a filmstrip, and the row reads as half
/// icons, half tofu.
pub const ICON_FONT: &str = "FontAwesome";

/// One icon by name, as a `text` widget already bound to the icon font.
///
/// A `char` cannot carry a family name and every call site repeating it would
/// be the same bug waiting to come back, so icon text goes through here.
pub fn icon<'a, M>(name: &str, size: impl Into<Pixels>) -> text::Text<'a> {
    text(glyph(name))
        .size(size)
        .font(Font::with_name(ICON_FONT))
}

/// Font Awesome 4.7 codepoints, by the name the design uses.
pub fn glyph(name: &str) -> char {
    match name {
        "play" => '\u{f04b}',
        "pause" => '\u{f04c}',
        "stop" => '\u{f04d}',
        "step-backward" => '\u{f048}',
        "step-forward" => '\u{f051}',
        "redo" => '\u{f01e}',
        "sync" => '\u{f021}',
        "power-off" => '\u{f011}',
        "home" => '\u{f015}',
        "film" => '\u{f008}',
        "globe" => '\u{f0ac}',
        "activity" => '\u{f201}',
        "cog" => '\u{f013}',
        "cogs" => '\u{f085}',
        "desktop" => '\u{f108}',
        "search" => '\u{f002}',
        "list" => '\u{f03a}',
        "th-large" => '\u{f009}',
        "chevron-left" => '\u{f104}',
        "chevron-right" => '\u{f105}',
        "chevron-down" => '\u{f078}',
        "ellipsis-v" => '\u{f142}',
        "ellipsis-h" => '\u{f141}',
        "check" => '\u{f00c}',
        "check-circle" => '\u{f058}',
        "circle" => '\u{f111}',
        "circle-o" => '\u{f10c}',
        "times" => '\u{f00d}',
        "plus" => '\u{f067}',
        "minus" => '\u{f068}',
        "image" => '\u{f03e}',
        "folder-open" => '\u{f07c}',
        "download" => '\u{f019}',
        "server" => '\u{f233}',
        "microchip" => '\u{f2db}',
        "clock-o" => '\u{f017}',
        "eye" => '\u{f06e}',
        "volume-up" => '\u{f028}',
        "volume-off" => '\u{f026}',
        "language" => '\u{f1ab}',
        "paint-brush" => '\u{f1fc}',
        "sliders" => '\u{f1de}',
        "rocket" => '\u{f135}',
        "bolt" => '\u{f0e7}',
        "leaf" => '\u{f06c}',
        "building" => '\u{f0f8}',
        "star" => '\u{f005}',
        "star-o" => '\u{f006}',
        "sun" => '\u{f185}',
        "moon" => '\u{f186}',
        "adjust" => '\u{f042}',
        "window-minimize" => '\u{f2d1}',
        "window-maximize" => '\u{f2d0}',
        "window-restore" => '\u{f2d2}',
        "folder" => '\u{f07b}',
        "wrench" => '\u{f0ad}',
        "info-circle" => '\u{f05a}',
        "exclamation-triangle" => '\u{f071}',
        "wifi" => '\u{f1eb}',
        "compress" => '\u{f066}',
        _ => '\u{f111}',
    }
}

// --------------------------------------------------------------------------- //
// Layout constants                                                             //
// --------------------------------------------------------------------------- //

/// Width of the navigation rail, and the parts it is made of.
///
/// The number is set from the render, not from a font metric: the widest
/// label the shipped translations produce is `Производительность`, and the
/// rail has to hold it beside its icon without breaking the word. The label
/// wraps anyway (see `App::rail`), so a translation longer than this grows
/// the row to two lines instead of clipping — the width is a comfort target,
/// not the thing that prevents a cut-off.
pub const RAIL_WIDTH: f32 = 244.0;
/// Horizontal padding inside a rail entry.
pub const RAIL_PAD_X: f32 = 16.0;
/// Space between a rail entry's icon and its label.
pub const RAIL_ICON_GAP: f32 = 13.0;
/// Width of the Settings sub-navigation column.
///
/// The same reasoning as the rail, one step down: it holds six section names
/// in the longest locale, and the widest of those decides it. The labels wrap,
/// so a translation longer than this takes a second line instead of clipping.
pub const SECTION_WIDTH: f32 = 224.0;
/// Radius shared by the rail, the header bar and the cards.
pub const RADIUS: f32 = 22.0;
/// Radius of the smaller surfaces inside a card.
pub const RADIUS_INNER: f32 = 14.0;

// --------------------------------------------------------------------------- //
// Container styles                                                             //
// --------------------------------------------------------------------------- //

/// The window backdrop: a cold light in the top corner over a deep base, the
/// same night-sky wash the reference is drawn on.
///
/// Iced has no radial gradient and the light in the mockups clearly comes from
/// an angle, so the wash is a linear gradient at a steep diagonal. It is
/// painted once, on the root, and every translucent surface above it picks up
/// the same light for free.
pub fn backdrop(sem: &'static Semantics) -> impl Fn(&iced::Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Gradient(gradient(sem))),
        ..Default::default()
    }
}

pub fn gradient(sem: &'static Semantics) -> iced::Gradient {
    let linear = iced::gradient::Linear::new(Degrees(115.0))
        .add_stop(0.0, sem.bg_glow)
        .add_stop(0.42, sem.bg_mid)
        .add_stop(1.0, sem.bg_deep);
    iced::Gradient::Linear(linear)
}

/// The scrim under the hero's title and transport controls: transparent at
/// the top of the artwork, opaque enough at the bottom that any frame the
/// wallpaper shows leaves the text readable.
///
/// This is the one gradient in the app that is not the backdrop. It is built
/// from black rather than from a token because its job is to darken whatever
/// is behind it, and what is behind it is a video.
pub fn scrim_gradient(sem: &'static Semantics) -> iced::Gradient {
    let linear = iced::gradient::Linear::new(Degrees(90.0))
        .add_stop(0.0, black_a(0.0))
        .add_stop(0.45, black_a(0.18))
        .add_stop(1.0, black_a(if sem.dark { 0.86 } else { 0.72 }));
    iced::Gradient::Linear(linear)
}

/// A floating glass card: translucent fill, hairline daylight edge, soft drop
/// shadow. The surface every page section is built from.
pub fn card_style(sem: &'static Semantics) -> container::Style {
    container::Style {
        background: Some(Background::Color(sem.card_bg)),
        border: Border {
            color: sem.card_edge,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow {
            color: black_a(0.35),
            offset: Vector::new(0.0, 12.0),
            blur_radius: 34.0,
        },
        ..Default::default()
    }
}

/// The navigation rail: card material, shadowed toward the content.
pub fn rail_style(sem: &'static Semantics) -> container::Style {
    container::Style {
        background: Some(Background::Color(sem.rail_bg)),
        border: Border {
            color: sem.card_edge,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow {
            color: black_a(0.40),
            offset: Vector::new(-6.0, 12.0),
            blur_radius: 30.0,
        },
        ..Default::default()
    }
}

/// An inset row inside a card — slightly lighter than the card itself so the
/// list reads as rows cut into the glass rather than tiles stacked on it.
pub fn row_style(sem: &'static Semantics) -> container::Style {
    container::Style {
        background: Some(Background::Color(sem.row_bg)),
        border: Border {
            color: sem.card_edge,
            width: 1.0,
            radius: RADIUS_INNER.into(),
        },
        ..Default::default()
    }
}

/// The amber halo the reference puts behind the selected rail item and the
/// highlighted stat tile. It is the accent, blurred, not a border.
pub fn glow_style(sem: &'static Semantics) -> container::Style {
    container::Style {
        background: Some(Background::Color(sem.glow_accent)),
        border: Border {
            color: sem.accent_border,
            width: 1.0,
            radius: RADIUS_INNER.into(),
        },
        shadow: Shadow {
            color: sem.glow_accent,
            offset: Vector::new(0.0, 0.0),
            blur_radius: 26.0,
        },
        ..Default::default()
    }
}

/// A small pill: chips, the engine badge, the daemon status.
pub fn pill_style(sem: &'static Semantics, active: bool) -> container::Style {
    container::Style {
        background: Some(Background::Color(if active {
            sem.accent_soft
        } else {
            sem.pill_bg
        })),
        border: Border {
            color: if active {
                sem.accent_border
            } else {
                sem.pill_edge
            },
            width: 1.0,
            radius: 999.0.into(),
        },
        ..Default::default()
    }
}

// --------------------------------------------------------------------------- //
// Primitives                                                                   //
// --------------------------------------------------------------------------- //

/// A card with the standard inner padding, filled across its parent.
pub fn card<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    sem: &'static Semantics,
) -> Container<'a, M> {
    container(content)
        .padding(Padding::new(20.0))
        .width(Length::Fill)
        .style(move |_| card_style(sem))
}

/// A card that keeps its own height, for the hero surfaces where stretching
/// would smear the artwork.
pub fn card_fit<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    sem: &'static Semantics,
    padding: f32,
) -> Container<'a, M> {
    container(content)
        .padding(Padding::new(padding))
        .style(move |_| card_style(sem))
}

/// A rounded, clipped plate: the frame a thumbnail sits in.
///
/// Iced clips a container's contents to its radius but does not blur or shade
/// the image itself, so the radius is owned here and the image is left square.
/// Rounding both would double the inset and show a halo.
pub fn plate<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    sem: &'static Semantics,
    radius: f32,
) -> Container<'a, M> {
    container(content)
        .clip(true)
        .style(move |_| container::Style {
            background: Some(Background::Color(sem.row_bg)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: radius.into(),
            },
            ..Default::default()
        })
}

/// Section heading inside a card.
pub fn heading<M: 'static>(label: String, sem: &'static Semantics) -> Element<'static, M> {
    text(label).size(15).color(sem.text).into()
}

/// Heading with a leading glyph, the form the reference uses for card titles.
pub fn heading_icon<'a, M: 'a>(
    icon_name: &str,
    label: String,
    sem: &'static Semantics,
) -> Element<'a, M> {
    Row::new()
        .spacing(9)
        .align_y(Alignment::Center)
        .push(icon::<M>(icon_name, 14).color(sem.text_secondary))
        .push(text(label).size(15).color(sem.text))
        .into()
}

/// A quiet helper line under a heading.
pub fn caption<M: 'static>(label: String, sem: &'static Semantics) -> Element<'static, M> {
    text(label).size(11.5).color(sem.text_muted).into()
}

/// A filled circle of `size` logical pixels — the dot in a status pill.
pub fn dot<'a, M: 'a>(color: Color, size: f32) -> Element<'a, M> {
    container(Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 999.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A pill with a leading dot and an optional second line: the engine badge,
/// the daemon status, the MPV chip.
pub fn status_pill<'a, M: 'a>(
    label: String,
    sub: Option<String>,
    dot_color: Color,
    sem: &'static Semantics,
) -> Element<'a, M> {
    let mut col = Column::new()
        .spacing(1)
        .push(text(label).size(12.5).color(sem.text));
    if let Some(sub) = sub {
        col = col.push(text(sub).size(10.5).color(sem.text_muted));
    }
    container(
        Row::new()
            .spacing(9)
            .align_y(Alignment::Center)
            .push(dot::<M>(dot_color, 8.0))
            .push(col),
    )
    .padding(Padding {
        top: 9.0,
        right: 15.0,
        bottom: 9.0,
        left: 13.0,
    })
    .style(move |_| pill_style(sem, false))
    .into()
}

/// A separator that reads as a shadow line rather than a stroke.
pub fn divider<M: 'static>(sem: &'static Semantics) -> Element<'static, M> {
    rule::horizontal(1)
        .style(move |_| rule::Style {
            color: sem.divider,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: false,
        })
        .into()
}

/// A label/value pair on one line — the shape the detail cards are made of.
pub fn kv_row<'a, M: 'a>(label: String, value: String, sem: &'static Semantics) -> Element<'a, M> {
    Row::new()
        .align_y(Alignment::Center)
        .spacing(12)
        .push(
            text(label)
                .size(12.5)
                .color(sem.text_muted)
                .width(Length::Fill),
        )
        .push(text(value).size(12.5).color(sem.text))
        .into()
}

/// A title/hint pair with a trailing control: the settings row, the monitor
/// row, the toggle row. Everything in the reference that reads as a line of
/// text with something on the right is this shape.
pub fn line<'a, M: 'a>(
    title: String,
    hint: Option<String>,
    trailing: Element<'a, M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    let mut text_col = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .push(text(title).size(13.5).color(sem.text));
    if let Some(hint) = hint {
        text_col = text_col.push(text(hint).size(11.5).color(sem.text_muted));
    }
    Row::new()
        .spacing(14)
        .align_y(Alignment::Center)
        .push(text_col)
        .push(trailing)
        .into()
}

/// A `line` cut into the glass: the inset row inside a settings card.
pub fn inset<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    sem: &'static Semantics,
    vertical_padding: f32,
) -> Element<'a, M> {
    container(content)
        .width(Length::Fill)
        .padding(Padding {
            top: vertical_padding,
            right: 14.0,
            bottom: vertical_padding,
            left: 14.0,
        })
        .style(move |_| row_style(sem))
        .into()
}

/// A clickable wrapper for surfaces that are not buttons — a wallpaper card,
/// a category tile. `None` renders the content inert instead of dropping it.
pub fn clickable<'a, M: Clone + 'a>(content: Element<'a, M>, message: Option<M>) -> Element<'a, M> {
    match message {
        Some(message) => mouse_area(content).on_release(message).into(),
        None => content,
    }
}

/// A fixed-height spacer, named so the pages read as layout, not arithmetic.
pub fn gap<M: 'static>(height: f32) -> Element<'static, M> {
    Space::new().height(Length::Fixed(height)).into()
}

/// The amber square with the play triangle: the brand mark, the one element
/// that appears on every page.
pub fn brand_mark<'a, M: 'a>(sem: &'static Semantics, size: f32) -> Element<'a, M> {
    container(icon::<M>("play", size * 0.42))
        // `align_x`/`align_y`, not `center_x`: the latter sets the container's
        // own width, which would stretch the mark across the rail.
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(sem.accent_bg)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: (size * 0.32).into(),
            },
            shadow: Shadow {
                color: sem.accent_shadow,
                offset: Vector::new(0.0, 3.0),
                blur_radius: 14.0,
            },
            text_color: Some(sem.on_accent),
            ..Default::default()
        })
        .into()
}

// --------------------------------------------------------------------------- //
// Buttons                                                                      //
// --------------------------------------------------------------------------- //

/// What a button is made of, so one closure covers the whole app.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Amber solid, the page's primary action.
    Primary,
    /// Glass with a hairline edge.
    Secondary,
    /// Nothing until hovered — the kebab, the carousel arrows, window chrome.
    Icon,
    /// A rail entry.
    Nav,
    /// The selected rail entry, with the amber halo.
    NavActive,
    /// A filter chip.
    Chip,
    /// The selected filter chip.
    ChipActive,
    /// A square action tile on Home.
    Tile,
    /// The close control, the only chrome button that changes hue.
    ChromeClose,
    /// An icon that sits on a wallpaper frame rather than on the app's own
    /// surface. Its ink is fixed for the same reason the scrim is: what is
    /// behind it is a video, in both themes.
    IconOnArtwork,
}

pub fn button_style(
    sem: &'static Semantics,
    kind: Kind,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style + 'static {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let pressed = matches!(status, button::Status::Pressed);
        let (background, ink, edge, radius, border_width, shadow) = match (kind, hovered) {
            (Kind::Primary, _) => (
                Some(Background::Color(if pressed {
                    sem.accent_bg2
                } else {
                    sem.accent_bg
                })),
                sem.on_accent,
                Color::TRANSPARENT,
                13.0,
                0.0,
                Shadow {
                    color: sem.accent_shadow,
                    offset: Vector::new(0.0, 4.0),
                    blur_radius: 18.0,
                },
            ),
            (Kind::Secondary, false) => (
                Some(Background::Color(sem.surface)),
                sem.text,
                sem.border,
                13.0,
                1.0,
                Shadow::default(),
            ),
            (Kind::Secondary, true) => (
                Some(Background::Color(sem.surface_hover)),
                sem.text,
                sem.border_strong,
                13.0,
                1.0,
                Shadow::default(),
            ),
            (Kind::Icon, false) => (
                Some(Background::Color(Color::TRANSPARENT)),
                sem.text_secondary,
                Color::TRANSPARENT,
                999.0,
                0.0,
                Shadow::default(),
            ),
            (Kind::Icon, true) => (
                Some(Background::Color(sem.icon_hover)),
                sem.text,
                Color::TRANSPARENT,
                999.0,
                0.0,
                Shadow::default(),
            ),
            (Kind::IconOnArtwork, false) => (
                Some(Background::Color(Color::TRANSPARENT)),
                sem.on_artwork,
                Color::TRANSPARENT,
                999.0,
                0.0,
                Shadow::default(),
            ),
            (Kind::IconOnArtwork, true) => (
                // The same ink, faintly: a hover fill on a video frame has to
                // be lighter than whatever the frame is showing there, and
                // the only colour guaranteed to be is the one already used to
                // read on it.
                Some(Background::Color(Semantics::fade(sem.on_artwork, 0.16))),
                sem.on_artwork,
                Color::TRANSPARENT,
                999.0,
                0.0,
                Shadow::default(),
            ),
            (Kind::Nav, false) => (
                Some(Background::Color(Color::TRANSPARENT)),
                sem.text_secondary,
                Color::TRANSPARENT,
                RADIUS_INNER,
                0.0,
                Shadow::default(),
            ),
            (Kind::Nav, true) => (
                Some(Background::Color(sem.surface_hover)),
                sem.text,
                Color::TRANSPARENT,
                RADIUS_INNER,
                0.0,
                Shadow::default(),
            ),
            (Kind::NavActive, _) => (
                Some(Background::Color(sem.glow_accent)),
                sem.accent_text,
                sem.accent_border,
                RADIUS_INNER,
                1.0,
                Shadow {
                    color: sem.glow_accent,
                    offset: Vector::new(0.0, 0.0),
                    blur_radius: 22.0,
                },
            ),
            (Kind::Chip, false) => (
                Some(Background::Color(sem.pill_bg)),
                sem.text_secondary,
                sem.pill_edge,
                999.0,
                1.0,
                Shadow::default(),
            ),
            (Kind::Chip, true) => (
                Some(Background::Color(sem.surface_active)),
                sem.text,
                sem.border_strong,
                999.0,
                1.0,
                Shadow::default(),
            ),
            (Kind::ChipActive, _) => (
                Some(Background::Color(sem.accent_bg)),
                sem.on_accent,
                Color::TRANSPARENT,
                999.0,
                0.0,
                Shadow {
                    color: sem.accent_shadow,
                    offset: Vector::new(0.0, 3.0),
                    blur_radius: 14.0,
                },
            ),
            (Kind::Tile, false) => (
                Some(Background::Color(sem.row_bg)),
                sem.text,
                sem.card_edge,
                RADIUS_INNER,
                1.0,
                Shadow::default(),
            ),
            (Kind::Tile, true) => (
                Some(Background::Color(sem.surface_hover)),
                sem.text,
                sem.border_strong,
                RADIUS_INNER,
                1.0,
                Shadow::default(),
            ),
            (Kind::ChromeClose, false) => (
                Some(Background::Color(Color::TRANSPARENT)),
                sem.text_muted,
                Color::TRANSPARENT,
                9.0,
                0.0,
                Shadow::default(),
            ),
            (Kind::ChromeClose, true) => (
                Some(Background::Color(sem.danger_soft)),
                sem.danger,
                Color::TRANSPARENT,
                9.0,
                0.0,
                Shadow::default(),
            ),
        };
        button::Style {
            background,
            text_color: ink,
            border: Border {
                color: edge,
                width: border_width,
                radius: radius.into(),
            },
            shadow,
            ..Default::default()
        }
    }
}

fn button_sized<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    message: Option<M>,
    kind: Kind,
    sem: &'static Semantics,
    padding: Padding,
) -> Button<'a, M> {
    button(content)
        .padding(padding)
        .on_press_maybe(message)
        .style(button_style(sem, kind))
}

/// The amber action button.
pub fn primary<'a, M: Clone + 'a>(
    label: String,
    message: Option<M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        text(label).size(13),
        message,
        Kind::Primary,
        sem,
        Padding {
            top: 10.0,
            right: 18.0,
            bottom: 10.0,
            left: 18.0,
        },
    )
    .into()
}

/// The amber action button with a leading glyph.
pub fn primary_icon<'a, M: Clone + 'a>(
    glyph_name: &str,
    label: String,
    message: Option<M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(icon::<M>(glyph_name, 12))
            .push(text(label).size(13)),
        message,
        Kind::Primary,
        sem,
        Padding {
            top: 10.0,
            right: 18.0,
            bottom: 10.0,
            left: 18.0,
        },
    )
    .into()
}

/// A glass action button.
pub fn secondary<'a, M: Clone + 'a>(
    label: String,
    message: Option<M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        text(label).size(12.5),
        message,
        Kind::Secondary,
        sem,
        Padding {
            top: 9.0,
            right: 15.0,
            bottom: 9.0,
            left: 15.0,
        },
    )
    .into()
}

/// A glass action button with a leading glyph.
pub fn secondary_icon<'a, M: Clone + 'a>(
    glyph_name: &str,
    label: String,
    message: Option<M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(icon::<M>(glyph_name, 12))
            .push(text(label).size(12.5)),
        message,
        Kind::Secondary,
        sem,
        Padding {
            top: 9.0,
            right: 15.0,
            bottom: 9.0,
            left: 15.0,
        },
    )
    .into()
}

/// A round icon-only button.
pub fn icon_button<'a, M: Clone + 'a>(
    glyph_name: &str,
    message: Option<M>,
    size: f32,
    kind: Kind,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        icon::<M>(glyph_name, size),
        message,
        kind,
        sem,
        Padding {
            top: size * 0.5,
            right: size * 0.5,
            bottom: size * 0.5,
            left: size * 0.5,
        },
    )
    .into()
}

/// A square icon button on a tile background — the Home quick actions.
pub fn tile_button<'a, M: Clone + 'a>(
    glyph_name: &str,
    label: String,
    message: Option<M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        Column::new()
            .spacing(10)
            .align_x(Alignment::Center)
            .push(icon::<M>(glyph_name, 20).color(sem.text_secondary))
            .push(text(label).size(12.5).color(sem.text)),
        message,
        Kind::Tile,
        sem,
        Padding {
            top: 18.0,
            right: 12.0,
            bottom: 18.0,
            left: 12.0,
        },
    )
    .width(Length::Fill)
    .into()
}

/// One entry in a filter-chip row.
pub fn chip_button<'a, M: Clone + 'a>(
    label: String,
    active: bool,
    message: Option<M>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button_sized(
        text(label).size(12.5),
        message,
        if active { Kind::ChipActive } else { Kind::Chip },
        sem,
        Padding {
            top: 8.0,
            right: 16.0,
            bottom: 8.0,
            left: 16.0,
        },
    )
    .into()
}

/// A static chip: a fact, not a control.
pub fn chip<'a, M: 'a>(label: String, sem: &'static Semantics) -> Element<'a, M> {
    container(text(label).size(11).color(sem.text_secondary))
        .padding(Padding {
            top: 4.0,
            right: 10.0,
            bottom: 4.0,
            left: 10.0,
        })
        .style(move |_| pill_style(sem, false))
        .into()
}

/// The pill that carries the accent tint: mode and playback state on the hero.
pub fn accent_chip<'a, M: 'a>(label: String, sem: &'static Semantics) -> Element<'a, M> {
    container(text(label).size(11).color(sem.accent_text))
        .padding(Padding {
            top: 4.0,
            right: 10.0,
            bottom: 4.0,
            left: 10.0,
        })
        .style(move |_| pill_style(sem, true))
        .into()
}

/// A row of chips, the filter bar shape.
pub fn chip_row<'a, M: Clone + 'a>(
    entries: Vec<(String, bool, Option<M>)>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    let mut row = Row::new().spacing(8).align_y(Alignment::Center);
    for (label, active, message) in entries {
        row = row.push(chip_button(label, active, message, sem));
    }
    row.into()
}

/// The amber switch the reference draws: a wide pill, a light knob, and the
/// accent only when the setting is on.
///
/// Iced's own toggler is a grey box on both palettes, which reads as a
/// checkbox rather than a system switch, so the material is named here once.
pub fn toggler_style(
    sem: &'static Semantics,
) -> impl Fn(&iced::Theme, iced::widget::toggler::Status) -> iced::widget::toggler::Style + 'static
{
    use iced::widget::toggler;
    move |_theme, status| {
        let on = matches!(
            status,
            toggler::Status::Active { is_toggled: true }
                | toggler::Status::Hovered { is_toggled: true }
        );
        let hovered = matches!(status, toggler::Status::Hovered { .. });
        let disabled = matches!(status, toggler::Status::Disabled { .. });
        toggler::Style {
            background: if on {
                Background::Color(if disabled {
                    Semantics::fade(sem.accent_bg, 0.45)
                } else {
                    sem.accent_bg
                })
            } else {
                Background::Color(if hovered {
                    sem.surface_active
                } else {
                    sem.surface
                })
            },
            background_border_width: if on { 0.0 } else { 1.0 },
            background_border_color: sem.border_strong,
            // The knob is the one light thing in the control; on the dark
            // theme it is near white, on the light theme it is white with an
            // edge so it does not vanish into the track.
            foreground: Background::Color(if on && !disabled {
                sem.on_accent
            } else {
                sem.text
            }),
            foreground_border_width: if on { 0.0 } else { 1.0 },
            foreground_border_color: sem.border,
            text_color: None,
            // A pill, not a circle: the reference's switch is wider than it
            // is tall, which the default (round, no padding) cannot express.
            border_radius: Some(999.0.into()),
            padding_ratio: 0.14,
        }
    }
}

/// A row of mutually exclusive options, the shape under `Theme`, `Profile`
/// and `Hardware decoding`. The selected one is the amber chip; the rest are
/// quiet.
pub fn segmented<'a, M: Clone + 'a>(
    options: Vec<(String, bool, M)>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    let mut row = Row::new().spacing(8).align_y(Alignment::Center);
    for (label, active, message) in options {
        row = row.push(chip_button(label, active, Some(message), sem));
    }
    row.into()
}

// --------------------------------------------------------------------------- //
// Sparklines                                                                   //
// --------------------------------------------------------------------------- //

/// A filled area chart, the shape under every number on the Home and
/// Performance pages.
///
/// The series is drawn as a smooth curve — a quadratic through each sample, so
/// the line does not zig-zag between ticks — and closed to the bottom edge
/// with a translucent fill of the same hue. Values are normalised against the
/// series' own floor and `ceiling`, so a flat 6% CPU still reads as a line
/// rather than a stripe glued to the baseline.
pub struct Sparkline {
    /// One entry per tick, `None` where the tick produced no reading.
    pub values: Vec<Option<f64>>,
    pub color: Color,
    /// Upper bound the values are scaled against, in the same unit as the
    /// values. `None` scales to the data.
    pub ceiling: Option<f64>,
}

impl Sparkline {
    /// The chart at a fixed height. A sparkline has no intrinsic height — it
    /// is the tile that decides how much of itself is chart — so the caller
    /// always names it.
    pub fn block<'a, M: 'a>(self, height: f32) -> Element<'a, M> {
        canvas(SparklineProgram(self))
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }
}

struct SparklineProgram(Sparkline);

impl<M> canvas::Program<M> for SparklineProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let points = self.0.values.as_slice();
        if points.is_empty() {
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        // A little air at the crest, and the baseline never exactly on the
        // bottom edge: the fill should read as a body, not a stripe.
        let top = 3.0f32;
        let base = bounds.height;

        let measured: Vec<f64> = points.iter().filter_map(|value| *value).collect();
        if measured.is_empty() {
            return vec![frame.into_geometry()];
        }
        let ceiling = self
            .0
            .ceiling
            .unwrap_or_else(|| measured.iter().cloned().fold(f64::MIN, f64::max));
        let floor = measured
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min)
            .min(0.0);
        let span = (ceiling - floor).max(1e-6);

        let y_of = |value: f64| base - ((value - floor) / span) as f32 * (base - top);
        let x_of = |index: usize| (index as f32 / (points.len() - 1).max(1) as f32) * width;
        // The midpoint between two samples as the control point: the curve
        // passes through every sample and stays smooth where the deltas are
        // large, which is what a CPU trace looks like.
        let control = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);

        let color = self.0.color;
        let stroke = || {
            canvas::Stroke::default()
                .with_color(color)
                .with_width(2.0)
                .with_line_cap(canvas::LineCap::Round)
                .with_line_join(canvas::LineJoin::Round)
        };

        // Each unbroken run is its own curve. A tick with no reading ends the
        // run instead of being smoothed over: carrying the last value forward
        // would draw a plateau that never happened, and connecting across the
        // gap would draw a slope that never happened either.
        let mut start = 0usize;
        loop {
            while start < points.len() && points[start].is_none() {
                start += 1;
            }
            if start >= points.len() {
                break;
            }
            let mut end = start;
            while end < points.len() && points[end].is_some() {
                end += 1;
            }
            let run = |index: usize| Point::new(x_of(index), y_of(points[index].unwrap_or(0.0)));

            if end - start == 1 {
                // A lone sample is still a fact: draw it as a dot.
                let dot = canvas::Path::circle(run(start), 2.0);
                frame.fill(&dot, color);
            } else {
                let mut line = canvas::path::Builder::new();
                line.move_to(run(start));
                for index in start + 1..end {
                    line.quadratic_curve_to(control(run(index - 1), run(index)), run(index));
                }
                let mut area = canvas::path::Builder::new();
                area.move_to(Point::new(x_of(start), base));
                area.line_to(run(start));
                for index in start + 1..end {
                    area.quadratic_curve_to(control(run(index - 1), run(index)), run(index));
                }
                area.line_to(Point::new(x_of(end - 1), base));
                area.close();

                frame.fill(&area.build(), Semantics::fade(color, 0.22));
                frame.stroke(&line.build(), stroke());
            }
            start = end;
        }

        vec![frame.into_geometry()]
    }
}
