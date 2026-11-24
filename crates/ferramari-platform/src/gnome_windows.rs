//! Optional GNOME Shell window-state helper.
//!
//! Native Wayland toplevels are invisible to EWMH and GNOME does not expose a
//! public window-list API to ordinary session processes. The helper shipped in
//! `data/gnome-shell/` runs inside GNOME Shell and answers one small D-Bus
//! question per poll: which monitors are covered or fullscreen. It carries no
//! titles, PIDs or application identifiers.
//!
//! The integration is optional and additive: the X11 EWMH watcher keeps
//! covering X11/XWayland clients, and this helper adds native Wayland windows.

use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use ferramari_core::paths;
use ferramari_core::util;
use ferramari_core::window::{MonitorVisibility, NativeMonitorState};
use ferramari_core::{Command, CommandSender};
use serde::Deserialize;

pub const UUID: &str = "ferramari-window-state@ferramari.github.io";
pub const SERVICE: &str = "io.github.ferramari.Ferramari.WindowState";
pub const PATH: &str = "/io/github/ferramari/Ferramari/WindowState";
pub const INTERFACE: &str = "io.github.ferramari.Ferramari.WindowState";

const METADATA: &str = include_str!(
    "../../../data/gnome-shell/ferramari-window-state@ferramari.github.io/metadata.json"
);
const EXTENSION: &str = include_str!(
    "../../../data/gnome-shell/ferramari-window-state@ferramari.github.io/extension.js"
);
const POLL: Duration = Duration::from_millis(750);

/// Whether the helper currently owns its session-bus name.
pub fn available() -> bool {
    use zbus::blocking::{Connection, Proxy};
    let Ok(connection) = Connection::session() else {
        return false;
    };
    let Ok(proxy) = Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return false;
    };
    proxy
        .call::<_, _, bool>("NameHasOwner", &(SERVICE,))
        .unwrap_or(false)
}

/// Poll the Shell helper and forward per-monitor coverage to the command loop.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
        crate::x11::join_bounded("GNOME window helper", &mut self.thread);
    }
}

/// Start the helper watcher. Returns `None` on non-GNOME sessions or when the
/// service is not installed/enabled; the X11 watcher runs regardless.
pub fn start(tx: CommandSender) -> Option<Watcher> {
    if !paths::is_gnome() {
        return None;
    }
    // A name owner is not enough: a broken or half-loaded extension must fall
    // back to the X11 watcher instead of leaving the app without window state.
    let initial = probe_state()?;
    log::debug!("[WindowHandler] GNOME helper initial state: {initial:?}");
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("ferramari-gnome-windows".into())
        .spawn(move || run(tx, thread_stop))
        .ok()?;
    log::info!("[WindowHandler] source: GNOME Shell helper");
    Some(Watcher {
        stop,
        thread: Some(thread),
    })
}

fn probe_state() -> Option<Vec<NativeMonitorState>> {
    use zbus::blocking::{Connection, Proxy};
    let connection = Connection::session().ok()?;
    let proxy = Proxy::new(&connection, SERVICE, PATH, INTERFACE).ok()?;
    let json: String = proxy.call("GetState", &()).ok()?;
    parse_state(&json).ok()
}

fn run(tx: CommandSender, stop: Arc<AtomicBool>) {
    use zbus::blocking::{Connection, Proxy};
    let Ok(connection) = Connection::session() else {
        return;
    };
    let Ok(proxy) = Proxy::new(&connection, SERVICE, PATH, INTERFACE) else {
        return;
    };

    let mut previous: Option<Vec<NativeMonitorState>> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match proxy.call::<_, _, String>("GetState", &()) {
            Ok(json) => match parse_state(&json) {
                Ok(state) => {
                    if previous.as_ref() != Some(&state) {
                        log::debug!("[WindowHandler] GNOME monitors: {state:?}");
                        previous = Some(state.clone());
                        if tx.send(Command::NativeMonitors(state)).is_err() {
                            return;
                        }
                    }
                }
                Err(err) => {
                    log::warn!("[WindowHandler] GNOME helper returned invalid state: {err}");
                }
            },
            // The shell reloads extensions when it restarts or when the
            // session changes. Clear the last native state so an app that is
            // gone cannot leave the wallpaper paused forever; the helper will
            // send a fresh list when it returns.
            Err(err) => {
                if previous.take().is_some() {
                    log::warn!("[WindowHandler] GNOME helper unavailable: {err}");
                    let _ = tx.send(Command::NativeMonitors(Vec::new()));
                }
            }
        }
        // Sleep in short slices so shutdown never waits for a full poll.
        let slices = (POLL.as_millis() / 100).max(1) as usize;
        for _ in 0..slices {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(default)]
    monitors: Vec<WireMonitor>,
}

#[derive(Debug, Deserialize)]
struct WireMonitor {
    #[serde(default)]
    connector: Option<String>,
    #[serde(default)]
    index: i32,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    #[serde(default)]
    covered: bool,
    #[serde(default)]
    fullscreen: bool,
}

fn parse_state(json: &str) -> anyhow::Result<Vec<NativeMonitorState>> {
    let payload: Payload = serde_json::from_str(json)?;
    Ok(payload
        .monitors
        .into_iter()
        .map(|monitor| NativeMonitorState {
            connector: monitor.connector.filter(|value| !value.trim().is_empty()),
            index: monitor.index,
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
            visibility: MonitorVisibility {
                covered: monitor.covered,
                fullscreen: monitor.fullscreen,
            },
        })
        .collect())
}

/// Directory GNOME Shell scans for per-user extensions.
pub fn extension_dir() -> PathBuf {
    data_home().join("gnome-shell/extensions").join(UUID)
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home().join(".local/share"))
}

/// Install the bundled helper and try to enable it.
///
/// GNOME Shell only scans the extensions directory at startup. On a running
/// Wayland session `gnome-extensions enable` therefore fails for a fresh
/// directory; the UUID is added to `org.gnome.shell enabled-extensions` so the
/// next login enables it without another command.
pub fn install() -> anyhow::Result<String> {
    if !paths::is_gnome() {
        anyhow::bail!("the window-state helper is only used on GNOME");
    }
    let dir = extension_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("metadata.json"), METADATA)?;
    std::fs::write(dir.join("extension.js"), EXTENSION)?;

    let enabled_now = gnome_extensions(&["enable", UUID]);
    if !enabled_now {
        set_enabled_setting(true);
        return Ok(format!(
            "installed {} — GNOME Shell has not scanned it yet; log out and back in once, then it will be enabled automatically",
            dir.display()
        ));
    }
    Ok(format!("installed and enabled {}", dir.display()))
}

/// Remove the helper and its enabled-extensions entry.
pub fn remove() -> anyhow::Result<String> {
    let dir = extension_dir();
    let _ = gnome_extensions(&["disable", UUID]);
    set_enabled_setting(false);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(format!("removed {}", dir.display()))
}

/// Human-readable installation and service status.
pub fn status() -> String {
    let dir = extension_dir();
    let installed = dir.join("metadata.json").is_file() && dir.join("extension.js").is_file();
    let enabled = gsettings_get("enabled-extensions")
        .map(|value| parse_strv(&value).iter().any(|item| item == UUID))
        .unwrap_or(false);
    format!(
        "GNOME window helper\n  path: {}\n  installed: {}\n  enabled in settings: {}\n  service: {}",
        dir.display(),
        if installed { "yes" } else { "no" },
        if enabled { "yes" } else { "no" },
        if available() { "available" } else { "not available" }
    )
}

fn gnome_extensions(args: &[&str]) -> bool {
    let mut command = ProcessCommand::new("gnome-extensions");
    command.args(args);
    util::output_with_timeout(&mut command, Duration::from_secs(5))
        .is_some_and(|output| output.status.success())
}

fn gsettings_get(key: &str) -> Option<String> {
    let mut command = ProcessCommand::new("gsettings");
    command.args(["get", "org.gnome.shell", key]);
    let output = util::output_with_timeout(&mut command, Duration::from_secs(2))?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn gsettings_set(key: &str, value: &str) -> bool {
    let mut command = ProcessCommand::new("gsettings");
    command.args(["set", "org.gnome.shell", key, value]);
    util::run_with_timeout(&mut command, Duration::from_secs(2))
}

fn set_enabled_setting(enabled: bool) {
    let Some(current) = gsettings_get("enabled-extensions") else {
        return;
    };
    let mut items = parse_strv(&current);
    let present = items.iter().any(|item| item == UUID);
    if enabled && !present {
        items.push(UUID.to_string());
    } else if !enabled && present {
        items.retain(|item| item != UUID);
    } else {
        return;
    }
    let _ = gsettings_set("enabled-extensions", &format_strv(&items));
}

/// Parse the GVariant string list `gsettings get` prints.
fn parse_strv(text: &str) -> Vec<String> {
    // `gsettings get` prints `@as []` for an empty typed string array.
    let text = text.trim();
    let text = text.strip_prefix("@as ").unwrap_or(text);
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|item| item.trim().trim_matches('\'').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn format_strv(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|item| format!("'{}'", item.replace('\'', "\\'")))
        .collect();
    format!("[{}]", quoted.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitor_coverage_and_optional_connector() {
        let json = r#"{"monitors":[
            {"index":0,"connector":"eDP-1","x":0,"y":0,"width":1920,"height":1080,"covered":true,"fullscreen":true},
            {"index":1,"x":1920,"y":0,"width":1920,"height":1080,"covered":false,"fullscreen":false}
        ]}"#;
        let state = parse_state(json).unwrap();
        assert_eq!(state.len(), 2);
        assert_eq!(state[0].connector.as_deref(), Some("eDP-1"));
        assert!(state[0].visibility.covered);
        assert!(state[0].visibility.fullscreen);
        assert_eq!(state[1].connector, None);
        assert!(!state[1].visibility.covered);
    }

    #[test]
    fn an_empty_helper_state_is_an_empty_list() {
        assert!(parse_state(r#"{"monitors":[]}"#).unwrap().is_empty());
        assert!(parse_state(r#"{}"#).unwrap().is_empty());
    }

    #[test]
    fn malformed_helper_state_is_rejected() {
        assert!(parse_state("not json").is_err());
    }

    #[test]
    fn gvariant_string_lists_round_trip() {
        let parsed = parse_strv("['ubuntu-dock@ubuntu.com', 'ding@rastersoft.com']");
        assert_eq!(
            parsed,
            vec!["ubuntu-dock@ubuntu.com", "ding@rastersoft.com"]
        );
        assert_eq!(
            format_strv(&parsed),
            "['ubuntu-dock@ubuntu.com', 'ding@rastersoft.com']"
        );
        assert!(parse_strv("[]").is_empty());
        assert!(parse_strv("@as []").is_empty());
        assert_eq!(parse_strv("@as ['one@example']"), vec!["one@example"]);
    }
}
