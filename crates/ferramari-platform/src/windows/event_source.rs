//! Windows desktop event integration.
//!
//! Watchers normalize Win32 notifications into the core command bus. Their
//! native windows and thread handles stay here and are released when the
//! watcher set is dropped.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use ferramari_core::platform::{PlatformEventSource, PlatformWatchers};
use ferramari_core::CommandSender;

use crate::{handlers, monitors, x11};

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPlatformEventSource;

struct WindowsWatchers {
    _session: Option<handlers::SessionWatcher>,
    _window: Option<x11::Watcher>,
    _monitors: Option<monitors::Watcher>,
}

impl PlatformWatchers for WindowsWatchers {}

impl PlatformEventSource for WindowsPlatformEventSource {
    fn start(
        &self,
        tx: CommandSender,
        idle_threshold: Arc<AtomicU64>,
    ) -> Box<dyn PlatformWatchers> {
        handlers::start_idle_watcher(tx.clone(), idle_threshold);
        // Lock/unlock, suspend/resume and charger transitions share one
        // notification window: three separate WTS registrations in one
        // process are legal but needlessly duplicate the message loop.
        let session = handlers::start_session_watcher(tx.clone());
        let window = x11::start(tx.clone());
        let monitors = monitors::start(tx);
        Box::new(WindowsWatchers {
            _session: session,
            _window: window,
            _monitors: monitors,
        })
    }
}
