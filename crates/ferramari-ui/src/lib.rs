//! Ferramari Iced UI layer.
//!
//! * [`client`] — a GTK-free D-Bus client of the running wallpaper server,
//! * [`theme`] — the Liquid Glass palette shared by every page,
//! * [`style`] — the widget styles and primitives the pages compose from.

pub mod client;
pub mod style;
pub mod theme;
