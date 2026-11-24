//! Ferramari platform layer: native wallpaper surfaces, display and session
//! watchers, tray, autostart, static wallpaper and the local control API.
//!
//! The crate is GTK-free on every target. Linux talks to XRandR and D-Bus;
//! Windows uses Win32 through the `windows` crate and a loopback JSON
//! transport for the control surface. The rest of the program only sees the
//! platform-neutral contracts from `ferramari_core::platform`.

pub mod static_frame;

#[cfg(target_os = "linux")]
pub mod autostart;
#[cfg(target_os = "linux")]
pub mod dbus;
#[cfg(target_os = "linux")]
pub mod desktop;
#[cfg(target_os = "linux")]
pub mod event_source;
#[cfg(target_os = "linux")]
pub mod gnome_bg;
#[cfg(target_os = "linux")]
pub mod gnome_extensions;
#[cfg(target_os = "linux")]
pub mod gnome_windows;
#[cfg(target_os = "linux")]
pub mod handlers;
#[cfg(target_os = "linux")]
pub mod monitors;
#[cfg(target_os = "linux")]
pub mod power;
#[cfg(target_os = "linux")]
pub mod power_events;
#[cfg(target_os = "linux")]
pub mod surface;
#[cfg(target_os = "linux")]
pub mod tray;
#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(target_os = "windows")]
#[path = "windows/autostart.rs"]
pub mod autostart;
#[cfg(target_os = "windows")]
#[path = "windows/control.rs"]
pub mod control;
#[cfg(target_os = "windows")]
#[path = "windows/dbus.rs"]
pub mod dbus;
#[cfg(target_os = "windows")]
#[path = "windows/desktop.rs"]
pub mod desktop;
#[cfg(target_os = "windows")]
#[path = "windows/event_source.rs"]
pub mod event_source;
#[cfg(target_os = "windows")]
#[path = "windows/gnome_extensions.rs"]
pub mod gnome_extensions;
#[cfg(target_os = "windows")]
#[path = "windows/handlers.rs"]
pub mod handlers;
#[cfg(target_os = "windows")]
#[path = "windows/monitors.rs"]
pub mod monitors;
#[cfg(target_os = "windows")]
#[path = "windows/power.rs"]
pub mod power;
#[cfg(target_os = "windows")]
#[path = "windows/surface.rs"]
pub mod surface;
#[cfg(target_os = "windows")]
#[path = "windows/tray.rs"]
pub mod tray;
#[cfg(target_os = "windows")]
#[path = "windows/ui_instance.rs"]
pub mod ui_instance;
#[cfg(target_os = "windows")]
#[path = "windows/wallpaper.rs"]
pub mod wallpaper;
#[cfg(target_os = "windows")]
#[path = "windows/win32util.rs"]
pub(crate) mod win32util;
#[cfg(target_os = "windows")]
#[path = "windows/window_state.rs"]
pub mod x11;

#[cfg(target_os = "linux")]
pub mod wallpaper {
    //! The Linux static wallpaper keeps its historic GNOME implementation.
    pub use super::gnome_bg::*;

    /// Platform-neutral name used by the server; on Linux this is GNOME's
    /// background.
    pub use super::gnome_bg::GnomeBackground as StaticBackground;
}

#[cfg(target_os = "windows")]
pub use surface::WindowsSurfaceProvider as DefaultSurfaceProvider;
/// Alias used by the server so provider wiring has one spelling per platform.
#[cfg(target_os = "linux")]
pub use surface::X11SurfaceProvider as DefaultSurfaceProvider;

#[cfg(target_os = "windows")]
pub use monitors::WindowsDisplayProvider as DefaultDisplayProvider;
#[cfg(target_os = "linux")]
pub use monitors::X11DisplayProvider as DefaultDisplayProvider;

#[cfg(target_os = "linux")]
pub use power::LinuxPowerProvider as DefaultPowerProvider;
#[cfg(target_os = "windows")]
pub use power::WindowsPowerProvider as DefaultPowerProvider;

#[cfg(target_os = "linux")]
pub use event_source::LinuxPlatformEventSource as DefaultPlatformEventSource;
#[cfg(target_os = "windows")]
pub use event_source::WindowsPlatformEventSource as DefaultPlatformEventSource;
