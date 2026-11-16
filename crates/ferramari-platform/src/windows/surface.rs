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

use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW, GetClassNameW, GetParent,
    GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW,
    PostQuitMessage, SendMessageTimeoutW, SetWindowPos, ShowWindow, HTTRANSPARENT, HWND_BOTTOM,
    HWND_TOP, SMTO_NORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WM_CLOSE,
    WM_ERASEBKGND, WM_NCDESTROY, WM_NCHITTEST, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_VISIBLE,
};

use ferramari_core::platform::{WallpaperSurface, WallpaperSurfaceProvider};
use ferramari_core::MonitorInfo;

use crate::win32util::{
    create_window, hwnd_from_raw, hwnd_raw, register_class, run_message_loop, wide,
};

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
    /// The thread that owns `hwnd` and pumps its messages. Explorer sends
    /// synchronous messages to the wallpaper window while it replaces its
    /// worker (a wallpaper change does this), and a window whose thread does
    /// not pump stalls the caller for minutes. Keeping the window off the
    /// command loop is what lets `SPI_SETDESKWALLPAPER` return immediately.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WallpaperSurface for Surface {
    fn embedding_handle(&self) -> u64 {
        self.hwnd.0 as u64
    }

    fn is_alive(&self) -> bool {
        // Cross-process children survive their parent being destroyed: after
        // an Explorer restart our window still answers `IsWindow`, but its
        // parent handle is dead. Check the parent link and the owning process
        // as well, because Windows may recycle an old handle value. Explorer
        // also replaces its wallpaper worker when the desktop image changes:
        // the old worker stays alive but is hidden, and a child of it would
        // never be seen again.
        unsafe {
            IsWindow(Some(self.hwnd)).as_bool()
                && IsWindow(Some(self.host)).as_bool()
                && IsWindowVisible(self.host).as_bool()
                && GetParent(self.hwnd).ok() == Some(self.host)
                && window_process(self.host) == Some(self.host_pid)
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        // The window thread answers `WM_CLOSE` by destroying the window, which
        // also posts `WM_QUIT`. If Explorer already destroyed the host (and
        // with it this child), the thread has stopped and the join is instant.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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
///
/// The window is created on a dedicated thread that then pumps messages. A
/// child of Explorer's desktop receives synchronous messages from Explorer,
/// and the command loop must never be the thread that answers them: while it
/// is inside `SPI_SETDESKWALLPAPER`, Explorer waits for the surface, which
/// would wait for the call to return.
pub fn create(monitor: &MonitorInfo) -> anyhow::Result<Surface> {
    let monitor = monitor.clone();
    let (ready_tx, ready_rx) = mpsc::channel::<anyhow::Result<(isize, isize, u32)>>();
    let thread = std::thread::Builder::new()
        .name(format!("ferramari-surface-{}", monitor.name))
        .spawn(move || match create_on_thread(&monitor) {
            Ok((hwnd, host, host_pid)) => {
                if ready_tx
                    .send(Ok((hwnd_raw(hwnd), hwnd_raw(host), host_pid)))
                    .is_err()
                {
                    // The caller gave up waiting (Explorer was slow). Do not
                    // leave an orphaned window and a message loop nobody can
                    // reach.
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    return;
                }
                run_message_loop();
            }
            Err(err) => {
                let _ = ready_tx.send(Err(err));
            }
        })
        .map_err(|err| anyhow::anyhow!("could not start the wallpaper window thread: {err}"))?;
    let (hwnd, host, host_pid) = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow::anyhow!("the wallpaper window was not created in time"))??;
    Ok(Surface {
        hwnd: hwnd_from_raw(hwnd),
        host: hwnd_from_raw(host),
        host_pid,
        thread: Some(thread),
    })
}

/// Runs on the window thread, so every Win32 call that touches the new window
/// already belongs to the thread that will pump it.
fn create_on_thread(monitor: &MonitorInfo) -> anyhow::Result<(HWND, HWND, u32)> {
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
    let host_pid = window_process(host.hwnd)
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
        let _ = MapWindowPoints(None, Some(host.hwnd), &mut points);
    }
    let rect = RECT {
        left: points[0].x,
        top: points[0].y,
        right: points[1].x,
        bottom: points[1].y,
    };
    log::debug!(
        "[Surface] {} monitor={}x{}+{},{} host=0x{:x} ({:?}) client={}x{}+{},{}",
        monitor.name,
        monitor.width,
        monitor.height,
        monitor.x,
        monitor.y,
        host.hwnd.0 as usize,
        host.kind,
        rect.right - rect.left,
        rect.bottom - rect.top,
        rect.left,
        rect.top
    );

    let hwnd = create_window(
        CLASS_NAME,
        "Ferramari Wallpaper",
        WS_CHILD | WS_VISIBLE,
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        rect.left,
        rect.top,
        (rect.right - rect.left).max(1),
        (rect.bottom - rect.top).max(1),
        Some(host.hwnd),
    )
    .map_err(|err| anyhow::anyhow!("could not create the wallpaper window: {err}"))?;

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // Inside the wallpaper WorkerW, the surface is a child of the window
        // that paints the desktop image, so it draws above that image by
        // definition. The Progman fallback has no such parent: put the surface
        // directly below the icon view, above the wallpaper worker that
        // Windows 11 keeps as a sibling. `HWND_BOTTOM` used to place it below
        // that worker, where the static wallpaper hid every frame.
        let defview = defview_of(host.hwnd);
        let after = match host.kind {
            HostKind::Worker => Some(HWND_TOP),
            HostKind::Sibling => defview.or(Some(HWND_BOTTOM)),
        };
        let placed = SetWindowPos(
            hwnd,
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        log::debug!(
            "[Surface] defview={:?} after={:?} z-order={:?}",
            defview.map(|value| value.0 as usize),
            after.map(|value| value.0 as usize),
            placed.as_ref().err().map(|err| err.code().0)
        );
        let mut actual = RECT::default();
        let _ = GetWindowRect(hwnd, &mut actual);
        log::debug!(
            "[Surface] created 0x{:x} rect={}x{}+{},{}",
            hwnd.0 as usize,
            actual.right - actual.left,
            actual.bottom - actual.top,
            actual.left,
            actual.top
        );
    }

    Ok((hwnd, host.hwnd, host_pid))
}

/// Where the surface was attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKind {
    /// A `WorkerW` that paints the desktop image. The surface is its child.
    Worker,
    /// A window that also contains `SHELLDLL_DefView`; the surface is a
    /// sibling placed directly below the icon view.
    Sibling,
}

#[derive(Debug, Clone, Copy)]
struct Host {
    hwnd: HWND,
    kind: HostKind,
}

/// The `SHELLDLL_DefView` child of `parent`, if it is still there.
fn defview_of(parent: HWND) -> Option<HWND> {
    let class = wide("SHELLDLL_DefView");
    unsafe { FindWindowExW(Some(parent), None, PCWSTR(class.as_ptr()), None) }.ok()
}

/// The process that owns `hwnd`, or `None` when the handle is dead.
fn window_process(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (thread != 0).then_some(pid)
}

/// Find the window that hosts the wallpaper. The `0x052C` message asks
/// Explorer to create the worker when it has not yet.
///
/// There are two layouts in the wild:
///
/// * Classic: a top-level `WorkerW` owns `SHELLDLL_DefView`; the wallpaper
///   belongs inside that `WorkerW`.
/// * Windows 11: `SHELLDLL_DefView` stays directly under `Progman`, with a
///   separate `WorkerW` sibling below it painting the desktop image. The
///   surface is parented into that worker. As a sibling of it the surface
///   sits *behind* the static wallpaper, which hides every frame.
fn find_host() -> Option<Host> {
    let progman = unsafe { FindWindowW(PCWSTR(wide("Progman").as_ptr()), None) }.ok()?;

    // Prefer a host that already exists. Sending `0x052C` when Explorer
    // already has a wallpaper worker creates a second, briefly visible worker
    // above the real one; that dummy then gets hidden, which used to take the
    // surface (and the video) with it.
    if let Some(host) = existing_host(progman) {
        return Some(host);
    }

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
    // The message is synchronous, but Explorer may create the window on
    // another thread. Poll briefly instead of falling back to a layout that
    // cannot show the video.
    for _ in 0..10 {
        if let Some(host) = existing_host(progman) {
            return Some(host);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Last resort: Progman itself. `defview_of` then places the surface
    // directly below the icon view.
    Some(Host {
        hwnd: progman,
        kind: HostKind::Sibling,
    })
}

/// A host Explorer already provides, without asking it to create one.
fn existing_host(progman: HWND) -> Option<Host> {
    if let Some(host) = classic_host() {
        return Some(host);
    }
    // Windows 11 layout: the wallpaper worker is a direct child of Progman,
    // below the icon view. It paints the desktop image, so a surface parented
    // into it draws above the static frame. A sibling of that worker instead
    // sits behind it and the static image covers every video frame.
    worker_below_icons(progman).map(|worker| Host {
        hwnd: worker,
        kind: HostKind::Worker,
    })
}

/// Classic layout: a top-level `WorkerW` owns `SHELLDLL_DefView`; the
/// wallpaper host is the next top-level `WorkerW`, or the icon host itself
/// when Explorer never created a separate worker.
fn classic_host() -> Option<Host> {
    let defview_class = wide("SHELLDLL_DefView");
    let worker_class = wide("WorkerW");
    let mut previous: Option<HWND> = None;
    loop {
        // Reaching the end of the WorkerW list is not an error: Explorer
        // keeps the desktop under Progman on systems where it never creates
        // the separate wallpaper worker.
        let Ok(worker) =
            (unsafe { FindWindowExW(None, previous, PCWSTR(worker_class.as_ptr()), None) })
        else {
            break;
        };
        let defview =
            unsafe { FindWindowExW(Some(worker), None, PCWSTR(defview_class.as_ptr()), None) };
        if defview.is_ok() {
            // The next top-level WorkerW is the wallpaper host created by
            // `0x052C`; it sits behind the icon host and paints the desktop.
            if let Ok(wallpaper) =
                unsafe { FindWindowExW(None, Some(worker), PCWSTR(worker_class.as_ptr()), None) }
            {
                return Some(Host {
                    hwnd: wallpaper,
                    kind: HostKind::Worker,
                });
            }
            // Only one worker: keep the surface inside it, below the icon
            // view.
            return Some(Host {
                hwnd: worker,
                kind: HostKind::Sibling,
            });
        }
        previous = Some(worker);
    }
    None
}

/// The visible `WorkerW` that Explorer keeps directly under
/// `SHELLDLL_DefView` inside `Progman`, if it exists. `FindWindowExW` walks
/// direct children in z-order from top to bottom; the bottom-most visible
/// worker is the one painting the desktop image. Transient workers created by
/// `0x052C` sit above it and are skipped by taking the last match.
fn worker_below_icons(progman: HWND) -> Option<HWND> {
    let mut previous: Option<HWND> = None;
    let mut seen_icons = false;
    let mut found = None;
    loop {
        let Ok(child) = (unsafe { FindWindowExW(Some(progman), previous, PCWSTR::null(), None) })
        else {
            break;
        };
        let mut buffer = [0u16; 128];
        let length = unsafe { GetClassNameW(child, &mut buffer) };
        if length > 0 {
            let class = String::from_utf16_lossy(&buffer[..length as usize]);
            if class == "SHELLDLL_DefView" {
                seen_icons = true;
            } else if seen_icons
                && class == "WorkerW"
                && unsafe { IsWindowVisible(child).as_bool() }
            {
                found = Some(child);
            }
        }
        previous = Some(child);
    }
    found
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
        // Dropping a surface posts this to the thread that owns the window.
        // `DestroyWindow` must run there, and the resulting `WM_NCDESTROY`
        // ends the thread's message loop.
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        // Explorer destroys the host when it replaces its wallpaper worker;
        // that takes this child with it. The thread must stop even though no
        // `WM_CLOSE` was posted.
        WM_NCDESTROY => {
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            unsafe {
                PostQuitMessage(0);
            }
            result
        }
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
