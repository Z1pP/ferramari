//! Ferramari platform layer: system tray, D-Bus interfaces, session watchers,
//! monitor enumeration, autostart, and the GNOME static background.
//!
//! This crate is GTK-free. Monitor discovery uses XRandR through x11rb;
//! everything that GTK used to provide here (single-instance D-Bus, tray,
//! session events, gsettings access) is reimplemented on the native protocols.

pub mod autostart;
pub mod dbus;
pub mod desktop;
pub mod event_source;
pub mod gnome_bg;
pub mod gnome_extensions;
pub mod handlers;
pub mod monitors;
pub mod power;
pub mod tray;
pub mod surface;
pub mod x11;
