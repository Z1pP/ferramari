//! Platform-facing contracts shared by the media layer and OS adapters.
//!
//! These interfaces deliberately contain no X11, Win32, Cocoa, GTK, Iced or
//! D-Bus types. A platform backend owns native handles and their lifetime;
//! media receives only an embedding handle and keeps the surface alive.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::bus::CommandSender;
use crate::power::PowerStatus;
use crate::MonitorInfo;

/// How completely a backend can provide a capability in the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SupportLevel {
    /// The capability is available and implemented for this session.
    Supported,
    /// The capability is available only for a subset of windows/devices or
    /// has known limitations (for example, X11 clients under XWayland).
    Partial,
    /// The backend cannot provide this capability in the current session.
    #[default]
    Unsupported,
}

/// Capabilities detected for the active desktop session, not just the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlatformCapabilities {
    pub wallpaper_surface: SupportLevel,
    pub display_enumeration: SupportLevel,
    pub display_hotplug: SupportLevel,
    pub window_tracking: SupportLevel,
    pub session_lock: SupportLevel,
    pub power_events: SupportLevel,
    pub idle_detection: SupportLevel,
}

/// Enumerates connected displays using stable backend identities where available.
pub trait DisplayProvider {
    fn displays(&self) -> Vec<MonitorInfo>;
}

/// Reads the current power-supply state from the operating system.
pub trait PowerProvider {
    fn status(&self) -> PowerStatus;
}

/// Owns the lifetime of platform event subscriptions and watcher threads.
pub trait PlatformWatchers {}

/// Starts OS event sources and forwards normalized events to the core command bus.
pub trait PlatformEventSource {
    fn start(
        &self,
        tx: CommandSender,
        idle_threshold: Arc<AtomicU64>,
    ) -> Box<dyn PlatformWatchers>;
}

/// An OS-owned surface suitable for embedding the media renderer.
///
/// The embedding handle is intentionally opaque to core. Its interpretation
/// is owned by the selected media/platform adapter and must never be persisted.
pub trait WallpaperSurface {
    fn embedding_handle(&self) -> u64;
}

/// Creates and owns native wallpaper surfaces for a platform backend.
///
/// The returned object owns the native surface for as long as it is retained
/// by the media engine. Dropping it must release the native resources.
pub trait WallpaperSurfaceProvider {
    fn create(
        &self,
        monitor: &MonitorInfo,
    ) -> anyhow::Result<Box<dyn WallpaperSurface>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_to_explicitly_unsupported() {
        let capabilities = PlatformCapabilities::default();
        assert_eq!(capabilities.wallpaper_surface, SupportLevel::Unsupported);
        assert_eq!(capabilities.display_enumeration, SupportLevel::Unsupported);
        assert_eq!(capabilities.display_hotplug, SupportLevel::Unsupported);
        assert_eq!(capabilities.window_tracking, SupportLevel::Unsupported);
        assert_eq!(capabilities.session_lock, SupportLevel::Unsupported);
        assert_eq!(capabilities.power_events, SupportLevel::Unsupported);
        assert_eq!(capabilities.idle_detection, SupportLevel::Unsupported);
    }
}
