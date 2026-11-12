//! Windows session integrations: idle time, lock/unlock, suspend/resume and
//! logoff.
//!
//! One hidden top-level window receives `WM_WTSSESSION_CHANGE` (after a
//! successful `WTSRegisterSessionNotification`), `WM_POWERBROADCAST` and the
//! end-session messages. Idle time is polled with `GetLastInputInfo` because
//! Windows has no push notification for "the user has not touched anything for
//! N seconds".

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ferramari_core::power::PowerEventType;
use ferramari_core::session::{IdleState, LockSource};
use ferramari_core::{Command, CommandSender};
use windows::core::BOOL;
use windows::Win32::Foundation::{FALSE, HANDLE, HWND, LPARAM, LRESULT, TRUE, WPARAM};
use windows::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
    CTRL_SHUTDOWN_EVENT,
};
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, UnregisterPowerSettingNotification, HPOWERNOTIFY,
    POWERBROADCAST_SETTING,
};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::SystemServices::GUID_ACDC_POWER_SOURCE;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostMessageW, PostQuitMessage,
    SetWindowLongPtrW, DEVICE_NOTIFY_WINDOW_HANDLE, GWLP_USERDATA, PBT_APMRESUMEAUTOMATIC,
    PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, PBT_POWERSETTINGCHANGE, WM_CLOSE, WM_ENDSESSION,
    WM_NCDESTROY, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_WTSSESSION_CHANGE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};

use crate::win32util::{create_window, hwnd_from_raw, hwnd_raw, register_class};

/// Linux's idle watcher polls every 15 s; keep the same cadence so the Eco
/// Engine sees comparable latency on both platforms.
const IDLE_POLL: Duration = Duration::from_secs(15);
const SESSION_CLASS: &str = "FerramariSessionWatcher";

/// Consecutive failed probes before the idle state becomes `Unknown`.
const IDLE_MISSES_BEFORE_UNKNOWN: u8 = 4;

// ---------------------------------------------------------------------------
// Idle detection
// ---------------------------------------------------------------------------

/// Milliseconds since the last keyboard or mouse input, or `None` when the
/// call itself failed (a non-interactive session).
fn idle_millis() -> Option<u64> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info).as_bool() } {
        return None;
    }
    Some(unsafe { GetTickCount64() }.wrapping_sub(u64::from(info.dwTime)))
}

/// The same edge-reporting rule as the Linux watcher, kept separate from the
/// thread so it can be tested: report only state changes, and treat a few
/// failed probes as transient noise rather than "the user is active".
#[derive(Debug, Default)]
struct IdleTracker {
    reported: Option<IdleState>,
    misses: u8,
}

impl IdleTracker {
    fn poll(&mut self, reading: Option<u64>, threshold_millis: u64) -> Option<IdleState> {
        let state = match reading {
            Some(millis) => {
                self.misses = 0;
                IdleState::from_millis(Some(millis), threshold_millis)
            }
            None => {
                self.misses = self.misses.saturating_add(1);
                if self.misses < IDLE_MISSES_BEFORE_UNKNOWN {
                    return None;
                }
                IdleState::Unknown
            }
        };
        (self.reported != Some(state)).then(|| {
            self.reported = Some(state);
            state
        })
    }
}

pub fn start_idle_watcher(tx: CommandSender, threshold_secs: Arc<AtomicU64>) {
    std::thread::Builder::new()
        .name("ferramari-idle".to_string())
        .spawn(move || {
            let mut tracker = IdleTracker::default();
            loop {
                std::thread::sleep(IDLE_POLL);
                let threshold_ms = threshold_secs.load(Ordering::Relaxed).saturating_mul(1000);
                if let Some(state) = tracker.poll(idle_millis(), threshold_ms) {
                    if tx.send(Command::Idle(state)).is_err() {
                        break;
                    }
                }
            }
        })
        .ok();
}

// ---------------------------------------------------------------------------
// Console control events
// ---------------------------------------------------------------------------

/// Ctrl-C, Ctrl-Break, closing the console window, logoff and shutdown.
///
/// Windows runs the handler on a separate thread; the server keeps owning all
/// media state on its command thread, so the handler only posts `Quit`. The
/// long grace period only exists for the events after which Windows terminates
/// the process itself: it gives `App::shutdown` time to stop mpv and restore
/// the previous wallpaper instead of leaving a paused frame on the desktop.
static CONSOLE_SENDER: OnceLock<CommandSender> = OnceLock::new();

/// Route console control events into the command loop. Returns `false` when
/// the process has no console to attach to (a `windows_subsystem = "windows"`
/// build started from Explorer) or another handler was already installed.
pub fn install_console_handler(tx: CommandSender) -> bool {
    if CONSOLE_SENDER.set(tx).is_err() {
        return false;
    }
    match unsafe { SetConsoleCtrlHandler(Some(console_control), true) } {
        Ok(()) => true,
        Err(err) => {
            log::warn!("[Signal] Could not install the console control handler: {err}");
            false
        }
    }
}

unsafe extern "system" fn console_control(event: u32) -> BOOL {
    let Some(tx) = CONSOLE_SENDER.get() else {
        return FALSE;
    };
    if tx.send(Command::Quit).is_err() {
        // The command loop is already gone; let Windows take the default
        // action rather than pretending the event was handled.
        return FALSE;
    }
    match event {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {}
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            std::thread::sleep(Duration::from_millis(1500));
        }
        _ => {}
    }
    TRUE
}

// ---------------------------------------------------------------------------
// Lock, power and end-session notifications
// ---------------------------------------------------------------------------

/// Owns the hidden notification window and its message thread.
pub struct SessionWatcher {
    hwnd: HWND,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SessionWatcher {
    pub fn stop(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for SessionWatcher {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start the session notification window. `None` when the window class or
/// window cannot be created; the server then keeps working without lock and
/// suspend events, which the capability report states explicitly.
/// Everything the notification window owns. Freed on `WM_NCDESTROY`.
struct SessionState {
    tx: CommandSender,
    power: Option<HPOWERNOTIFY>,
}

pub fn start_session_watcher(tx: CommandSender) -> Option<SessionWatcher> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Option<isize>>();
    let thread = std::thread::Builder::new()
        .name("ferramari-session".to_string())
        .spawn(move || {
            let state = Box::into_raw(Box::new(SessionState { tx, power: None }));
            if let Err(err) = register_class(SESSION_CLASS, Some(wndproc)) {
                log::warn!("[Session] Could not register the notification window class: {err}");
                unsafe {
                    drop(Box::from_raw(state));
                }
                let _ = ready_tx.send(None);
                return;
            }
            let hwnd = create_window(
                SESSION_CLASS,
                "Ferramari session watcher",
                windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(0),
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                0,
                0,
                0,
                0,
                None,
            );
            match hwnd {
                Ok(hwnd) => {
                    unsafe {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                    }
                    if let Err(err) =
                        unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) }
                    {
                        // Session-end and power messages still arrive without
                        // WTS; only lock/unlock would be missing.
                        log::warn!("[Session] WTS lock notifications unavailable: {err}");
                    }
                    // Immediate charger plug/unplug instead of waiting for the
                    // next status poll. Failure is not fatal: the poll remains.
                    match unsafe {
                        RegisterPowerSettingNotification(
                            HANDLE(hwnd.0),
                            &GUID_ACDC_POWER_SOURCE,
                            DEVICE_NOTIFY_WINDOW_HANDLE,
                        )
                    } {
                        Ok(power) => {
                            if let Some(state) = unsafe { state.as_mut() } {
                                state.power = Some(power);
                            }
                        }
                        Err(err) => {
                            log::debug!("[Power] AC source notifications unavailable: {err}")
                        }
                    }
                    let _ = ready_tx.send(Some(hwnd_raw(hwnd)));
                    crate::win32util::run_message_loop();
                }
                Err(err) => {
                    log::warn!("[Session] Could not create the notification window: {err}");
                    unsafe {
                        drop(Box::from_raw(state));
                    }
                    let _ = ready_tx.send(None);
                }
            }
        })
        .ok()?;
    let hwnd = hwnd_from_raw(ready_rx.recv_timeout(Duration::from_secs(2)).ok()??);
    Some(SessionWatcher {
        hwnd,
        thread: Some(thread),
    })
}

fn sender(hwnd: HWND) -> Option<&'static CommandSender> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const SessionState;
    unsafe { pointer.as_ref().map(|state| &state.tx) }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_WTSSESSION_CHANGE => {
            let active = match wparam.0 as u32 {
                WTS_SESSION_LOCK => Some(true),
                WTS_SESSION_UNLOCK => Some(false),
                _ => None,
            };
            if let (Some(active), Some(tx)) = (active, sender(hwnd)) {
                let _ = tx.send(Command::LockChanged {
                    source: LockSource::SessionManager,
                    active,
                });
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            let event = wparam.0 as u32;
            let sleeping = match event {
                PBT_APMSUSPEND => Some(true),
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(false),
                PBT_POWERSETTINGCHANGE => {
                    if let Some(tx) = sender(hwnd) {
                        if let Some(connected) = ac_source_connected(lparam) {
                            let event = if connected {
                                PowerEventType::Connected
                            } else {
                                PowerEventType::Disconnected
                            };
                            let _ = tx.send(Command::PowerEvent(event));
                        }
                    }
                    None
                }
                _ => None,
            };
            // Vista+ expects TRUE for every power broadcast that is not a
            // specific power-setting registration.
            if let (Some(sleeping), Some(tx)) = (sleeping, sender(hwnd)) {
                let _ = tx.send(Command::Suspend(sleeping));
            }
            LRESULT(1)
        }
        WM_QUERYENDSESSION => {
            // Allow the logoff/shutdown; the quit itself arrives as
            // WM_ENDSESSION once every application has answered.
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                if let Some(tx) = sender(hwnd) {
                    let _ = tx.send(Command::Quit);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // The state was leaked into GWLP_USERDATA when the window was
            // created; take it back exactly once when the window dies.
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SessionState;
            if !pointer.is_null() {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let state = Box::from_raw(pointer);
                    if let Some(power) = state.power {
                        let _ = UnregisterPowerSettingNotification(power);
                    }
                    let _ = WTSUnRegisterSessionNotification(hwnd);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Decode `PBT_POWERSETTINGCHANGE` for the AC line.
///
/// `WM_POWERBROADCAST` carries a `POWERBROADCAST_SETTING` in `lParam`; only
/// the AC/DC GUID is registered, so a different setting means the message is
/// not ours. `Data[0]` is 0 on AC and 1 on battery.
fn ac_source_connected(lparam: LPARAM) -> Option<bool> {
    if lparam.0 == 0 {
        return None;
    }
    let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
    power_source_from(setting)
}

fn power_source_from(setting: &POWERBROADCAST_SETTING) -> Option<bool> {
    if setting.PowerSetting != GUID_ACDC_POWER_SOURCE || setting.DataLength < 1 {
        return None;
    }
    Some(setting.Data[0] == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_reading_is_reported() {
        let mut tracker = IdleTracker::default();
        assert_eq!(tracker.poll(Some(400_000), 300_000), Some(IdleState::Idle));
    }

    #[test]
    fn repeated_readings_only_report_edges() {
        let mut tracker = IdleTracker::default();
        assert_eq!(tracker.poll(Some(1_000), 300_000), Some(IdleState::Active));
        assert_eq!(tracker.poll(Some(2_000), 300_000), None);
        assert_eq!(tracker.poll(Some(400_000), 300_000), Some(IdleState::Idle));
        assert_eq!(tracker.poll(Some(500_000), 300_000), None);
        assert_eq!(tracker.poll(Some(1_000), 300_000), Some(IdleState::Active));
    }

    #[test]
    fn transient_failures_do_not_flap_the_state() {
        let mut tracker = IdleTracker::default();
        assert_eq!(tracker.poll(Some(1_000), 300_000), Some(IdleState::Active));
        for _ in 1..IDLE_MISSES_BEFORE_UNKNOWN {
            assert_eq!(tracker.poll(None, 300_000), None);
        }
        assert_eq!(tracker.poll(None, 300_000), Some(IdleState::Unknown));
        assert_eq!(tracker.poll(None, 300_000), None);
    }

    #[test]
    fn the_threshold_is_read_every_poll() {
        let mut tracker = IdleTracker::default();
        assert_eq!(tracker.poll(Some(400_000), 300_000), Some(IdleState::Idle));
        assert_eq!(
            tracker.poll(Some(400_000), 600_000),
            Some(IdleState::Active)
        );
    }

    #[test]
    fn the_live_idle_probe_returns_a_plausible_value() {
        if let Some(millis) = idle_millis() {
            assert!(millis < 86_400_000 * 10, "{millis}");
        }
    }

    #[test]
    fn an_ac_source_broadcast_maps_to_plugged_and_unplugged() {
        let mut setting = POWERBROADCAST_SETTING {
            PowerSetting: GUID_ACDC_POWER_SOURCE,
            DataLength: 1,
            Data: [0],
        };
        assert_eq!(power_source_from(&setting), Some(true));
        setting.Data[0] = 1;
        assert_eq!(power_source_from(&setting), Some(false));
    }

    #[test]
    fn broadcasts_for_other_settings_are_ignored() {
        let setting = POWERBROADCAST_SETTING {
            PowerSetting: windows::core::GUID::from_u128(0),
            DataLength: 1,
            Data: [0],
        };
        assert_eq!(power_source_from(&setting), None);
        let empty = POWERBROADCAST_SETTING {
            PowerSetting: GUID_ACDC_POWER_SOURCE,
            DataLength: 0,
            Data: [0],
        };
        assert_eq!(power_source_from(&empty), None);
    }
}
