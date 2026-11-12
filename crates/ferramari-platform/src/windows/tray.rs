//! Windows notification-area icon.
//!
//! `Shell_NotifyIconW` needs a window that receives the icon's mouse messages,
//! so the tray owns a hidden top-level window and a message thread. The menu is
//! a plain Win32 popup menu: no extra crate, and the same commands as the Linux
//! StatusNotifierItem menu.

use std::sync::mpsc;
use std::time::Duration;

use ferramari_core::util::tr;
use ferramari_core::{Command, CommandSender};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DefWindowProcW, DestroyMenu, DestroyWindow, GetCursorPos,
    GetWindowLongPtrW, LoadIconW, PostMessageW, PostQuitMessage, SetForegroundWindow,
    SetWindowLongPtrW, TrackPopupMenu, GWLP_USERDATA, IDI_APPLICATION, MF_SEPARATOR, MF_STRING,
    TPM_RIGHTBUTTON, WM_CLOSE, WM_COMMAND, WM_LBUTTONUP, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use crate::win32util::{create_window, hwnd_from_raw, hwnd_raw, register_class, wide};

const CLASS_NAME: &str = "FerramariTray";
const TRAY_ID: u32 = 1;
/// `WM_APP` is the first message id applications may use.
const CALLBACK_MESSAGE: u32 = 0x8000 + 1;

const ID_SHOW: usize = 1;
const ID_TOGGLE_MUTE: usize = 2;
const ID_TOGGLE_PAUSE: usize = 3;
const ID_RELOAD: usize = 4;
const ID_LUCKY: usize = 5;
const ID_QUIT: usize = 7;

/// The live tray icon. `App` holds one so the "Run in background" switch can
/// remove and re-register the icon at runtime.
pub struct Handle {
    hwnd: HWND,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Returned by [`Handle::shutdown`]; `wait()` joins the tray message thread so
/// the icon is definitely gone before the setting is reported as applied.
pub struct Shutdown {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Shutdown {
    pub fn wait(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Handle {
    pub fn shutdown(mut self) -> Shutdown {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        Shutdown {
            thread: self.thread.take(),
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

struct TrayState {
    tx: CommandSender,
    // The icon definition is rebuilt for add/delete; only the window and the
    // sender need to survive.
}

/// Spawn the tray. `None` when the shell rejects the icon (no Explorer, a
/// locked-down notification area) or the window cannot be created.
pub fn spawn(tx: CommandSender) -> Option<Handle> {
    let (ready_tx, ready_rx) = mpsc::channel::<Option<isize>>();
    let thread = std::thread::Builder::new()
        .name("ferramari-tray".to_string())
        .spawn(move || {
            let state = Box::into_raw(Box::new(TrayState { tx }));
            if let Err(err) = register_class(CLASS_NAME, Some(wndproc)) {
                log::warn!("[Systray] Could not register the tray window class: {err}");
                unsafe {
                    drop(Box::from_raw(state));
                }
                let _ = ready_tx.send(None);
                return;
            }
            let hwnd = create_window(
                CLASS_NAME,
                "Ferramari",
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
                    if !add_icon(hwnd) {
                        log::warn!("[Systray] Shell_NotifyIconW rejected the icon");
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                        let _ = ready_tx.send(None);
                        return;
                    }
                    log::info!("[Systray] Ready");
                    let _ = ready_tx.send(Some(hwnd_raw(hwnd)));
                    crate::win32util::run_message_loop();
                }
                Err(err) => {
                    log::warn!("[Systray] Could not create the tray window: {err}");
                    unsafe {
                        drop(Box::from_raw(state));
                    }
                    let _ = ready_tx.send(None);
                }
            }
        })
        .ok()?;
    let hwnd = hwnd_from_raw(ready_rx.recv_timeout(Duration::from_secs(2)).ok()??);
    Some(Handle {
        hwnd,
        thread: Some(thread),
    })
}

fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: CALLBACK_MESSAGE,
        ..Default::default()
    };
    // A stock application icon is always present; shipping a custom icon file
    // is an installer concern, not a runtime lookup.
    if let Ok(icon) = unsafe { LoadIconW(None, IDI_APPLICATION) } {
        data.hIcon = icon;
    }
    copy_wide(&mut data.szTip, "Ferramari");
    data
}

fn add_icon(hwnd: HWND) -> bool {
    let data = notify_data(hwnd);
    unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() }
}

fn delete_icon(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn copy_wide(buffer: &mut [u16], value: &str) {
    let mut units = value.encode_utf16();
    for slot in buffer.iter_mut() {
        *slot = units.next().unwrap_or(0);
    }
    if let Some(last) = buffer.last_mut() {
        *last = 0;
    }
}

fn state(hwnd: HWND) -> Option<&'static mut TrayState> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut TrayState;
    unsafe { pointer.as_mut() }
}

fn send(hwnd: HWND, command: Command) {
    if let Some(state) = state(hwnd) {
        let _ = state.tx.send(command);
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        CALLBACK_MESSAGE => match lparam.0 as u32 {
            WM_LBUTTONUP => {
                send(hwnd, Command::ShowGui);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu(hwnd);
                LRESULT(0)
            }
            _ => LRESULT(0),
        },
        WM_COMMAND => {
            let id = wparam.0 & 0xFFFF;
            let command = match id {
                ID_SHOW => Some(Command::ShowGui),
                ID_TOGGLE_MUTE => Some(Command::ToggleMute),
                ID_TOGGLE_PAUSE => Some(Command::TogglePause),
                ID_RELOAD => Some(Command::Reload),
                ID_LUCKY => Some(Command::FeelingLucky),
                ID_QUIT => Some(Command::Quit),
                _ => None,
            };
            if let Some(command) = command {
                send(hwnd, command);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            delete_icon(hwnd);
            unsafe {
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut TrayState;
            if !pointer.is_null() {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(pointer));
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn show_menu(hwnd: HWND) {
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    let entries = [
        (tr("Show Ferramari"), ID_SHOW),
        (tr("Toggle Mute Audio"), ID_TOGGLE_MUTE),
        (tr("Toggle Play/Pause"), ID_TOGGLE_PAUSE),
        (tr("Reload"), ID_RELOAD),
        (tr("I'm Feeling Lucky"), ID_LUCKY),
    ];
    for (label, id) in entries {
        let label = wide(&label);
        unsafe {
            let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(label.as_ptr()));
        }
    }
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let label = wide(&tr("Quit Ferramari"));
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(label.as_ptr()));
    }

    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        // Required by the shell: without foreground ownership the menu stays
        // on screen after the user clicks elsewhere.
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
        unsafe {
            let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, Some(0), hwnd, None);
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
    unsafe {
        let _ = DestroyMenu(menu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_is_nul_terminated_and_truncated() {
        let mut buffer = [0xFFFFu16; 128];
        copy_wide(&mut buffer, "Ferramari");
        assert_eq!(buffer[0], u16::from(b'F'));
        assert_eq!(buffer[8], u16::from(b'i'));
        assert_eq!(buffer[9], 0);
    }

    #[test]
    fn an_overlong_tooltip_still_ends_with_a_nul() {
        let mut buffer = [0xFFFFu16; 8];
        copy_wide(&mut buffer, "a very long tooltip");
        assert_eq!(buffer[7], 0);
    }

    #[test]
    fn the_notify_data_names_this_process_icon() {
        let data = notify_data(HWND::default());
        assert_eq!(data.uID, TRAY_ID);
        assert_eq!(data.uCallbackMessage, CALLBACK_MESSAGE);
        assert!(data.uFlags.contains(NIF_MESSAGE));
    }

    #[test]
    fn spawning_the_tray_does_not_panic() {
        let (tx, _rx) = mpsc::channel::<Command>();
        let tx = CommandSender::new(tx);
        if let Some(handle) = spawn(tx) {
            handle.shutdown().wait();
        }
    }
}
