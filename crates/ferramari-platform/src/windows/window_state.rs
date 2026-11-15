//! Windows foreground/occlusion tracking.
//!
//! `EnumWindows` is sampled twice a second and normalized into the shared
//! window geometry model. A WinEvent hook would be event-driven, but it only
//! reports the events it is asked for: a window can cover the wallpaper by
//! moving, resizing, uncloaking or losing its maximized state, and a missed
//! event would leave playback paused behind a window that is gone. Sampling
//! the complete list cannot miss a transition, and the comparison below keeps
//! an unchanged desktop from producing any command.
//!
//! Windows that are invisible, minimized, cloaked by the shell (virtual
//! desktops, UWP suspension), tool windows or owned by this process are
//! excluded: none of them can cover a wallpaper the user is looking at.

pub use ferramari_core::window::{MonitorVisibility, WindowRect, WindowState};

use std::sync::mpsc;
use std::time::Duration;

use ferramari_core::{Command, CommandSender};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, KillTimer, PostMessageW,
    PostQuitMessage, SetTimer, GWL_EXSTYLE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_TIMER,
    WS_EX_TOOLWINDOW,
};

use crate::win32util::{create_window, hwnd_from_raw, hwnd_raw, register_class};

const CLASS_NAME: &str = "FerramariWindowWatcher";
const TIMER_ID: usize = 1;
/// 750 ms is short enough that a fullscreen game pauses the wallpaper before
/// the user notices a frame and long enough that the enumeration stays off
/// the CPU profile; the list is compared before anything is sent.
const SAMPLE_INTERVAL_MS: u32 = 750;

/// Shell windows that cover the desktop by design.
///
/// `Progman` and `WorkerW` are the desktop itself and the wallpaper host; when
/// they appear in `EnumWindows` their rectangles span the whole monitor, so
/// treating them as applications would pause every wallpaper on a healthy
/// desktop. The taskbar and Explorer's XAML island are excluded for the same
/// reason: they belong to the shell, not to a window the user put over the
/// wallpaper.
const SHELL_WINDOW_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "XamlExplorerHostIslandWindow_WASDK",
];

fn is_shell_class(class: &str) -> bool {
    SHELL_WINDOW_CLASSES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(class))
}

fn is_shell_window(hwnd: HWND) -> bool {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buffer[..length as usize]);
    is_shell_class(&class)
}

pub struct Watcher {
    hwnd: HWND,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    pub fn stop(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start sampling. `None` when the watcher window could not be created; the
/// server then keeps playing without occlusion detection.
pub fn start(tx: CommandSender) -> Option<Watcher> {
    let (ready_tx, ready_rx) = mpsc::channel::<Option<isize>>();
    let thread = std::thread::Builder::new()
        .name("ferramari-windows-state".to_string())
        .spawn(move || {
            let state = Box::into_raw(Box::new(WatcherState { tx, previous: None }));
            if let Err(err) = register_class(CLASS_NAME, Some(wndproc)) {
                log::warn!("[WindowHandler] Could not register the watcher class: {err}");
                unsafe {
                    drop(Box::from_raw(state));
                }
                let _ = ready_tx.send(None);
                return;
            }
            let hwnd = create_window(
                CLASS_NAME,
                "Ferramari window watcher",
                WINDOW_STYLE(0),
                WINDOW_EX_STYLE(0),
                0,
                0,
                0,
                0,
                None,
            );
            match hwnd {
                Ok(hwnd) => {
                    unsafe {
                        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                            hwnd,
                            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                            state as isize,
                        );
                    }
                    // Report the state that already exists at startup; the app
                    // starts from an empty window list, which is a default, not
                    // a measurement.
                    if let Some(state) = unsafe { state.as_mut() } {
                        let sample = sample();
                        state.previous = Some(sample.clone());
                        let _ = state.tx.send(Command::WindowState(sample));
                    }
                    if unsafe { SetTimer(Some(hwnd), TIMER_ID, SAMPLE_INTERVAL_MS, None) } == 0 {
                        log::warn!("[WindowHandler] Could not start the sampling timer");
                    }
                    let _ = ready_tx.send(Some(hwnd_raw(hwnd)));
                    crate::win32util::run_message_loop();
                }
                Err(err) => {
                    log::warn!("[WindowHandler] Could not create the watcher window: {err}");
                    unsafe {
                        drop(Box::from_raw(state));
                    }
                    let _ = ready_tx.send(None);
                }
            }
        })
        .ok()?;
    let hwnd = hwnd_from_raw(ready_rx.recv_timeout(Duration::from_secs(2)).ok()??);
    Some(Watcher {
        hwnd,
        thread: Some(thread),
    })
}

struct WatcherState {
    tx: CommandSender,
    previous: Option<WindowState>,
}

fn state(hwnd: HWND) -> Option<&'static mut WatcherState> {
    let pointer = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
        )
    } as *mut WatcherState;
    unsafe { pointer.as_mut() }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER if wparam.0 == TIMER_ID => {
            if let Some(state) = state(hwnd) {
                let sample = sample();
                if state.previous.as_ref() != Some(&sample) {
                    log::debug!("[WindowHandler] {} window(s)", sample.windows.len());
                    state.previous = Some(sample.clone());
                    let _ = state.tx.send(Command::WindowState(sample));
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_NCDESTROY => {
            let pointer = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                )
            } as *mut WatcherState;
            if !pointer.is_null() {
                unsafe {
                    windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                        hwnd,
                        windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
                        0,
                    );
                    drop(Box::from_raw(pointer));
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    let windows = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
    windows.push(hwnd);
    TRUE
}

/// One complete sample of the visible top-level windows.
fn sample() -> WindowState {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM((&mut handles as *mut Vec<HWND>) as isize),
        );
    }
    let monitors = crate::monitors::list();
    let own_pid = std::process::id();
    let mut windows = Vec::new();
    for hwnd in handles {
        if !unsafe { IsWindowVisible(hwnd).as_bool() } || unsafe { IsIconic(hwnd).as_bool() } {
            continue;
        }
        if is_shell_window(hwnd) {
            continue;
        }
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        if pid == own_pid {
            continue;
        }
        if unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            continue;
        }
        if cloaked(hwnd) {
            continue;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            continue;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            continue;
        }
        let maximized = unsafe { IsZoomed(hwnd).as_bool() };
        let fullscreen = !maximized
            && monitors.iter().any(|monitor| {
                rect.left <= monitor.x
                    && rect.top <= monitor.y
                    && rect.right >= monitor.x + monitor.width
                    && rect.bottom >= monitor.y + monitor.height
            });
        windows.push(WindowRect {
            x: rect.left,
            y: rect.top,
            width,
            height,
            maximized,
            fullscreen,
        });
    }
    // Stacking order and focus are irrelevant to coverage; sorting keeps a
    // focus change from looking like a new window list.
    windows.sort_by_key(|window| {
        (
            window.x,
            window.y,
            window.width,
            window.height,
            window.maximized,
            window.fullscreen,
        )
    });
    WindowState { windows }
}

/// A cloaked window still has a valid HWND and `IsWindowVisible` is true; the
/// shell hides it on another virtual desktop or while a UWP app is suspended.
fn cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_a_headless_session_does_not_panic() {
        let state = sample();
        // CI may have a desktop or nothing at all; either way the result must
        // be a plain, sortable list.
        let mut sorted = state.windows.clone();
        sorted.sort_by_key(|window| (window.x, window.y, window.width, window.height));
        assert_eq!(state.windows, sorted);
    }

    #[test]
    fn shell_classes_are_not_treated_as_application_windows() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("progman"));
        assert!(is_shell_class("WorkerW"));
        assert!(is_shell_class("Shell_TrayWnd"));
        assert!(is_shell_class("Shell_SecondaryTrayWnd"));
        assert!(is_shell_class("XamlExplorerHostIslandWindow_WASDK"));
        assert!(!is_shell_class("ApplicationFrameWindow"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
        assert!(!is_shell_class(""));
    }

    #[test]
    fn the_watcher_can_start_and_stop() {
        let (tx, _rx) = mpsc::channel::<Command>();
        let tx = CommandSender::new(tx);
        if let Some(watcher) = start(tx) {
            watcher.stop();
            drop(watcher);
        }
    }
}
