//! Ferramari Iced UI layer.
//!
//! * [`client`] — a GTK-free client of the running wallpaper server: D-Bus on
//!   Linux, the loopback control channel on Windows,
//! * [`client_state`] — the transport-neutral snapshot every page reads,
//! * [`theme`] — the Liquid Glass palette shared by every page,
//! * [`style`] — the widget styles and primitives the pages compose from.

#[cfg(unix)]
pub mod client;
#[cfg(windows)]
#[path = "client_control.rs"]
pub mod client;
pub mod client_state;
pub mod style;
pub mod theme;
