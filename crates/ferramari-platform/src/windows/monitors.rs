//! Windows display discovery and hot-plug notifications.
//!
//! Geometry is reported in physical pixels (the server is per-monitor DPI
//! aware), with the effective DPI carried separately in `scale_factor`.
//! `name` is the connector (`DISPLAY1`, …), the key the config stores;
//! `identity.native_id` carries the monitor's device interface string, which
//! survives a connector reassignment better than the ordinal.

use std::sync::mpsc;

use ferramari_core::monitor::DisplayIdentity;
pub use ferramari_core::MonitorInfo;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW,
    GetDeviceCaps, GetMonitorInfoW, DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, HDC,
    HMONITOR, HORZSIZE, MONITORINFOEXW, VERTSIZE,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, PostMessageW, PostQuitMessage, MONITORINFOF_PRIMARY,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DISPLAYCHANGE,
};

use crate::win32util::{create_window, from_wide, hwnd_from_raw, hwnd_raw, register_class, wide};

/// One `EnumDisplayMonitors` hit.
struct RawMonitor {
    device: String,
    rect: RECT,
    primary: bool,
    dpi: u32,
    width_mm: u32,
    height_mm: u32,
    native_id: Option<String>,
}

unsafe extern "system" fn collect_monitor(
    handle: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    monitors.push(handle);
    TRUE
}

/// Physical panel size in millimetres as the display driver reports it.
/// `MONITORINFOEXW` has no millimetre fields, so the documented route is a
/// display DC for the device plus `HORZSIZE`/`VERTSIZE`. Drivers that do not
/// report a size return zero, which callers treat as unknown.
fn physical_size_mm(device: &str) -> (u32, u32) {
    let driver = wide("DISPLAY");
    let device = wide(device);
    let dc = unsafe {
        CreateDCW(
            PCWSTR(driver.as_ptr()),
            PCWSTR(device.as_ptr()),
            PCWSTR::null(),
            None,
        )
    };
    if dc.0.is_null() {
        return (0, 0);
    }
    let width = unsafe { GetDeviceCaps(Some(dc), HORZSIZE) }.max(0) as u32;
    let height = unsafe { GetDeviceCaps(Some(dc), VERTSIZE) }.max(0) as u32;
    unsafe {
        let _ = DeleteDC(dc);
    }
    (width, height)
}

fn device_details(hmonitor: HMONITOR) -> RawMonitor {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let rect = if unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo).as_bool() } {
        info.monitorInfo.rcMonitor
    } else {
        RECT::default()
    };
    let device = from_wide(&info.szDevice);
    let primary = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;

    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    let _ = unsafe { GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };

    let (width_mm, height_mm) = physical_size_mm(&device);
    let native_id = monitor_native_id(&device);

    RawMonitor {
        device,
        rect,
        primary,
        dpi: dpi_x.max(96),
        width_mm,
        height_mm,
        native_id,
    }
}

/// The monitor's PnP device interface (`MONITOR\DEL40AB\{...}`), from
/// `EnumDisplayDevicesW`. This is what ties a config key to one physical
/// panel; a cable swap can change `DISPLAY1`, not this string.
fn monitor_native_id(device: &str) -> Option<String> {
    let device = wide(device);
    let mut display = DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    let ok = unsafe { EnumDisplayDevicesW(PCWSTR(device.as_ptr()), 0, &mut display, 0).as_bool() };
    if !ok {
        return None;
    }
    let id = from_wide(&display.DeviceID);
    (!id.is_empty()).then_some(id)
}

/// Vertical refresh of the current mode, if the display driver reports one.
fn refresh_hz(device: &str) -> Option<f64> {
    let device = wide(device);
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe {
        EnumDisplaySettingsW(PCWSTR(device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode).as_bool()
    };
    if !ok || mode.dmDisplayFrequency <= 1 {
        return None;
    }
    Some(f64::from(mode.dmDisplayFrequency))
}

pub fn list() -> Vec<MonitorInfo> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM((&mut handles as *mut Vec<HMONITOR>) as isize),
        );
    }
    let raw: Vec<RawMonitor> = handles.into_iter().map(device_details).collect();

    let mut used = std::collections::HashSet::new();
    let mut monitors = Vec::new();
    for (index, monitor) in raw.iter().enumerate() {
        let connector = monitor
            .device
            .strip_prefix(r"\\.\")
            .unwrap_or(&monitor.device);
        let mut name = if connector.is_empty() {
            format!("DISPLAY{}", index + 1)
        } else {
            connector.to_string()
        };
        if !used.insert(name.clone()) {
            name = format!("{name} #{}", index + 1);
            used.insert(name.clone());
        }
        let scale_factor = ((monitor.dpi as f64 / 96.0).round() as i32).max(1);
        monitors.push(MonitorInfo {
            index: index as i32,
            name,
            x: monitor.rect.left,
            y: monitor.rect.top,
            width: (monitor.rect.right - monitor.rect.left).max(0),
            height: (monitor.rect.bottom - monitor.rect.top).max(0),
            scale_factor,
            refresh_hz: refresh_hz(&monitor.device),
            is_primary: monitor.primary,
            identity: DisplayIdentity {
                connector: Some(monitor.device.clone()).filter(|value| !value.is_empty()),
                edid: None,
                width_mm: monitor.width_mm,
                height_mm: monitor.height_mm,
                native_id: monitor.native_id.clone(),
            },
        });
    }
    if monitors.is_empty() {
        log::warn!("[Monitors] EnumDisplayMonitors reported no monitors");
    }
    monitors
}

/// Message-only watcher for desktop layout changes. Windows broadcasts
/// `WM_DISPLAYCHANGE` to top-level windows, so the hidden window must not be
/// `HWND_MESSAGE`.
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

const CLASS_NAME: &str = "FerramariDisplayWatcher";

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DISPLAYCHANGE => {
            if let Some(tx) = watcher_sender() {
                let _ = tx.send(ferramari_core::Command::MonitorsChanged);
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
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// The sender is stored in a `OnceLock` because a window procedure is a plain
/// C function and cannot capture. The app creates this watcher exactly once.
static SENDER: std::sync::OnceLock<ferramari_core::CommandSender> = std::sync::OnceLock::new();

fn watcher_sender() -> Option<&'static ferramari_core::CommandSender> {
    SENDER.get()
}

pub fn start(tx: ferramari_core::CommandSender) -> Option<Watcher> {
    let _ = SENDER.set(tx);
    let (ready_tx, ready_rx) = mpsc::channel::<Option<isize>>();
    let thread = std::thread::Builder::new()
        .name("ferramari-monitors".into())
        .spawn(move || {
            if let Err(err) = register_class(CLASS_NAME, Some(wndproc)) {
                log::warn!("[Monitors] Could not register the display watcher class: {err}");
                let _ = ready_tx.send(None);
                return;
            }
            match create_window(
                CLASS_NAME,
                "Ferramari display watcher",
                WINDOW_STYLE(0),
                WINDOW_EX_STYLE(0),
                0,
                0,
                0,
                0,
                None,
            ) {
                Ok(hwnd) => {
                    let _ = ready_tx.send(Some(hwnd_raw(hwnd)));
                    crate::win32util::run_message_loop();
                }
                Err(err) => {
                    log::warn!("[Monitors] Could not create the display watcher: {err}");
                    let _ = ready_tx.send(None);
                }
            }
        })
        .ok()?;
    let hwnd = hwnd_from_raw(
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .ok()??,
    );
    Some(Watcher {
        hwnd,
        thread: Some(thread),
    })
}

/// A display provider for the server bootstrap.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsDisplayProvider;

impl ferramari_core::platform::DisplayProvider for WindowsDisplayProvider {
    fn displays(&self) -> Vec<MonitorInfo> {
        list()
    }
}
