//! Linux desktop event integration.
//!
//! Watchers normalize X11, D-Bus session and idle/power signals into the
//! core command bus. Their native connections and thread handles stay here.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use ferramari_core::platform::{PlatformEventSource, PlatformWatchers};
use ferramari_core::CommandSender;

use crate::{handlers, monitors, x11};

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPlatformEventSource;

struct LinuxWatchers {
    _window: Option<x11::Watcher>,
    _monitors: Option<monitors::Watcher>,
}

impl PlatformWatchers for LinuxWatchers {}

impl PlatformEventSource for LinuxPlatformEventSource {
    fn start(
        &self,
        tx: CommandSender,
        idle_threshold: Arc<AtomicU64>,
    ) -> Box<dyn PlatformWatchers> {
        handlers::start_screen_lock_watcher(tx.clone());
        handlers::start_end_session_watcher(tx.clone());
        handlers::start_sleep_watcher(tx.clone());
        handlers::start_logind_lock_watcher(tx.clone());
        handlers::start_idle_watcher(tx.clone(), idle_threshold);

        Box::new(LinuxWatchers {
            _window: x11::start(tx.clone()),
            _monitors: monitors::start(tx),
        })
    }
}
