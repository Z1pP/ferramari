//! Single control window per interactive session.
//!
//! The server may ask to launch the UI more than once (tray activation, the
//! Start menu shortcut, the CLI), but a second window would just show the same
//! state twice. Linux holds a well-known session-bus name for this; on Windows
//! a session-local named mutex serves the same purpose because there is no bus.
//!
//! The handle must stay alive for the lifetime of the UI process. Dropping it
//! releases the name, so a later launch can open the window again.

use crate::win32util::{claim_named_mutex, MutexClaim, NamedMutex};

const UI_INSTANCE_MUTEX: &str = r"Local\io.github.ferramari.Ferramari.ui";

/// Ownership token for the control-window instance name.
pub struct UiInstance {
    _mutex: NamedMutex,
}

/// Claim the control-window instance.
///
/// `Ok(None)` means the kernel object could not be created. The caller should
/// keep running: a missing mutex is a reason to risk a duplicate window, not a
/// reason to leave the user without a control surface.
pub fn acquire() -> Result<Option<UiInstance>, String> {
    match claim_named_mutex(UI_INSTANCE_MUTEX) {
        MutexClaim::Owned(mutex) => Ok(Some(UiInstance { _mutex: mutex })),
        MutexClaim::AlreadyRunning => {
            Err("another Ferramari control window is already running".to_string())
        }
        MutexClaim::Unavailable(err) => {
            log::warn!("[GUI] Could not create the instance mutex: {err}");
            Ok(None)
        }
    }
}
