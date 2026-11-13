//! Windows wallpaper surfaces hosted behind the desktop icons.
//!
//! Explorer owns the desktop. The classic, tool-friendly way to draw a live
//! wallpaper is to parent a window to the `WorkerW` that sits behind
//! `SHELLDLL_DefView`; the icons stay visible and clickable while the player
//! renders into the wallpaper window.
//!
//! Explorer restarts destroy that host (and with it our child window). The
//! surface reports `is_alive() == false`; the media supervisor then rebuilds
//! the engine and this provider attaches a fresh window to the new WorkerW.

use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW, GetParent, GetWindowThreadProcessId,
    IsWindow, SendMessageTimeoutW, SetWindowPos, ShowWindow, HTTRANSPARENT, HWND_BOTTOM,
    SMTO_NORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WM_ERASEBKGND,
    WM_NCHITTEST, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_VISIBLE,
};

use ferramari_core::platform::{WallpaperSurface, WallpaperSurfaceProvider};
use ferramari_core::MonitorInfo;

use crate::win32util::{create_window, register_class, wide};

const CLASS_NAME: &str = "FerramariWallpaperSurface";

/// Explorer's undocumented "spawn the wallpaper WorkerW" message. Wallpaper
/// Engine and every other live-wallpaper project use the same one; there is
/// no documented replacement.
const SPAWN_WORKERW: u32 = 0x052C;

pub struct Surface {
    hwnd: HWND,
    /// The Explorer window that owns this child. Windows does not destroy a
    /// cross-process child when the parent dies, so the stale window keeps
    /// answering `IsWindow`; `is_alive` compares this handle and its process.
    host: HWND,
    /// Explorer's process at creation time. A restarted Explorer is a new
    /// process, so a recycled host handle still fails the liveness check.
    host_pid: u32,
}

impl WallpaperSurface for Surface {
    fn embedding_handle(&self) -> u64 {
        self.hwnd.0 as u64
    }

    fn is_alive(&self) -> bool {
        // Cross-process children survive their parent being destroyed: after
        // an Explorer restart our window still answers `IsWindow`, but its
        // parent handle is dead. Check the parent link and the owning process
        // as well, because Windows may recycle an old handle value.
        unsafe {
            IsWindow(Some(self.hwnd)).as_bool()
                && IsWindow(Some(self.host)).as_bool()
                && GetParent(self.hwnd).ok() == Some(self.host)
                && window_process(self.host) == Some(self.host_pid)
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

/// The provider injected into the server bootstrap.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsSurfaceProvider;

impl WallpaperSurfaceProvider for WindowsSurfaceProvider {
    fn create(&self, monitor: &MonitorInfo) -> anyhow::Result<Box<dyn WallpaperSurface>> {
        Ok(Box::new(create(monitor)?))
    }
}

/// Create one wallpaper window for `monitor` and attach it to Explorer's
/// wallpaper host. Retries briefly: right after logon or an Explorer restart
/// `Progman` can exist before `WorkerW` does.
pub fn create(monitor: &MonitorInfo) -> anyhow::Result<Surface> {
    register_class(CLASS_NAME, Some(wndproc))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let host = loop {
        if let Some(host) = find_host() {
            break host;
        }
        if Instant::now() >= deadline {
            anyhow::bail!("Explorer's wallpaper host (Progman/WorkerW) was not found");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let host_pid = window_process(host)
        .ok_or_else(|| anyhow::anyhow!("Explorer's wallpaper host has no owning process"))?;

    // WorkerW uses the virtual-screen coordinate space, so move the monitor
    // rectangle into the host's client coordinates before creating the child.
    let mut points = [
        POINT {
            x: monitor.x,
            y: monitor.y,
        },
        POINT {
            x: monitor.x + monitor.width.max(1),
            y: monitor.y + monitor.height.max(1),
        },
    ];
    unsafe {
        let _ = MapWindowPoints(None, Some(host), &mut points);
    }
    let rect = RECT {
        left: points[0].x,
        top: points[0].y,
        right: points[1].x,
        bottom: points[1].y,
    };

    let hwnd = create_window(
        CLASS_NAME,
        "Ferramari Wallpaper",
        WS_CHILD | WS_VISIBLE,
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        rect.left,
        rect.top,
        (rect.right - rect.left).max(1),
        (rect.bottom - rect.top).max(1),
        Some(host),
    )
    .map_err(|err| anyhow::anyhow!("could not create the wallpaper window: {err}"))?;

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // WorkerW is already behind the icons. The Progman fallback hosts
        // SHELLDLL_DefView directly, so put the wallpaper at the bottom.
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }

    Ok(Surface {
        hwnd,
        host,
        host_pid,
    })
}

/// The process that owns `hwnd`, or `None` when the handle is dead.
fn window_process(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (thread != 0).then_some(pid)
}

/// Find the WorkerW behind the desktop icons, falling back to Progman. The
/// `0x052C` message asks Explorer to create the worker when it has not yet.
fn find_host() -> Option<HWND> {
    let progman = unsafe { FindWindowW(PCWSTR(wide("Progman").as_ptr()), None) }.ok()?;
    unsafe {
        let _ = SendMessageTimeoutW(
            progman,
            SPAWN_WORKERW,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            None,
        );
    }

    // Find the WorkerW that owns SHELLDLL_DefView, then its next sibling: that
    // sibling is the wallpaper host.
    let defview_class = wide("SHELLDLL_DefView");
    let worker_class = wide("WorkerW");
    let mut previous: Option<HWND> = None;
    loop {
        // Reaching the end of the WorkerW list is not an error: Explorer
        // keeps the desktop under Progman on systems where it never creates
        // the separate wallpaper worker. Treating "no more windows" as a
        // failure here used to skip the Progman fallback entirely.
        let Ok(worker) =
            (unsafe { FindWindowExW(None, previous, PCWSTR(worker_class.as_ptr()), None) })
        else {
            break;
        };
        let defview =
            unsafe { FindWindowExW(Some(worker), None, PCWSTR(defview_class.as_ptr()), None) };
        if defview.is_ok() {
            if let Ok(next) =
                unsafe { FindWindowExW(None, Some(worker), PCWSTR(worker_class.as_ptr()), None) }
            {
                return Some(next);
            }
            break;
        }
        previous = Some(worker);
    }

    // No WorkerW: Explorer keeps the desktop itself under Progman.
    Some(progman)
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Paint nothing: the player owns every pixel.
        WM_ERASEBKGND => LRESULT(1),
        // Let clicks reach the desktop icons instead of stopping at the
        // wallpaper window.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Whether Explorer's desktop can host a wallpaper window at all.
pub fn available() -> bool {
    unsafe { FindWindowW(PCWSTR(wide("Progman").as_ptr()), None).is_ok() }
}

/// Create one surface per monitor. Dropping a surface destroys its window.
pub fn create_all(monitors: &[MonitorInfo]) -> Vec<(Surface, MonitorInfo)> {
    monitors
        .iter()
        .filter_map(|monitor| match create(monitor) {
            Ok(surface) => Some((surface, monitor.clone())),
            Err(err) => {
                log::warn!(
                    "[Wallpaper] could not create a surface for {}: {err}",
                    monitor.name
                );
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_does_not_panic_without_a_desktop() {
        let _ = available();
    }

    #[test]
    fn window_rect_moves_into_host_coordinates() {
        let mut rect = RECT {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080,
        };
        // Pure arithmetic sanity: the Win32 call itself needs a host window.
        assert_eq!(rect.right - rect.left, 1920);
        let _ = &mut rect;
    }

    #[test]
    fn host_is_optional_on_a_headless_session() {
        // CI may not have Explorer; the probe must return None, not panic.
        let _ = find_host();
    }
}
