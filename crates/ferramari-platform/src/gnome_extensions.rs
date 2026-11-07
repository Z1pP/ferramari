//! GNOME Shell integration. Kept in the platform crate because it talks to
//! the desktop session over D-Bus; the core crate must stay OS-agnostic.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ferramari_core::paths;

/// GNOME Shell reports extension state as an integer; 1 means enabled.
/// GNOME Shell reports extension state as an integer; 1 means enabled.
pub fn gnome_extension_state(name: &str) -> Option<i32> {
    use zbus::blocking::{Connection, Proxy};
    let connection = Connection::session().ok()?;
    let proxy = Proxy::new(
        &connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    )
    .ok()?;
    let info: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
        proxy.call("GetExtensionInfo", &(name,)).ok()?;
    let state = info.get("state")?;
    owned_value_to_i32(state)
}

/// `org.gnome.Shell.Extensions` reports `state` as an integer in older
/// releases but as a double (`1.0`) in GNOME 50. Accept both, plus the other
/// integer flavours, instead of silently failing the workaround.
fn owned_value_to_i32(value: &zbus::zvariant::OwnedValue) -> Option<i32> {
    if let Ok(value) = i32::try_from(value) {
        return Some(value);
    }
    if let Ok(value) = u32::try_from(value) {
        return Some(value as i32);
    }
    if let Ok(value) = i64::try_from(value) {
        return Some(value as i32);
    }
    if let Ok(value) = u64::try_from(value) {
        return Some(value as i32);
    }
    if let Ok(value) = f64::try_from(value) {
        return Some(value as i32);
    }
    None
}

pub fn gnome_extension_installed(name: &str) -> bool {
    use zbus::blocking::{Connection, Proxy};
    let Ok(connection) = Connection::session() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(
        &connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    ) else {
        return false;
    };
    let installed: std::collections::HashMap<
        String,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    > = match proxy.call("ListExtensions", &()) {
        Ok(value) => value,
        Err(_) => return false,
    };
    installed.contains_key(name)
}

fn gnome_extension_call(method: &str, name: &str) {
    use zbus::blocking::{Connection, Proxy};
    let Ok(connection) = Connection::session() else {
        return;
    };
    let Ok(proxy) = Proxy::new(
        &connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    ) else {
        return;
    };
    if let Err(err) = proxy.call::<_, _, bool>(method, &(name,)) {
        log::debug!("[Utils] {method}({name}) failed: {err}");
    }
}

/// Desktop-icon extensions keep their icons above the wallpaper window.
/// Restarting them after the wallpaper is shown puts the icons back on top.
///
/// Runs on a worker thread: disabling and re-enabling an extension takes a
/// moment, and GNOME 50 leaves the extension `INACTIVE` if `EnableExtension`
/// arrives immediately after `DisableExtension`.
pub fn gnome_desktop_icon_workaround() {
    if !paths::is_gnome() {
        return;
    }
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        for extension in [
            "ding@rastersoft.com",
            "desktopicons-neo@darkdemon",
            "gtk4-ding@smedius.gitlab.com",
            "zorin-desktop-icons@zorinos.com",
        ] {
            let installed = gnome_extension_installed(extension);
            let state = gnome_extension_state(extension);
            log::debug!("[Utils] {extension}: installed={installed} state={state:?}");
            if installed && state == Some(1) {
                log::info!("[Utils] Apply workaround for {extension}");
                gnome_extension_call("DisableExtension", extension);
                std::thread::sleep(Duration::from_millis(700));
                gnome_extension_call("EnableExtension", extension);
                std::thread::sleep(Duration::from_millis(700));
                log::debug!(
                    "[Utils] {extension} state after workaround: {:?}",
                    gnome_extension_state(extension)
                );
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

