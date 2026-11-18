//! Single control window per interactive session.
//!
//! The server may ask to launch the UI more than once (tray activation, the
//! Start menu shortcut, the CLI), but a second window would just show the same
//! state twice. Linux holds a well-known session-bus name for this; on Windows
//! a session-local named mutex serves the same purpose because there is no bus.
//!
//! The handle must stay alive for the lifetime of the UI process. Dropping it
//! releases the name, so a later launch can open the window again.

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowTextW, IsIconic,
    IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, SW_RESTORE, SW_SHOW,
    WS_EX_TOOLWINDOW,
};

use crate::win32util::{claim_named_mutex, wide, MutexClaim, NamedMutex};

const UI_INSTANCE_MUTEX: &str = r"Local\io.github.ferramari.Ferramari.ui";

/// The native title Iced gives the control window. It is the product name, not
/// a translated page title, so it is stable across locales.
const UI_WINDOW_TITLE: &str = "Ferramari";

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

/// Raise the already-open control window instead of opening a second one.
///
/// A launch from the Start menu, a shortcut or the CLI goes through
/// `show_gui`, which spawns a new UI process. That process loses the instance
/// mutex and exits; without this call the existing window would stay wherever
/// it was, possibly minimized. Closing the window with the title-bar X hides
/// it on Windows, so a hidden window must be shown again here as well: the
/// tray and the second launch are the only handles the user has on it.
///
/// The caller is normally the freshly launched process, so Windows' foreground
/// lock usually lets the request through. If it does not, showing the window is
/// still useful and the taskbar button shows where it is.
pub fn focus_existing_window() -> bool {
    let Some(hwnd) = find_control_window(UI_WINDOW_TITLE) else {
        return false;
    };
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            // `SW_SHOW` keeps a maximized window maximized, unlike
            // `SW_RESTORE`, which would also undo the user's maximized state.
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = BringWindowToTop(hwnd);
        if SetForegroundWindow(hwnd).as_bool() {
            log::info!("[GUI] Raised the existing control window");
        } else {
            // Foreground rights belong to the process that received the last
            // input; a tray command or a second launch may show the window
            // without being allowed to activate it. The window is visible and
            // the caller must not spawn a duplicate just because activation
            // was refused.
            log::debug!("[GUI] Shown the control window but could not raise it");
        }
        true
    }
}

/// The first top-level window with `title` that is not a tool window.
///
/// Visibility is deliberately not part of the match: the control surface hides
/// itself when the user closes it, and that hidden window is exactly what a
/// second launch or the tray has to raise. The tray also calls itself
/// Ferramari, so tool windows are excluded instead; the display and window
/// watchers use longer titles and never collide with the product name.
fn find_control_window(title: &str) -> Option<HWND> {
    struct Search {
        title: Vec<u16>,
        found: Option<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = unsafe { &mut *(data.0 as *mut Search) };
        // Tool windows are the tray and the session watcher. They may share
        // the product name but must never be raised as the control surface.
        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return TRUE;
        }
        if is_terminal_window(hwnd) {
            return TRUE;
        }
        let mut buffer = [0u16; 64];
        let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        let length = usize::try_from(length).unwrap_or(0);
        if length > 0 && buffer[..length] == search.title[..search.title.len() - 1] {
            search.found = Some(hwnd);
            return BOOL(0);
        }
        TRUE
    }

    let mut search = Search {
        title: wide(title),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut search as *mut Search) as isize));
    }
    search.found
}

/// Console hosts also call their window `Ferramari` when they run the server.
/// They are not the Iced control surface and must not shadow it.
fn is_terminal_window(hwnd: HWND) -> bool {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buffer[..length as usize]);
    is_terminal_class(&class)
}

fn is_terminal_class(class: &str) -> bool {
    [
        "ConsoleWindowClass",
        "CASCADIA_HOSTING_WINDOW_CLASS",
        "PseudoConsoleWindow",
    ]
    .iter()
    .any(|terminal| class.eq_ignore_ascii_case(terminal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    use crate::win32util::{create_window, register_class};

    unsafe extern "system" fn test_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    #[test]
    fn a_visible_window_is_found_by_title() {
        const CLASS: &str = "FerramariUiInstanceTest";
        register_class(CLASS, Some(test_window_proc)).expect("register test class");
        let title = format!("Ferramari test {}", std::process::id());
        let control = create_window(
            CLASS,
            &title,
            WS_POPUP | WS_VISIBLE,
            Default::default(),
            -10_000,
            -10_000,
            10,
            10,
            None,
        )
        .expect("create test window");
        // The tray also calls itself Ferramari; it must not shadow the control
        // surface even when it sits above it in Z-order.
        let tool = create_window(
            CLASS,
            &title,
            WS_POPUP | WS_VISIBLE,
            WS_EX_TOOLWINDOW,
            -10_000,
            -10_000,
            10,
            10,
            None,
        )
        .expect("create tool window");
        assert_eq!(find_control_window(&title), Some(control));
        unsafe {
            let _ = DestroyWindow(tool);
            let _ = DestroyWindow(control);
        }
    }

    #[test]
    fn a_hidden_window_is_still_found_by_title() {
        const CLASS: &str = "FerramariUiInstanceHiddenTest";
        register_class(CLASS, Some(test_window_proc)).expect("register test class");
        let title = format!("Ferramari hidden test {}", std::process::id());
        // No WS_VISIBLE: this is what `window::Mode::Hidden` leaves behind.
        let control = create_window(
            CLASS,
            &title,
            WS_POPUP,
            Default::default(),
            -10_000,
            -10_000,
            10,
            10,
            None,
        )
        .expect("create hidden test window");
        assert_eq!(find_control_window(&title), Some(control));
        unsafe {
            let _ = DestroyWindow(control);
        }
    }

    #[test]
    fn terminal_classes_are_not_control_windows() {
        assert!(is_terminal_class("ConsoleWindowClass"));
        assert!(is_terminal_class("CASCADIA_HOSTING_WINDOW_CLASS"));
        assert!(is_terminal_class("PseudoConsoleWindow"));
        assert!(!is_terminal_class("Window Class"));
    }
}
