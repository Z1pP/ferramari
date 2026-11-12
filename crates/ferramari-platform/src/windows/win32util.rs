//! Small Win32 helpers shared by the Windows backend modules.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG,
    WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WNDPROC,
};

/// UTF-8 to a NUL-terminated UTF-16 buffer for the `W` APIs.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 from a fixed buffer, stopping at the first NUL.
pub fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// The raw value of an `HWND`, for handing a window to another thread.
/// `HWND` is `!Send` in the `windows` crate, but window handles themselves are
/// process-wide values and may be passed between threads.
pub fn hwnd_raw(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

/// Rebuild an `HWND` from [`hwnd_raw`].
pub fn hwnd_from_raw(value: isize) -> HWND {
    HWND(value as *mut core::ffi::c_void)
}

/// The module handle of the running executable, needed to register classes.
pub fn module_handle() -> windows::Win32::Foundation::HINSTANCE {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        windows::Win32::Foundation::HINSTANCE(module.0)
    }
}

/// Register a window class once per process. Re-registering the same name is
/// reported as success because `RegisterClassW` returns zero with
/// `ERROR_CLASS_ALREADY_EXISTS` on the second call, which is harmless here.
pub fn register_class(name: &str, wndproc: WNDPROC) -> anyhow::Result<()> {
    let name = wide(name);
    let class = WNDCLASSW {
        lpfnWndProc: wndproc,
        hInstance: module_handle(),
        lpszClassName: windows::core::PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    unsafe {
        if RegisterClassW(&class) == 0 {
            let error = GetLastError();
            const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
            if error.0 != ERROR_CLASS_ALREADY_EXISTS {
                anyhow::bail!("RegisterClassW failed: {}", error.0);
            }
        }
    }
    Ok(())
}

/// Create a window with a registered class. The caller owns the HWND and must
/// destroy it.
#[allow(clippy::too_many_arguments)]
pub fn create_window(
    class: &str,
    title: &str,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: Option<HWND>,
) -> anyhow::Result<HWND> {
    let class = wide(class);
    let title = wide(title);
    unsafe {
        CreateWindowExW(
            ex_style,
            windows::core::PCWSTR(class.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            parent,
            None,
            Some(module_handle()),
            None,
        )
        .map_err(|err| anyhow::anyhow!("CreateWindowExW failed: {err}"))
    }
}

/// A session-local named mutex held for as long as the value lives.
pub struct NamedMutex {
    handle: HANDLE,
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Outcome of claiming a named instance mutex.
pub enum MutexClaim {
    /// This process owns the name.
    Owned(NamedMutex),
    /// Another process already owns it.
    AlreadyRunning,
    /// The kernel object could not be created. Callers decide whether that is
    /// fatal; the server and the UI both continue rather than becoming
    /// unusable on a system where the API is blocked.
    Unavailable(windows::core::Error),
}

/// Claim `name`, a mutex in the caller's session namespace.
pub fn claim_named_mutex(name: &str) -> MutexClaim {
    let name = wide(name);
    match unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) } {
        Ok(handle) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                MutexClaim::AlreadyRunning
            } else {
                MutexClaim::Owned(NamedMutex { handle })
            }
        }
        Err(err) => MutexClaim::Unavailable(err),
    }
}

/// Pump messages until `WM_QUIT` (or the window is destroyed and the handler
/// posts it). Used by every backend thread that owns a window.
pub fn run_message_loop() {
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).into() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
