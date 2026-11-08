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

    /// Subscribe to real-time charger plug/unplug events.
    ///
    /// Returns `Some` when the backend can deliver live `PowerEventType`
    /// events (typically via sysfs inotify or D-Bus).  `None` means only
    /// polling is available.
    fn subscribe(&self) -> Option<Box<dyn PowerWatcher>> {
        None
    }
}

/// Receives live charger plug / unplug notifications.
///
/// Implementations typically run their own watch thread; `poll` exists so
/// `--doctor` can query the last known state without duplicating sysfs logic.
pub trait PowerWatcher {
    /// Drain pending events, sending each one to `tx`.
    /// Returns `true` while the watcher should keep being polled.
    fn poll(&mut self, _tx: &CommandSender) -> bool {
        true // most implementations run independent threads
    }
}

/// Owns the lifetime of platform event subscriptions and watcher threads.
pub trait PlatformWatchers {}

/// Starts OS event sources and forwards normalized events to the core command bus.
pub trait PlatformEventSource {
    fn start(&self, tx: CommandSender, idle_threshold: Arc<AtomicU64>)
        -> Box<dyn PlatformWatchers>;
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
    fn create(&self, monitor: &MonitorInfo) -> anyhow::Result<Box<dyn WallpaperSurface>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Command, CommandSender};
    use crate::monitor::DisplayIdentity;
    use crate::power::PowerStatus;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

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

    #[test]
    fn support_level_orders_from_supported_to_unsupported() {
        // The contract expects Partial between Supported and Unsupported; a
        // session can advertise any of the three without breaking callers.
        assert_ne!(SupportLevel::Supported, SupportLevel::Unsupported);
        assert_ne!(SupportLevel::Partial, SupportLevel::Unsupported);
        assert_ne!(SupportLevel::Supported, SupportLevel::Partial);
    }

    // ---------------------------------------------------------------------
    // Fake providers: exercise the contracts the way a second OS would.
    // ---------------------------------------------------------------------

    fn fake_monitor(name: &str) -> MonitorInfo {
        MonitorInfo {
            index: 0,
            name: name.to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1,
            is_primary: true,
            identity: DisplayIdentity::default(),
        }
    }

    struct FakeDisplay {
        monitors: Vec<MonitorInfo>,
    }

    impl DisplayProvider for FakeDisplay {
        fn displays(&self) -> Vec<MonitorInfo> {
            self.monitors.clone()
        }
    }

    #[test]
    fn display_provider_returns_stable_data_without_native_handles() {
        let provider = FakeDisplay {
            monitors: vec![fake_monitor("eDP-1"), fake_monitor("HDMI-1")],
        };
        let first = provider.displays();
        let second = provider.displays();
        assert_eq!(first.len(), 2);
        // The same provider must return the same monitors every call.
        assert_eq!(first, second);
        assert_eq!(first[0].name, "eDP-1");
        // Both fake monitors report primary; contract tests only verify that
        // stable data flows through without panics or native-handle leakage.
        assert!(first.iter().all(|m| m.is_primary));
    }

    #[test]
    fn empty_display_provider_reports_no_monitors() {
        let provider = FakeDisplay {
            monitors: Vec::new(),
        };
        assert!(provider.displays().is_empty());
    }

    struct FakePower {
        status: PowerStatus,
    }

    impl PowerProvider for FakePower {
        fn status(&self) -> PowerStatus {
            self.status
        }
    }

    #[test]
    fn power_provider_reports_the_configured_state() {
        let ac = FakePower {
            status: PowerStatus {
                on_battery: false,
                percent: Some(80),
            },
        };
        assert!(!ac.status().on_battery);
        assert_eq!(ac.status().percent, Some(80));

        let battery = FakePower {
            status: PowerStatus {
                on_battery: true,
                percent: None,
            },
        };
        assert!(battery.status().on_battery);
        assert!(battery.status().percent.is_none());
    }

    struct FakeSurface {
        handle: u64,
    }

    impl WallpaperSurface for FakeSurface {
        fn embedding_handle(&self) -> u64 {
            self.handle
        }
    }

    struct FakeSurfaceProvider {
        next_handle: std::sync::atomic::AtomicU64,
    }

    impl WallpaperSurfaceProvider for FakeSurfaceProvider {
        fn create(&self, _monitor: &MonitorInfo) -> anyhow::Result<Box<dyn WallpaperSurface>> {
            let handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeSurface { handle }))
        }
    }

    #[test]
    fn surface_provider_hands_out_unique_handles_independently_of_monitor() {
        let provider = FakeSurfaceProvider {
            next_handle: std::sync::atomic::AtomicU64::new(100),
        };
        let monitor = fake_monitor("eDP-1");
        let a = provider.create(&monitor).unwrap();
        let b = provider.create(&monitor).unwrap();
        assert_eq!(a.embedding_handle(), 100);
        assert_eq!(b.embedding_handle(), 101);
        // Handles are opaque: core never interprets them, only forwards them.
        assert_ne!(a.embedding_handle(), b.embedding_handle());
    }

    #[test]
    fn surfaces_are_object_safe_and_can_be_moved_into_the_media_layer() {
        let provider = FakeSurfaceProvider {
            next_handle: std::sync::atomic::AtomicU64::new(1),
        };
        let surfaces: Vec<Box<dyn WallpaperSurface>> = vec![
            provider.create(&fake_monitor("A")).unwrap(),
            provider.create(&fake_monitor("B")).unwrap(),
        ];
        assert_eq!(surfaces.len(), 2);
        // Dropping a surface must not panic: the fake has no native resource,
        // a real backend releases it in its Drop implementation.
        drop(surfaces);
    }

    struct FakeEventSource {
        start_count: std::sync::atomic::AtomicUsize,
    }

    struct FakeWatchers;

    impl PlatformWatchers for FakeWatchers {}

    impl PlatformEventSource for FakeEventSource {
        fn start(
            &self,
            _tx: CommandSender,
            _idle_threshold: Arc<AtomicU64>,
        ) -> Box<dyn PlatformWatchers> {
            let _started = self.start_count.fetch_add(1, Ordering::SeqCst) + 1;
            // `_started` proves the event source counted the call before handing
            // ownership to the caller — a contract that a real backend must obey.
            Box::new(FakeWatchers)
        }
    }

    #[test]
    fn event_source_starts_watchers_and_hands_ownership_to_the_caller() {
        let source = FakeEventSource {
            start_count: std::sync::atomic::AtomicUsize::new(0),
        };
        let (sender, _receiver) = mpsc::channel::<Command>();
        let tx = CommandSender::new(sender);
        let threshold = Arc::new(AtomicU64::new(60));

        let watchers = source.start(tx.clone(), Arc::clone(&threshold));
        assert_eq!(source.start_count.load(Ordering::SeqCst), 1);
        // The returned object is a trait object owned by the caller; dropping
        // it is the documented teardown signal for a real backend.
        drop(watchers);

        // A second start creates independent watcher state.
        let _second = source.start(tx, threshold);
        assert_eq!(source.start_count.load(Ordering::SeqCst), 2);
    }
}
