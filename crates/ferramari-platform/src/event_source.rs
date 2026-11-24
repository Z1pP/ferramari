//! Linux desktop event integration.
//!
//! Watchers normalize X11, D-Bus session and idle/power signals into the
//! core command bus. Their native connections and thread handles stay here.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use ferramari_core::platform::{PlatformEventSource, PlatformWatchers};
use ferramari_core::CommandSender;

use crate::{gnome_windows, handlers, monitors, power_events, x11};

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPlatformEventSource;

struct LinuxWatchers {
    _power: Option<power_events::InotifyPowerWatcher>,
    _session: handlers::SessionWatchers,
    /// X11 EWMH sees X11/XWayland windows and is always available.
    _window: Option<x11::Watcher>,
    /// On GNOME this adds native Wayland coverage; absent elsewhere.
    _native: Option<gnome_windows::Watcher>,
    _monitors: Option<monitors::Watcher>,
}

impl PlatformWatchers for LinuxWatchers {}

impl PlatformEventSource for LinuxPlatformEventSource {
    fn start(
        &self,
        tx: CommandSender,
        idle_threshold: Arc<AtomicU64>,
    ) -> Box<dyn PlatformWatchers> {
        let session = handlers::SessionWatchers::start(tx.clone(), idle_threshold);
        // Real-time charger detection (inotify on /sys/class/power_supply).
        let _power = power_events::InotifyPowerWatcher::start(tx.clone());
        // Both sources are merged in the app: X11 EWMH covers X11/XWayland
        // clients, the optional GNOME helper adds native Wayland windows. If
        // the helper stops, X11 coverage remains.
        let window = x11::start(tx.clone());
        let native = gnome_windows::start(tx.clone());

        Box::new(LinuxWatchers {
            _power: Some(_power),
            _session: session,
            _window: window,
            _native: native,
            _monitors: monitors::start(tx),
        })
    }
}
