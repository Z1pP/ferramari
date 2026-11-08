//! Monitor discovery, GTK-free.
//!
//! The GDK-based implementation (`gdk::Monitor`) had to run on the GTK main
//! thread and could not be used from background code. This module reproduces
//! the same enumeration and, importantly, the same **naming** entirely from
//! XRandR through x11rb.
//!
//! Naming is the compatibility contract: `MonitorInfo.name` doubles as the
//! `data_source` key in the config, so it must survive the migration.
//!
//! GDK's X11 backend (gdk/x11/gdkscreen-x11.c, `init_randr15`) sets the model
//! to the RandR monitor's atom name — for modern Xorg that is the connector
//! name (`eDP-1`, `HDMI-1`, …). A single monitor therefore always keeps its
//! connector name as the key, which is exactly what existing configs contain.
//! Duplicates (two monitors with the same atom, e.g. a cloned output) are
//! disambiguated with the monitor index, mirroring the old belt-and-braces
//! rule.
//!
//! Geometry: RandR reports device pixels; GDK reports logical pixels
//! (device / `GDK_SCALE`). We apply the same division.
//!
//! Primary: RandR's `primary` flag, falling back to LVDS and then to the
//! first monitor — the same cascade GDK uses.
//!
//! Identity: on top of the name, each display carries the connector, its
//! physical size and the EDID property when the server exposes one. Core uses
//! those to keep config keys attached to the same physical screen; see
//! `ferramari_core::display_ids`. Under XWayland the compositor does not
//! forward EDID, so the connector and size are all there is — the code reads
//! the property and reports what it finds rather than assuming either case.

use std::collections::HashSet;

use ferramari_core::monitor::DisplayIdentity;
pub use ferramari_core::MonitorInfo;

/// The window scale GDK applies (`GDK_SCALE`), minimum 1.
fn window_scale() -> i32 {
    std::env::var("GDK_SCALE")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(1)
        .max(1)
}

/// One RandR monitor's raw facts, before naming.
struct RawMonitor {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    /// The RandR monitor atom name — the value GDK uses as the model.
    atom_name: String,
    is_primary: bool,
    identity: DisplayIdentity,
}

fn output_edid(
    connection: &impl x11rb::connection::Connection,
    output: x11rb::protocol::randr::Output,
) -> Option<Vec<u8>> {
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    for name in [b"EDID".as_slice(), b"EDID_DATA".as_slice()] {
        // `only_if_exists`: a zero atom means this server has no such
        // property, which is the XWayland case rather than an error.
        let Some(atom) = connection
            .intern_atom(true, name)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.atom)
            .filter(|atom| *atom != 0)
        else {
            continue;
        };
        let Some(reply) = connection
            .randr_get_output_property(
                output,
                atom,
                x11rb::protocol::xproto::AtomEnum::ANY,
                0,
                1024,
                false,
                false,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            continue;
        };
        // Servers store EDID either as 8-bit bytes or as 32-bit words that
        // each carry one byte, so both formats are accepted.
        let bytes: Vec<u8> = match reply.format {
            8 => reply.data.clone(),
            // Word format: one EDID byte in the low byte of each item.
            32 => reply.data.iter().copied().step_by(4).collect(),
            _ => Vec::new(),
        };
        if !bytes.is_empty() {
            return Some(bytes);
        }
    }
    None
}

/// The connector name of an output, which is the stable part of a RandR
/// monitor's identity (`eDP-1`, `HDMI-1`, …).
fn output_name(
    connection: &impl x11rb::connection::Connection,
    output: x11rb::protocol::randr::Output,
    timestamp: u32,
) -> Option<String> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let info = connection
        .randr_get_output_info(output, timestamp)
        .ok()?
        .reply()
        .ok()?;
    let name = String::from_utf8_lossy(&info.name).into_owned();
    (!name.is_empty()).then_some(name)
}

fn randr_monitors() -> (Vec<RawMonitor>, Option<i32>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::ConnectionExt as _;

    let Ok((connection, screen)) = x11rb::connect(None) else {
        return (Vec::new(), None);
    };
    let root = connection.setup().roots[screen].root;
    let Some(reply) = connection
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    else {
        return (Vec::new(), None);
    };
    let scale = window_scale();
    // Connector names and EDID live on the outputs, so one resources query
    // feeds every monitor. `GetScreenResourcesCurrent` is the cheap variant:
    // it returns the cached layout instead of re-probing.
    let resources = connection
        .randr_get_screen_resources_current(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    let config_timestamp = resources
        .as_ref()
        .map(|resources| resources.config_timestamp)
        .unwrap_or(0);

    let monitors: Vec<RawMonitor> = reply
        .monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            // The RandR monitor carries an atom naming it; on modern Xorg that
            // is the connector name (eDP-1, …). Atom 0 means unspecified.
            let atom_name = if monitor.name != 0 {
                connection
                    .get_atom_name(monitor.name)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("Monitor {}", index + 1))
            } else {
                format!("Monitor {}", index + 1)
            };
            let identity = DisplayIdentity {
                // The monitor's own outputs, in the order the server reports
                // them. A clone shares one output between two monitors, which
                // is why identity is only trusted per attached output.
                connector: monitor
                    .outputs
                    .first()
                    .and_then(|output| output_name(&connection, *output, config_timestamp)),
                edid: monitor
                    .outputs
                    .first()
                    .and_then(|output| output_edid(&connection, *output)),
                width_mm: monitor.width_in_millimeters,
                height_mm: monitor.height_in_millimeters,
                native_id: None,
            };
            RawMonitor {
                x: i32::from(monitor.x) / scale,
                y: i32::from(monitor.y) / scale,
                width: i32::from(monitor.width) / scale,
                height: i32::from(monitor.height) / scale,
                atom_name,
                is_primary: monitor.primary,
                identity,
            }
        })
        .collect();

    // Primary cascade, mirroring GDK: the RandR primary flag first, then an
    // LVDS built-in (laptops without a primary flag), then the first monitor.
    let primary_index = monitors
        .iter()
        .position(|monitor| monitor.is_primary)
        .or_else(|| {
            monitors
                .iter()
                .position(|monitor| monitor.atom_name.to_ascii_uppercase().starts_with("LVDS"))
        })
        .map(|index| index as i32);

    (monitors, primary_index)
}

/// List the connected monitors in RandR order with stable, config-safe names.
pub fn list() -> Vec<MonitorInfo> {
    let (raw, primary_index) = randr_monitors();
    let scale = window_scale();

    let mut used = HashSet::new();
    let mut monitors = Vec::new();
    for (index, raw) in raw.iter().enumerate() {
        let mut name = raw.atom_name.clone();
        if name.is_empty() {
            name = format!("Monitor {}", index + 1);
        }
        // Belt and braces: never hand out two identical keys.
        if !used.insert(name.clone()) {
            name = format!("{} #{}", name, index + 1);
            used.insert(name.clone());
        }
        monitors.push(MonitorInfo {
            index: index as i32,
            name,
            x: raw.x,
            y: raw.y,
            width: raw.width,
            height: raw.height,
            scale_factor: scale,
            is_primary: Some(index as i32) == primary_index,
            identity: raw.identity.clone(),
        });
    }
    if monitors.is_empty() {
        log::warn!("[Monitors] RandR reported no monitors");
    }
    monitors
}

/// A hot-plug watcher built on RandR events, replacing the GDK
/// `connect_monitor_added` / `connect_monitor_removed` /
/// `connect_size_changed` trio.
///
/// The X11 server coalesces RandR notifications; any of the output/crtc/
/// screen-change flavours that can follow a hot-plug is forwarded as a
/// single `MonitorsChanged` command, and the app's own debouncing (which it
/// already applies to that command) smooths the rest.
///
/// Returns `None` when no X server answers at all.
pub struct Watcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Watcher {
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn start(tx: ferramari_core::CommandSender) -> Option<Watcher> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::randr::{ConnectionExt as _, Notify, NotifyMask};

    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots[screen].root;
    let mask = NotifyMask::SCREEN_CHANGE
        | NotifyMask::CRTC_CHANGE
        | NotifyMask::OUTPUT_CHANGE
        | NotifyMask::RESOURCE_CHANGE;
    connection
        .randr_select_input(root, mask)
        .ok()?
        .check()
        .ok()?;

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = std::sync::Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("ferramari-monitors".into())
        .spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                match connection.wait_for_event() {
                    Ok(event) => {
                        use x11rb::protocol::Event;
                        let relevant = matches!(event, Event::RandrScreenChangeNotify(_))
                            || matches!(
                                event,
                                Event::RandrNotify(notify)
                                    if matches!(
                                        notify.sub_code,
                                        Notify::CRTC_CHANGE
                                            | Notify::OUTPUT_CHANGE
                                            | Notify::RESOURCE_CHANGE
                                    )
                            );
                        if relevant {
                            let _ = tx.send(ferramari_core::Command::MonitorsChanged);
                        }
                    }
                    Err(err) => {
                        log::debug!("[Monitors] watcher disconnected: {err}");
                        return;
                    }
                }
            }
        })
        .ok()?;

    Some(Watcher {
        stop,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn scale_falls_back_to_one_when_unset() {
        std::env::remove_var("GDK_SCALE");
        assert_eq!(super::window_scale(), 1);
    }
}

/// XRandR-backed display provider for Linux X11/XWayland sessions.
#[derive(Debug, Clone, Copy, Default)]
pub struct X11DisplayProvider;

impl ferramari_core::platform::DisplayProvider for X11DisplayProvider {
    fn displays(&self) -> Vec<MonitorInfo> {
        list()
    }
}
